//! Native process scan. The host TUI calls this; WASM never does.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `zellij action list-panes` can hang forever on a stuck client IPC.
/// Without a cap, one reconcile holds `reconcile.lock` and the board
/// keeps jumping to stale pane ids (dead pane → focus no-op → master).
const ZELLIJ_CLI_TIMEOUT: Duration = Duration::from_secs(3);

use crate::agent::{keep_cursor_agent, AgentId, PanePlace};
use crate::catalog::catalog;
use crate::protocol::{ensure_state, seen_dir, spool_dir, started_dir};

pub fn scan_host_text() -> String {
    ensure_state();
    let epoch = unix_now();
    let session_prefix = std::env::var("ZAB_SCAN_SESSION_PREFIX").ok();
    let mut out = format!(
        "META hooks={} epoch={epoch}\n",
        if hooks_installed() { 1 } else { 0 }
    );
    let pids = agent_pids();
    if pids.is_empty() {
        return out;
    }
    let args_by_pid = ps_args(&pids);
    let env_by_pid = ps_env(&pids);
    let mut live_keys: HashSet<String> = HashSet::new();
    let mut unplaced: Vec<Unplaced> = Vec::new();
    let mut panes_by_session: BTreeMap<String, Option<HashSet<u32>>> = BTreeMap::new();
    let mut pane_snapshots: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut codex_panes = Vec::new();
    for pid in &pids {
        let Some(args) = args_by_pid.get(pid) else {
            continue;
        };
        let argv = split_args(args);
        if !keep_cursor_agent(&argv) {
            continue;
        }
        let comm = argv
            .first()
            .map(|bin| bin.rsplit('/').next().unwrap_or(bin).to_string())
            .unwrap_or_else(|| "agent".into());
        if argv.last().map(String::as_str) == Some("ls") {
            let needle = catalog()
                .adapter_for_bin(&comm)
                .and_then(|adapter| adapter.chat_store_needle.as_deref());
            if let Some(needle) = needle {
                if !holding_chat_store(*pid, needle) {
                    continue;
                }
            }
        }
        // Node `process.title` overwrites the ps-visible env block, so a live
        // `pi` has no ZELLIJ_PANE_ID here even though the pane is real.
        let Some(blob) = env_by_pid.get(pid) else {
            unplaced.push(Unplaced {
                pid: *pid,
                bin: comm,
            });
            continue;
        };
        let Some((session, pane)) = zellij_ids_from_env_blob(blob) else {
            unplaced.push(Unplaced {
                pid: *pid,
                bin: comm,
            });
            continue;
        };
        if !session_is_in_scope(&session, session_prefix.as_deref()) {
            continue;
        }
        let listed = panes_by_session.entry(session.clone()).or_insert_with(|| {
            let json = pane_snapshots
                .entry(session.clone())
                .or_insert_with(|| list_panes_json(&session));
            terminal_pane_ids(&session, json.as_deref()?)
        });
        if !pane_still_open(pane, listed.as_ref()) {
            continue;
        }
        // One Agent is one pane: a wrapper and its re-exec'd child, or a
        // second CLI spawned inside the same pane, share the key. Pids
        // ascend, so the process the pane launched wins.
        let key = format!("{session}-{pane}");
        if !live_keys.insert(key.clone()) {
            continue;
        }
        out.push_str(&format!("SCAN {session} {pane} {comm} {args}\n"));
        append_saved_lines(&mut out, &key);
        if comm == "codex" {
            if let Some(title) = pane_snapshots
                .get(&session)
                .and_then(|text| text.as_deref())
                .and_then(|text| {
                    let items: Vec<serde_json::Value> = serde_json::from_str(text).ok()?;
                    items.into_iter().find(|item| {
                        item["is_plugin"] == false && item["id"].as_u64() == Some(u64::from(pane))
                    })?["title"]
                        .as_str()
                        .map(str::to_owned)
                })
            {
                let resume = argv
                    .windows(2)
                    .find(|pair| pair[0] == "resume")
                    .map(|pair| &pair[1]);
                codex_panes.push(serde_json::json!({"session": session, "pane": pane, "title": title, "resume_thread": resume}));
            }
        }
    }
    recover_title_clobbered_panes(
        &mut out,
        &mut live_keys,
        &unplaced,
        session_prefix.as_deref(),
    );
    append_codex_terminal_notices(&mut out, &codex_panes);
    // A failed match (0 live keys) must not wipe hook history.
    if !live_keys.is_empty() {
        prune_spool(&live_keys);
        prune_dir(&seen_dir(), &live_keys);
        prune_dir(&started_dir(), &live_keys);
    }
    out
}

