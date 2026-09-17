//! Real attached Zellij environment for one scenario.
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::oracle::Observation;
use crate::pty::PtyClient;
use crate::util::{
    err, pane_blob, parse_pane_id, require_artifact, resolve_zellij, strip_ansi,
    workspace_target_dir,
};

const NAMED_PANE_KEYS: &[&str] = &["title", "pane_command", "terminal_command", "name"];

pub struct Zellij {
    zellij: PathBuf,
    fake_claude: PathBuf,
    wasm: PathBuf,
    tui: PathBuf,
    isolate: PathBuf,
    sock: PathBuf,
    config: PathBuf,
    real_names: BTreeMap<String, String>,
    pty: Option<PtyClient>,
    server_cookie: BTreeMap<String, u64>,
    agent_serial: u32,
    /// `(session, pane id)` of every planted agent; the role only names the pane.
    created_agents: Vec<(String, i64)>,
    env_pairs: Vec<(String, String)>,
}

impl Zellij {
    /// Start an isolated Zellij client attached to a fresh `work` session.
    pub fn start() -> Result<Self, String> {
        let zellij = resolve_zellij()?;
        let target = workspace_target_dir();
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let fake_claude = target.join(profile).join("fake-claude");
        require_artifact(&fake_claude, "fake-claude")?;
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let wasm = repo.join("target/wasm32-wasip1/release/zellij-agent-board.wasm");
        require_artifact(&wasm, "zellij-agent-board.wasm")?;
        let tui = target.join(profile).join("board-tui");
        require_artifact(&tui, "board-tui")?;
        let isolate = tempfile::Builder::new()
            .prefix("zabf-")
            .tempdir_in("/tmp")
            .map_err(err)?
            .keep();
        let sock = isolate.join("s");
        let state = isolate.join("z");
        fs::create_dir_all(&sock).map_err(err)?;
        fs::create_dir_all(&state).map_err(err)?;
        fs::create_dir_all(isolate.join("claude-home").join("sessions")).map_err(err)?;
        let token = format!("{:04x}", (std::process::id() ^ 0xa5a5) as u16);
        let session_prefix = format!("z{token}");
        let env_pairs = build_env_pairs(&isolate, &sock, &state, &zellij, &session_prefix, &tui);
        let mut world = Self {
            zellij,
            fake_claude,
            wasm,
            tui,
            isolate: isolate.clone(),
            sock,
            config: isolate.join("config.kdl"),
            real_names: BTreeMap::from([("work".to_owned(), format!("{session_prefix}0"))]),
            pty: None,
            server_cookie: BTreeMap::new(),
            agent_serial: 0,
            created_agents: Vec::new(),
            env_pairs,
        };
        world.write_config()?;
        world.create_session("work")?;
        let attach = [
            world.zellij.to_string_lossy().into_owned(),
            "--config".to_owned(),
            world.config.to_string_lossy().into_owned(),
            "attach".to_owned(),
            world.real_names["work"].clone(),
        ];
        world.pty = Some(PtyClient::spawn(&attach, &world.env_pairs).map_err(err)?);
        world
            .wait_until(Duration::from_secs(8), |world| {
                Ok(world.pty.as_mut().is_some_and(|pty| pty.alive())
                    && world.attached_logical()? == Some("work".to_owned()))
            })
            .map_err(|_| "world_never_quiet".to_owned())?;
        world.server_cookie = world.server_cookie_now();
        Ok(world)
    }

