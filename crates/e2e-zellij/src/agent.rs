//! Daemon-managed (supervisor-less) claude handle for the Zellij E2E.
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::oracle::{judge, Judgement};
use crate::zellij::Zellij;

/// Observation of a daemon-managed (supervisor-less) claude: phase and session
/// come from the daemon's relationship table (`--snapshot`), live pids from the
/// shared claude registry directory.
#[derive(Debug, Clone)]
pub struct DaemonSnapshot {
    pub phase: String,
    pub session_id: String,
    pub status: String,
    pub pane_id: i64,
    pub live_pids: Vec<u32>,
}

/// Handle to a claude the daemon drives through pane injection, no supervisor.
pub struct DaemonClaudeAgent {
    pub(crate) session: String,
    pub(crate) role: String,
    pub(crate) pane_id: i64,
    pub(crate) sessions_dir: PathBuf,
}

impl DaemonClaudeAgent {
    pub fn pane_exists(&self, zellij: &Zellij) -> bool {
        zellij.find_named_pane(&self.session, &self.role) == Some(self.pane_id)
    }

    fn live_pids(&self) -> Vec<u32> {
        let Ok(entries) = fs::read_dir(&self.sessions_dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_suffix(".json"))
                    .and_then(|pid| pid.parse::<u32>().ok())
            })
            .collect()
    }

    pub fn observe(&self, zellij: &Zellij) -> Result<DaemonSnapshot, String> {
        let real = zellij.real_name(&self.session);
        let snapshot = zellij.daemon_snapshot()?;
        let row = snapshot
            .get("sleep")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|row| {
                row.get("session").and_then(Value::as_str) == Some(real.as_str())
                    && row.get("pane_id").and_then(Value::as_u64) == Some(self.pane_id as u64)
            })
            .cloned()
            .unwrap_or(Value::Null);
        Ok(DaemonSnapshot {
            phase: row["phase"].as_str().unwrap_or_default().to_owned(),
            session_id: row["session_id"].as_str().unwrap_or_default().to_owned(),
            status: row["status"].as_str().unwrap_or_default().to_owned(),
            pane_id: self.pane_id,
            live_pids: self.live_pids(),
        })
    }

    pub(crate) fn wait_until_running(&self, zellij: &mut Zellij) -> Result<DaemonSnapshot, String> {
        self.drive(zellij, "daemon claude running", |snap| {
            snap.phase == "running"
                && snap.status == "idle"
                && !snap.session_id.is_empty()
                && !snap.live_pids.is_empty()
        })
    }

    pub fn sleep(&self, zellij: &mut Zellij) -> Result<DaemonSnapshot, String> {
        let real = zellij.real_name(&self.session);
        self.control(zellij, "--sleep", &real)?;
        self.drive(zellij, "daemon claude sleeping", |snap| {
            snap.phase == "sleeping" && snap.live_pids.is_empty()
        })
    }

    pub fn resume(&self, zellij: &mut Zellij) -> Result<DaemonSnapshot, String> {
        let real = zellij.real_name(&self.session);
        self.control(zellij, "--resume", &real)?;
        self.drive(zellij, "daemon claude resumed", |snap| {
            snap.phase == "running" && snap.status == "idle" && !snap.live_pids.is_empty()
        })
    }

    fn control(&self, zellij: &Zellij, op: &str, real: &str) -> Result<(), String> {
        let output = zellij.board_tui(&[op, real, &self.pane_id.to_string()])?;
        output
            .status
            .success()
            .then_some(())
            .ok_or_else(|| format!("{op} failed: {}", String::from_utf8_lossy(&output.stderr)))
    }

    /// Poll the daemon (reconcile + observe) until `done`, with harness health
    /// checks so a broken Zellij fails fast instead of timing out.
    fn drive(
        &self,
        zellij: &mut Zellij,
        description: &str,
        done: impl Fn(&DaemonSnapshot) -> bool,
    ) -> Result<DaemonSnapshot, String> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut next_health = Instant::now();
        let mut last;
        loop {
            if Instant::now() >= next_health {
                if let Judgement::Broken { hold, evidence } = judge(&zellij.observe()) {
                    return Err(format!("{hold}: {evidence}"));
                }
                next_health = Instant::now() + Duration::from_millis(500);
            }
            zellij.reconcile()?;
            match self.observe(zellij) {
                Ok(snap) if done(&snap) => return Ok(snap),
                Ok(_) => last = "condition not met".to_owned(),
                Err(error) => last = error,
            }
            if Instant::now() >= deadline {
                return Err(format!("{description}: {last}"));
            }
            thread::sleep(Duration::from_millis(150));
        }
    }
}

pub fn process_exists(pid: Option<u32>) -> bool {
    let Some(pid) = pid else {
        return false;
    };
    unsafe { nix::libc::kill(pid as i32, 0) == 0 }
}