struct Unplaced {
    pid: u32,
    bin: String,
}

/// Recover terminal failures/interruptions that Codex did not send as hooks.
/// One bounded, read-only batch; Python/DB absence preserves ordinary hooks.
fn append_codex_terminal_notices(out: &mut String, panes: &[serde_json::Value]) {
    if panes.is_empty() {
        return;
    }
    let home = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex"));
    let python = [
        "/opt/homebrew/bin/python3",
        "/usr/local/bin/python3",
        "/usr/bin/python3",
    ]
    .into_iter()
    .find(|path| Path::new(path).is_file())
    .unwrap_or("python3");
    let mut command = Command::new(python);
    command.args(["-c", include_str!("../scripts/codex-terminal-state.py")]);
    command.arg(serde_json::json!({"home":home,"panes":panes}).to_string());
    let Some(output) =
        command_output_timeout(command, Duration::from_secs(2), "codex-terminal-state", "")
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let Ok(notices) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) else {
        return;
    };
    for notice in notices {
        let Some(session) = notice["session"].as_str() else {
            continue;
        };
        let Some(pane) = notice["pane"].as_u64() else {
            continue;
        };
        let Some(at_ms) = notice["at_ms"].as_u64() else {
            continue;
        };
        let Some(event) = notice["event"]
            .as_str()
            .filter(|event| matches!(*event, "stopFailure" | "interrupt"))
        else {
            continue;
        };
        if !panes.iter().any(|candidate| {
            candidate["session"] == session && candidate["pane"].as_u64() == Some(pane)
        }) {
            continue;
        }
        out.push_str(&format!(
            "HOOK {session} {pane} {event} @{} ~{at_ms} codex_rollout\n",
            at_ms / 1000
        ));
    }
}