    /// Plant a claude the daemon manages *without* a supervisor: the pane's
    /// top-level is a plain shell and claude runs inside it, so its exit keeps
    /// the pane. The fake-claude artifact is run as `claude` so the builtin
    /// catalog recognises it; it shares the isolate's `CLAUDE_CONFIG_DIR`.
    pub fn start_daemon_claude_agent(&mut self) -> Result<crate::agent::DaemonClaudeAgent, String> {
        let session = "work".to_owned();
        self.agent_serial += 1;
        let role = format!("dclaude-{}", self.agent_serial);
        let claude = self.claude_named_binary()?;
        let real = self.real_names[&session].clone();
        // Interactive shell as the pane's top-level process; never close-on-exit.
        let output = self.zj(
            &["action", "new-pane", "--name", &role, "--", "bash", "-i"],
            Some(&real),
        )?;
        let pane_id = parse_pane_id(&String::from_utf8_lossy(&output.stdout))
            .or_else(|| self.find_named_pane_in(&real, &role))
            .ok_or_else(|| "daemon claude pane created: new-pane returned no pane".to_owned())?;
        self.created_agents.push((session.clone(), pane_id));
        // Nudge the shell to a prompt, then start claude as a child of the shell.
        self.zj(
            &[
                "action",
                "write",
                "--pane-id",
                &format!("terminal_{pane_id}"),
                "--",
                "13",
            ],
            Some(&real),
        )?;
        self.write_chars(&session, pane_id, &format!("{}\n", claude.display()))?;
        let agent = crate::agent::DaemonClaudeAgent {
            session,
            role,
            pane_id,
            sessions_dir: self.isolate.join("claude-home").join("sessions"),
        };
        agent.wait_until_running(self)?;
        Ok(agent)
    }