fn append_saved_lines(out: &mut String, key: &str) {
    for read in [read_spool, read_seen, read_started] {
        if let Some(text) = read(key) {
            out.push_str(&text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
    }
}

/// `ps eww` cannot see `ZELLIJ_PANE_ID` after Node sets `process.title`.
/// The Zellij server process still carries `ZELLIJ_SESSION_NAME`, and
/// `list-panes` `pane_command` still starts with the CLI name.
fn recover_title_clobbered_panes(
    out: &mut String,
    live_keys: &mut HashSet<String>,
    unplaced: &[Unplaced],
    session_prefix: Option<&str>,
) {
    if unplaced.is_empty() {
        return;
    }
    let mut by_session: BTreeMap<String, Vec<&Unplaced>> = BTreeMap::new();
    for row in unplaced {
        let Some(session) = session_from_ancestors(row.pid) else {
            continue;
        };
        if !session_is_in_scope(&session, session_prefix) {
            continue;
        }
        by_session.entry(session).or_default().push(row);
    }
    for (session, rows) in by_session {
        let Some(json) = list_panes_json(&session) else {
            continue;
        };
        let hits = catalog_panes_from_json(&json);
        let mut taken: HashSet<u32> = live_keys
            .iter()
            .filter_map(|key| pane_id_from_key(key, &session))
            .collect();
        for row in rows {
            let cwd = process_cwd(row.pid);
            let Some(hit) = pick_pane(&hits, &row.bin, cwd.as_deref(), &taken) else {
                continue;
            };
            let key = format!("{session}-{}", hit.pane_id);
            if !live_keys.insert(key.clone()) {
                continue;
            }
            taken.insert(hit.pane_id);
            let args = hit.argv.join(" ");
            out.push_str(&format!(
                "SCAN {session} {} {} {args}\n",
                hit.pane_id, hit.bin
            ));
            append_saved_lines(out, &key);
        }
    }
}

fn pane_id_from_key(key: &str, session: &str) -> Option<u32> {
    let (name, pane) = key.rsplit_once('-')?;
    (name == session).then(|| pane.parse().ok())?
}

fn session_from_ancestors(pid: u32) -> Option<String> {
    let mut current = pid;
    for _ in 0..8 {
        let parent = parent_pid(current)?;
        if let Some(blob) = ps_env(&[parent]).get(&parent) {
            if let Some(session) = env_value(blob, "ZELLIJ_SESSION_NAME") {
                if !session.is_empty() {
                    return Some(session);
                }
            }
        }
        if parent == 1 {
            break;
        }
        current = parent;
    }
    None
}

fn parent_pid(pid: u32) -> Option<u32> {
    let output = Command::new(ps_bin())
        .args(["-p", &pid.to_string(), "-o", "ppid="])
        .output()
        .ok()?;
    let ppid = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    (ppid > 0 && ppid != pid).then_some(ppid)
}

fn process_cwd(pid: u32) -> Option<String> {
    let output = Command::new(lsof_bin())
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix('n').map(str::to_string))
}

struct PaneHit {
    pane_id: u32,
    cwd: String,
    argv: Vec<String>,
    bin: String,
}

fn catalog_panes_from_json(json: &str) -> Vec<PaneHit> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            if item.get("is_plugin").and_then(serde_json::Value::as_bool) != Some(false) {
                return None;
            }
            if item.get("exited").and_then(serde_json::Value::as_bool) == Some(true) {
                return None;
            }
            let command = item
                .get("pane_command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let mut argv = argv_before_glued_env(command);
            if !crate::agent::keep_cursor_agent(&argv) {
                return None;
            }
            let bin = argv
                .first()?
                .rsplit('/')
                .next()
                .unwrap_or(argv.first()?.as_str())
                .to_string();
            let cwd = item
                .get("pane_cwd")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            if crate::agent::workspace_from_argv(&argv).is_none()
                && !cwd.is_empty()
                && !cwd.chars().any(char::is_whitespace)
            {
                argv.push("--workspace".into());
                argv.push(cwd.clone());
            }
            let pane_id = item.get("id").and_then(serde_json::Value::as_u64)? as u32;
            Some(PaneHit {
                pane_id,
                cwd,
                argv,
                bin,
            })
        })
        .collect()
}

fn pick_pane<'a>(
    hits: &'a [PaneHit],
    bin: &str,
    cwd: Option<&str>,
    taken: &HashSet<u32>,
) -> Option<&'a PaneHit> {
    let mut matched: Vec<&PaneHit> = hits
        .iter()
        .filter(|hit| hit.bin == bin && !taken.contains(&hit.pane_id))
        .collect();
    if matched.len() > 1 {
        if let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) {
            let narrowed: Vec<&PaneHit> = matched
                .iter()
                .copied()
                .filter(|hit| hit.cwd == cwd)
                .collect();
            if !narrowed.is_empty() {
                matched = narrowed;
            }
        }
    }
    matched.into_iter().next()
}

/// Node's `process.title` smashes the NUL between argv and env, so Zellij's
/// `pane_command` looks like `pi CAML_LD_LIBRARY_PATH=/...`.
fn argv_before_glued_env(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .take_while(|token| !is_glued_env(token))
        .map(str::to_string)
        .collect()
}

fn is_glued_env(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn session_is_in_scope(session: &str, prefix: Option<&str>) -> bool {
    prefix
        .filter(|prefix| !prefix.is_empty())
        .is_none_or(|prefix| session.starts_with(prefix))
}

/// Titles from every session's `list-panes`. The WASM bridge only sees
/// other sessions through SessionUpdate, which often arrives with blank
/// pane names — done/time still come from hooks.
pub fn scan_places() -> Vec<(AgentId, PanePlace)> {
    scan_places_for(&list_sessions())
}

pub fn scan_places_for(sessions: &[String]) -> Vec<(AgentId, PanePlace)> {
    let mut out = Vec::new();
    for session in sessions {
        if session.is_empty() {
            continue;
        }
        let Some(output) = list_panes_json(session) else {
            continue;
        };
        out.extend(places_from_list_panes_json(session, &output));
    }
    out
}

pub fn places_from_list_panes_json(session: &str, json: &str) -> Vec<(AgentId, PanePlace)> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            if item.get("is_plugin").and_then(serde_json::Value::as_bool) != Some(false) {
                return None;
            }
            let pane_id = item.get("id").and_then(serde_json::Value::as_u64)? as u32;
            let tab_position = item
                .get("tab_position")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as usize;
            let tab_name = item
                .get("tab_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let pane_title = item
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            Some((
                AgentId {
                    session: session.to_string(),
                    pane_id,
                },
                PanePlace {
                    tab_position,
                    tab_name,
                    pane_title,
                },
            ))
        })
        .collect()
}