    /// The fake-claude artifact copied to a file literally named `claude`, so
    /// `ps` reports comm `claude` and the catalog's claude adapter matches.
    fn claude_named_binary(&self) -> Result<PathBuf, String> {
        let path = self.isolate.join("claude");
        if !path.exists() {
            fs::copy(&self.fake_claude, &path).map_err(err)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).map_err(err)?;
            }
        }
        Ok(path)
    }

    /// Run the host `board-tui` with the isolate environment (daemon client).
    pub(crate) fn board_tui(&self, args: &[&str]) -> Result<Output, String> {
        let mut command = Command::new(&self.tui);
        command.args(args);
        for (key, value) in &self.env_pairs {
            command.env(key, value);
        }
        command.output().map_err(err)
    }

    /// Request one scan so the daemon reconciles the relationship table.
    pub(crate) fn reconcile(&self) -> Result<(), String> {
        let output = self.board_tui(&["--reconcile"])?;
        output.status.success().then_some(()).ok_or_else(|| {
            format!(
                "reconcile failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
    }

    /// The daemon's committed snapshot, including the `sleep` relationship rows.
    pub(crate) fn daemon_snapshot(&self) -> Result<Value, String> {
        let output = self.board_tui(&["--snapshot"])?;
        if !output.status.success() {
            return Err(format!(
                "snapshot failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        serde_json::from_slice(&output.stdout).map_err(err)
    }

    pub(crate) fn real_name(&self, logical: &str) -> String {
        self.real_names[logical].clone()
    }

    pub fn write_chars(&mut self, session: &str, pane_id: i64, text: &str) -> Result<(), String> {
        let real = self.real_names[session].clone();
        let output = self.zj(
            &[
                "action",
                "write-chars",
                "--pane-id",
                &format!("terminal_{pane_id}"),
                "--",
                text,
            ],
            Some(&real),
        )?;
        if !output.status.success() {
            return Err(format!(
                "terminal input delivered: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    pub fn find_named_pane(&self, session: &str, role: &str) -> Option<i64> {
        let real = self.real_names.get(session)?;
        self.find_named_pane_in(real, role)
    }

    pub fn observe(&mut self) -> Observation {
        Observation {
            client_alive: self.pty.as_mut().is_some_and(|pty| pty.alive()),
            sessions: self.live_logical(),
            expected_sessions: self.real_names.keys().cloned().collect(),
            control_alive: self.control_alive(),
            server_restarted: self.server_restarted(),
            lost_connection: self
                .pty
                .as_ref()
                .is_some_and(|pty| pty.contains("Lost connection")),
        }
    }

    pub fn close_agents(&mut self) {
        let agents = std::mem::take(&mut self.created_agents);
        for (session, pane_id) in agents {
            let real = self.real_names[&session].clone();
            let _ = self.zj(
                &[
                    "action",
                    "close-pane",
                    "--pane-id",
                    &format!("terminal_{pane_id}"),
                ],
                Some(&real),
            );
        }
    }

    fn find_named_pane_in(&self, real: &str, role: &str) -> Option<i64> {
        for pane in self.panes_in(real, &["--command"]) {
            let blob = pane_blob(&pane, NAMED_PANE_KEYS);
            if blob.contains(role) {
                return pane.get("id").and_then(Value::as_i64);
            }
        }
        None
    }

    fn create_session(&mut self, name: &str) -> Result<(), String> {
        let real = self.real_names[name].clone();
        let output = self.zj(
            &[
                "--config",
                &self.config.to_string_lossy(),
                "attach",
                "--create-background",
                &real,
            ],
            None,
        )?;
        if !output.status.success() {
            return Err(format!(
                "session {name} failed to start: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        self.wait_until(Duration::from_secs(6), |world| {
            Ok(world
                .zj(&["action", "list-tabs", "--json"], Some(&real))?
                .status
                .success())
        })
        .map_err(|_| {
            format!(
                "session {name} failed to start: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
    }

    fn write_config(&self) -> Result<(), String> {
        let wasm = self.wasm.display();
        let tui = self.tui.display();
        let text = format!(
            r#"keybinds clear-defaults=true {{
    shared {{
        bind "Alt q" {{
            LaunchPlugin "file:{wasm}" {{
                floating true
                tui "{tui}"
            }}
        }}
    }}
}}
session_serialization false
show_startup_tips false
show_release_notes false
"#
        );
        fs::write(&self.config, text).map_err(err)
    }

    fn zj(&self, args: &[&str], session: Option<&str>) -> Result<Output, String> {
        let mut command = Command::new(&self.zellij);
        if let Some(session) = session {
            command.args(["--session", session]);
        }
        command.args(args);
        for (key, value) in &self.env_pairs {
            command.env(key, value);
        }
        command.output().map_err(err)
    }

    fn wait_until(
        &mut self,
        timeout: Duration,
        mut pred: impl FnMut(&mut Self) -> Result<bool, String>,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut last = "condition not met".to_owned();
        while Instant::now() < deadline {
            match pred(self) {
                Ok(true) => return Ok(()),
                Ok(false) => last = "condition not met".to_owned(),
                Err(error) => last = error,
            }
            thread::sleep(Duration::from_millis(150));
        }
        Err(last)
    }

    fn live_logical(&self) -> BTreeSet<String> {
        let text = self
            .zj(&["list-sessions", "-n"], None)
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default();
        self.real_names
            .iter()
            .filter(|(_, real)| {
                text.lines()
                    .any(|line| line.split_whitespace().next() == Some(real.as_str()))
            })
            .map(|(logical, _)| logical.clone())
            .collect()
    }

    fn attached_logical(&mut self) -> Result<Option<String>, String> {
        for (logical, real) in &self.real_names.clone() {
            let clients = self.zj(&["action", "list-clients"], Some(real))?;
            if clients.status.success()
                && String::from_utf8_lossy(&clients.stdout)
                    .lines()
                    .any(|line| {
                        line.split_whitespace()
                            .next()
                            .is_some_and(|tok| tok.chars().all(|ch| ch.is_ascii_digit()))
                    })
            {
                return Ok(Some(logical.clone()));
            }
        }
        if let Some(pty) = &self.pty {
            let text = strip_ansi(&pty.transcript());
            if let Some(current) = text.rmatch_indices("Zellij (").next().and_then(|(idx, _)| {
                let rest = &text[idx + "Zellij (".len()..];
                rest.split(')').next().map(str::to_owned)
            }) {
                for (logical, real) in &self.real_names {
                    if *real == current {
                        return Ok(Some(logical.clone()));
                    }
                }
            }
        }
        Ok(None)
    }

    fn control_alive(&self) -> bool {
        self.real_names.values().any(|real| {
            self.zj(&["action", "list-tabs", "--json"], Some(real))
                .map(|output| output.status.success())
                .unwrap_or(false)
        })
    }

    fn panes_in(&self, real: &str, extra: &[&str]) -> Vec<Value> {
        let mut args = vec!["action", "list-panes", "--json"];
        args.extend_from_slice(extra);
        args.push("--all");
        let Ok(output) = self.zj(&args, Some(real)) else {
            return Vec::new();
        };
        serde_json::from_slice::<Value>(&output.stdout)
            .ok()
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
    }

    fn server_cookie_now(&self) -> BTreeMap<String, u64> {
        let mut cookie = BTreeMap::new();
        if let Ok(entries) = fs::read_dir(&self.sock) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        cookie.insert(entry.file_name().to_string_lossy().into(), meta.ino());
                    }
                }
            }
        }
        cookie
    }

    fn server_restarted(&self) -> bool {
        if self.server_cookie.is_empty() {
            return false;
        }
        let now = self.server_cookie_now();
        self.server_cookie
            .iter()
            .any(|(name, ino)| now.get(name).is_some_and(|current| current != ino))
    }

    fn teardown(&mut self) {
        let transcript = self.pty.as_ref().map(PtyClient::transcript);
        self.close_agents();
        if let Some(mut pty) = self.pty.take() {
            pty.close();
        }
        for real in self.real_names.values() {
            let _ = Command::new(&self.zellij)
                .args(["delete-session", "--force", "--", real])
                .envs(self.env_pairs.iter().cloned())
                .output();
        }
        // The board starts a state owner lazily and outlives its pane. Stop it
        // before the isolate (which holds its database) goes away.
        let _ = Command::new(&self.tui)
            .arg("--daemon-stop")
            .envs(self.env_pairs.iter().cloned())
            .output();
        self.keep_or_remove_isolate(transcript);
    }

    /// A failing case keeps its isolate where CI can upload it; a passing one
    /// cleans up. Drop runs during unwinding, so `panicking` is the failure
    /// signal available here.
    fn keep_or_remove_isolate(&self, transcript: Option<String>) {
        if !thread::panicking() {
            let _ = fs::remove_dir_all(&self.isolate);
            return;
        }
        if let Some(text) = transcript {
            let _ = fs::write(self.isolate.join("transcript.txt"), text);
        }
        let name = self
            .isolate
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "zabf".to_owned());
        let _ = copy_tree(&self.isolate, &failure_artifacts_dir().join(name));
    }
}

fn failure_artifacts_dir() -> PathBuf {
    workspace_target_dir().join("e2e-zellij")
}

/// Recursive copy of regular files and directories; live sockets and other
/// special entries are skipped rather than aborting the copy.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

impl Drop for Zellij {
    fn drop(&mut self) {
        self.teardown();
    }
}

fn build_env_pairs(
    isolate: &std::path::Path,
    sock: &std::path::Path,
    state: &std::path::Path,
    zellij: &std::path::Path,
    session_prefix: &str,
    tui: &std::path::Path,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = env::vars().collect();
    env.retain(|(key, _)| key != "ZELLIJ" && key != "ZELLIJ_SESSION_NAME");
    env.push(("TMPDIR".into(), isolate.to_string_lossy().into()));
    env.push(("ZELLIJ_SOCKET_DIR".into(), sock.to_string_lossy().into()));
    env.push(("ZAB_STATE_DIR".into(), state.to_string_lossy().into()));
    env.push((
        "ZAB_JUMP_TRACE".into(),
        state.join("jump-trace").to_string_lossy().into(),
    ));
    env.push(("ZAB_ZELLIJ".into(), zellij.to_string_lossy().into()));
    env.push(("ZAB_SCAN_SESSION_PREFIX".into(), session_prefix.to_owned()));
    // The daemon and every claude in this isolate must agree on one registry
    // so the no-supervisor scan can read each session id from `sessions/<pid>`.
    env.push((
        "CLAUDE_CONFIG_DIR".into(),
        isolate.join("claude-home").to_string_lossy().into(),
    ));
    env.push((
        "ZELLIJ_AGENT_BOARD_TUI".into(),
        tui.to_string_lossy().into(),
    ));
    env
}