/// `None` means list-panes failed or timed out — keep the row (fail
/// open) so a probe glitch cannot empty the board. `Some` is the live
/// terminal pane ids; a process whose `ZELLIJ_PANE_ID` is missing is an
/// orphan left behind after the pane closed.
fn terminal_pane_ids(session: &str, json: &str) -> Option<HashSet<u32>> {
    Some(
        places_from_list_panes_json(session, json)
            .into_iter()
            .map(|(id, _)| id.pane_id)
            .collect(),
    )
}

fn list_panes_json(session: &str) -> Option<String> {
    let mut cmd = Command::new(zellij_bin());
    cmd.args([
        "--session",
        session,
        "action",
        "list-panes",
        "--all",
        "--json",
    ]);
    let output = command_output_timeout(cmd, ZELLIJ_CLI_TIMEOUT, "list-panes", session)?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn pane_still_open(pane: u32, listed: Option<&HashSet<u32>>) -> bool {
    listed.is_none_or(|ids| ids.contains(&pane))
}

fn list_sessions() -> Vec<String> {
    let mut cmd = Command::new(zellij_bin());
    cmd.args(["list-sessions", "-n"]);
    let Some(output) = command_output_timeout(cmd, ZELLIJ_CLI_TIMEOUT, "list-sessions", "") else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

/// Run a command with a wall-clock cap. Zellij IPC can stall; without
/// this, reconcile holds the store lock and the board jumps with stale
/// pane ids.
fn command_output_timeout(
    mut cmd: Command,
    timeout: Duration,
    kind: &str,
    session: &str,
) -> Option<Output> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn().ok()?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Some(output),
        Ok(Err(_)) => None,
        Err(_) => {
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
            let _ = rx.recv_timeout(Duration::from_secs(1));
            if session.is_empty() {
                log::warn!(
                    "zellij_cli_timeout kind={kind} timeout_ms={}",
                    timeout.as_millis()
                );
            } else {
                log::warn!(
                    "zellij_cli_timeout kind={kind} session={session} timeout_ms={}",
                    timeout.as_millis()
                );
            }
            None
        }
    }
}

/// Candidate zellij locations, first hit wins. The plugin launcher and the
/// host TUI run with a minimal PATH (no cargo bin), so `cargo install
/// zellij` setups (Linux `~/.cargo/bin`) need explicit probes.
fn zellij_candidates(home: &Path) -> Vec<String> {
    vec![
        home.join(".cargo/bin/zellij").display().to_string(),
        "/usr/bin/zellij".into(),
        "/usr/local/bin/zellij".into(),
        "/opt/homebrew/bin/zellij".into(),
    ]
}

fn zellij_bin_from(explicit: Option<&str>, home: &Path) -> String {
    if let Some(path) = explicit.filter(|path| !path.is_empty()) {
        return path.to_string();
    }
    zellij_candidates(home)
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .unwrap_or_else(|| "zellij".into())
}

pub fn zellij_bin() -> String {
    let explicit = std::env::var("ZAB_ZELLIJ").ok();
    zellij_bin_from(explicit.as_deref(), &home_dir())
}

pub fn zellij_ids_from_env_blob(blob: &str) -> Option<(String, u32)> {
    let pane = env_value(blob, "ZELLIJ_PANE_ID")?;
    let session = env_value(blob, "ZELLIJ_SESSION_NAME")?;
    let pane_id = pane.parse().ok()?;
    (!session.is_empty()).then_some((session, pane_id))
}

fn env_value(blob: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let start = blob.find(&needle)? + needle.len();
    let rest = &blob[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let value = &rest[..end];
    (!value.is_empty()).then(|| value.to_string())
}

fn hooks_installed() -> bool {
    catalog().hook_installed(&home_dir())
}

fn home_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(Into::into)
        .unwrap_or_else(|_| "/tmp".into())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Candidate pids from a `ps ax -o pid=,comm=,args=` dump. pgrep is avoided on
/// purpose: macOS pgrep silently skips some live processes (observed with a
/// Zellij-launched `codebuddy`), while `ps ax` sees everything.
///
/// A candidate matches by comm (fast path for native CLIs) or by argv. The argv
/// gate is required for wrappers: the cursor CLI ends with `exec -a "$0" node
/// index.js`, so the kernel comm is `MainThread` while argv[0] is still the
/// agent path. `keep_process` keeps the catalog's bins and skip lists the
/// single source of truth for both gates.
fn agent_pids() -> Vec<u32> {
    let Ok(output) = Command::new(ps_bin())
        .args(["ax", "-ww", "-o", "pid=", "-o", "comm=", "-o", "args="])
        .output()
    else {
        return Vec::new();
    };
    agent_pids_from_text(&String::from_utf8_lossy(&output.stdout))
}

fn agent_pids_from_text(text: &str) -> Vec<u32> {
    let wanted = catalog();
    let mut pids = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(comm)) = (parts.next(), parts.next()) else {
            continue;
        };
        let bin = comm.rsplit('/').next().unwrap_or(comm);
        let argv: Vec<String> = parts.map(str::to_string).collect();
        if !wanted.wants_bin(bin) && !wanted.keep_process(&argv) {
            continue;
        }
        if let Ok(pid) = pid.parse::<u32>() {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

fn first_bin(candidates: &[&str], fallback: &str) -> String {
    candidates
        .iter()
        .copied()
        .find(|path| Path::new(path).is_file())
        .unwrap_or(fallback)
        .to_string()
}

fn ps_bin() -> String {
    first_bin(&["/bin/ps", "/usr/bin/ps"], "ps")
}

fn lsof_bin() -> String {
    first_bin(&["/usr/sbin/lsof", "/usr/bin/lsof"], "lsof")
}

fn ps_args(pids: &[u32]) -> std::collections::BTreeMap<u32, String> {
    ps_map(ps_command_args(pids, false))
}

fn ps_env(pids: &[u32]) -> std::collections::BTreeMap<u32, String> {
    ps_map(ps_command_args(pids, true))
}

/// macOS `ps` accepts BSD flags like `eww` only immediately after the verb.
/// `ps -p 1 eww` is `illegal argument: eww` and returns no env.
fn ps_command_args(pids: &[u32], include_env: bool) -> Vec<String> {
    let list = pids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut args = Vec::new();
    if include_env {
        args.push("eww".into());
    }
    args.push("-p".into());
    args.push(list);
    args.extend(["-ww".into(), "-o".into(), "pid=".into(), "-o".into()]);
    args.push(if include_env { "command=" } else { "args=" }.into());
    args
}

fn ps_map(args: Vec<String>) -> std::collections::BTreeMap<u32, String> {
    let mut map = std::collections::BTreeMap::new();
    if args.is_empty() {
        return map;
    }
    let Ok(output) = Command::new(ps_bin()).args(&args).output() else {
        return map;
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        let Some((pid, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if let Ok(pid) = pid.parse::<u32>() {
            map.insert(pid, rest.trim().to_string());
        }
    }
    map
}

fn split_args(args: &str) -> Vec<String> {
    args.split_whitespace().map(str::to_string).collect()
}

fn holding_chat_store(pid: u32, needle: &str) -> bool {
    let Ok(output) = Command::new(lsof_bin())
        .args(["-p", &pid.to_string(), "-Fn"])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.contains(needle) && line.contains("store.db"))
}

fn read_spool(key: &str) -> Option<String> {
    std::fs::read_to_string(spool_dir().join(key)).ok()
}

fn read_seen(key: &str) -> Option<String> {
    std::fs::read_to_string(seen_dir().join(key)).ok()
}

fn read_started(key: &str) -> Option<String> {
    std::fs::read_to_string(started_dir().join(key)).ok()
}

fn prune_spool(live_keys: &HashSet<String>) {
    prune_dir(&spool_dir(), live_keys);
}

fn prune_dir(dir: &Path, live_keys: &HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !live_keys.contains(name) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use std::collections::HashSet;

    use super::{
        agent_pids_from_text, command_output_timeout, pane_still_open, places_from_list_panes_json,
        ps_command_args, scan_places_for, session_is_in_scope, zellij_bin_from,
        zellij_ids_from_env_blob,
    };
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn command_output_timeout_kills_a_hung_child() {
        let started = Instant::now();
        let mut sleep = Command::new("sleep");
        sleep.arg("30");
        assert!(command_output_timeout(sleep, Duration::from_millis(200), "sleep", "").is_none());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn command_output_timeout_returns_fast_success() {
        let mut printf = Command::new("printf");
        printf.arg("ok");
        let output =
            command_output_timeout(printf, Duration::from_secs(2), "printf", "").expect("printf");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"ok");
    }

    #[test]
    fn drops_orphan_pane_ids_once_list_panes_is_known() {
        let listed = HashSet::from([0, 1]);
        assert!(pane_still_open(0, Some(&listed)));
        assert!(!pane_still_open(10, Some(&listed)));
        assert!(pane_still_open(10, None));
    }

    #[test]
    fn macos_ps_eww_comes_before_dash_p() {
        let args = ps_command_args(&[2880, 2957], true);
        assert_eq!(
            args,
            vec![
                "eww",
                "-p",
                "2880,2957",
                "-ww",
                "-o",
                "pid=",
                "-o",
                "command="
            ]
        );
        let args = ps_command_args(&[2880], false);
        assert_eq!(args, vec!["-p", "2880", "-ww", "-o", "pid=", "-o", "args="]);
    }

    #[test]
    fn reads_zellij_ids_from_a_ps_eww_blob() {
        let blob = "/Users/ww/.local/bin/agent --workspace /tmp/w ZELLIJ_PANE_ID=3 ZELLIJ_SESSION_NAME=ww HOME=/Users/ww";
        assert_eq!(zellij_ids_from_env_blob(blob), Some(("ww".into(), 3)));
    }

    #[test]
    fn ignores_a_blob_without_zellij() {
        assert_eq!(zellij_ids_from_env_blob("HOME=/tmp PATH=/bin"), None);
    }

    #[test]
    fn explicit_zellij_binary_wins_over_home_probes() {
        assert_eq!(
            zellij_bin_from(Some("/tmp/zellij-0.45.0"), Path::new("/home/ww")),
            "/tmp/zellij-0.45.0"
        );
    }

    #[test]
    fn scan_scope_keeps_only_harness_sessions() {
        assert!(session_is_in_scope("zabf-a-origin", Some("zabf-a-")));
        assert!(!session_is_in_scope("lp-bench", Some("zabf-a-")));
        assert!(session_is_in_scope("lp-bench", None));
        assert!(session_is_in_scope("lp-bench", Some("")));
    }

    #[test]
    fn agent_pids_reads_ps_comm_text() {
        let text = "  123 /Users/ww/.local/bin/agent\n  456 codebuddy\n  321 claude\n  654 opencode\n  789 /usr/sbin/distnoted\n  810 vim\n   32 /bin/zsh\n  999 MainThread /home/ww/.local/bin/agent --use-system-ca /home/ww/.cursor/index.js\n";
        assert_eq!(agent_pids_from_text(text), vec![123, 321, 456, 654, 999]);
    }

    #[test]
    fn live_scan_finds_zellij_agents_when_they_run() {
        let dump = std::process::Command::new(super::ps_bin())
            .args(["ax", "-ww", "-o", "pid=", "-o", "comm="])
            .output();
        let has_agent = dump
            .ok()
            .map(|output| {
                !super::agent_pids_from_text(&String::from_utf8_lossy(&output.stdout)).is_empty()
            })
            .unwrap_or(false);
        if !has_agent {
            return;
        }
        let text = super::scan_host_text();
        assert!(text.lines().any(|line| line.starts_with("SCAN ")), "{text}");
        let mut panes = std::collections::HashSet::new();
        for line in text.lines().filter(|line| line.starts_with("SCAN ")) {
            let key = line
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                panes.insert(key),
                "two SCAN rows for one pane: {line}\n{text}"
            );
        }
    }

    #[test]
    fn empty_session_list_does_not_query_zellij() {
        assert!(scan_places_for(&[]).is_empty());
    }

    #[test]
    fn list_panes_json_keeps_mysql_syncer_agent_title() {
        let json = r#"[
            {"id":139,"is_plugin":true,"title":"zellij:tab-bar","tab_position":0,"tab_name":"master"},
            {"id":2,"is_plugin":false,"title":"refactor/use-yotta","tab_position":1,"tab_name":"refactor/use-yotta"},
            {"id":3,"is_plugin":false,"title":"zsh","tab_position":1,"tab_name":"refactor/use-yotta"}
        ]"#;
        let places = places_from_list_panes_json("mysql_syncer", json);
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].0.session, "mysql_syncer");
        assert_eq!(places[0].0.pane_id, 2);
        assert_eq!(places[0].1.tab_name, "refactor/use-yotta");
        assert_eq!(places[0].1.pane_title, "refactor/use-yotta");
    }

    #[test]
    fn prefers_absolute_host_bins_when_present() {
        if Path::new("/bin/ps").is_file() {
            assert_eq!(super::ps_bin(), "/bin/ps");
        }
        if Path::new("/usr/sbin/lsof").is_file() {
            assert_eq!(super::lsof_bin(), "/usr/sbin/lsof");
        }
    }

    #[test]
    fn live_scan_keeps_a_pi_whose_ps_env_is_unreadable() {
        let dump = std::process::Command::new(super::ps_bin())
            .args(["ax", "-o", "pid=", "-o", "comm="])
            .output();
        let has_pi = dump
            .ok()
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.split_whitespace().nth(1) == Some("pi"))
            })
            .unwrap_or(false);
        if !has_pi {
            return;
        }
        let text = super::scan_host_text();
        assert!(
            text.lines()
                .any(|line| { line.split_whitespace().nth(3) == Some("pi") }),
            "{text}"
        );
    }

    #[test]
    fn pi_pane_command_keeps_the_bin_and_drops_glued_env() {
        assert_eq!(
            super::argv_before_glued_env("pi CAML_LD_LIBRARY_PATH=/Users/ww/.opam/lf/lib/ocaml"),
            vec!["pi".to_string()]
        );
        assert_eq!(
            super::argv_before_glued_env("pi -p hello CAML_LD_LIBRARY_PATH=/tmp"),
            vec!["pi".to_string(), "-p".into(), "hello".into()]
        );
        let json = r#"[
            {"id":0,"is_plugin":false,"pane_command":"/Users/ww/.local/bin/agent --workspace /tmp/w","pane_cwd":"/tmp/w","exited":false},
            {"id":1,"is_plugin":false,"pane_command":"pi CAML_LD_LIBRARY_PATH=/Users/ww/.opam/lf/lib/ocaml","pane_cwd":"/tmp/pi","exited":false},
            {"id":2,"is_plugin":false,"pane_command":"pi -p hello CAML_LD_LIBRARY_PATH=/tmp","pane_cwd":"/tmp/one-shot","exited":false},
            {"id":3,"is_plugin":false,"pane_command":"/bin/zsh","pane_cwd":"/tmp/sh","exited":false}
        ]"#;
        let hits = super::catalog_panes_from_json(json);
        assert_eq!(hits.len(), 2);
        let taken = HashSet::from([0]);
        let hit = super::pick_pane(&hits, "pi", Some("/tmp/pi"), &taken).expect("pi pane");
        assert_eq!(hit.pane_id, 1);
        assert_eq!(
            hit.argv,
            vec![
                "pi".to_string(),
                "--workspace".to_string(),
                "/tmp/pi".to_string()
            ]
        );
        assert!(super::pick_pane(&hits, "pi", Some("/tmp/missing"), &HashSet::from([1])).is_none());
    }

    #[test]
    fn zellij_candidates_probe_cargo_bin_first() {
        let candidates = super::zellij_candidates(Path::new("/Users/ww"));
        assert_eq!(
            candidates,
            vec![
                "/Users/ww/.cargo/bin/zellij".to_string(),
                "/usr/bin/zellij".to_string(),
                "/usr/local/bin/zellij".to_string(),
                "/opt/homebrew/bin/zellij".to_string(),
            ]
        );
    }
}
