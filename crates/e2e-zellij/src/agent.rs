//! Stable mock-agent handle; observations are immutable.
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::oracle::{judge, Judgement};
use crate::zellij::Zellij;

/// Immutable agent observation. Later operations do not update existing values.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub sequence: u64,
    pub state: String,
    pub error: Option<String>,
    pub pid: Option<u32>,
    pub supervisor_pid: u32,
    pub pane_id: i64,
    pub instance_id: String,
    pub conversation_id: String,
    pub messages: Vec<(String, String)>,
    pub restored_messages: Vec<(String, String)>,
    pub resumed: bool,
    pub activity: String,
    pub draft: String,
    pub clean_exit: bool,
}

#[derive(Debug, Clone)]
pub struct MockAgentOptions {
    pub session: String,
    pub state: String,
    pub history: Vec<String>,
    pub exit_behavior: String,
}

impl Default for MockAgentOptions {
    fn default() -> Self {
        Self {
            session: "work".to_owned(),
            state: "idle".to_owned(),
            history: Vec::new(),
            exit_behavior: "normal".to_owned(),
        }
    }
}

impl MockAgentOptions {
    pub fn idle_with_history(history: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            history: history.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    pub fn with_state(mut self, state: impl Into<String>) -> Self {
        self.state = state.into();
        self
    }

    pub fn with_exit_behavior(mut self, exit_behavior: impl Into<String>) -> Self {
        self.exit_behavior = exit_behavior.into();
        self
    }
}

pub struct MockAgent {
    pub(crate) session: String,
    pub(crate) role: String,
    pub(crate) pane_id: i64,
    pub(crate) directory: PathBuf,
}

impl MockAgent {
    pub fn pane_exists(&self, zellij: &Zellij) -> bool {
        zellij.find_named_pane(&self.session, &self.role) == Some(self.pane_id)
    }

    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let status: Value = read_json(&self.directory.join("status.json"))?;
        let events = read_events(&self.directory.join("chat/events.jsonl"))?;
        let ready = events
            .iter()
            .rev()
            .find(|event| event["event"] == "ready")
            .ok_or_else(|| "agent has not published a ready event".to_owned())?;
        if status["state"] == "running" && status["agent_pid"].as_u64() != ready["pid"].as_u64() {
            return Err("agent observations are still changing".to_owned());
        }
        let session_id = ready["session_id"]
            .as_str()
            .ok_or_else(|| "ready event missing session id".to_owned())?;
        let saved: Value = read_json(&self.directory.join(format!("chat/{session_id}.json")))?;
        let messages = |rows: &Value| -> Vec<(String, String)> {
            rows.as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| {
                    Some((
                        row["role"].as_str()?.to_owned(),
                        row["text"].as_str()?.to_owned(),
                    ))
                })
                .collect()
        };
        let instance_id = ready["instance_id"].as_str().unwrap_or_default().to_owned();
        let clean_exit = events.iter().any(|event| {
            event["event"] == "exited"
                && event["instance_id"].as_str() == Some(instance_id.as_str())
        });
        Ok(Snapshot {
            sequence: status["sequence"].as_u64().unwrap_or(0),
            state: status["state"].as_str().unwrap_or("").to_owned(),
            error: status["error"].as_str().map(str::to_owned),
            pid: status["agent_pid"].as_u64().map(|pid| pid as u32),
            supervisor_pid: status["supervisor_pid"].as_u64().unwrap_or(0) as u32,
            pane_id: self.pane_id,
            instance_id,
            conversation_id: session_id.to_owned(),
            messages: messages(&saved["messages"]),
            restored_messages: messages(&ready["messages"]),
            resumed: ready["resumed"].as_bool().unwrap_or(false),
            activity: status["agent"]["status"].as_str().unwrap_or("").to_owned(),
            draft: status["agent"]["draft"].as_str().unwrap_or("").to_owned(),
            clean_exit,
        })
    }

    pub fn sleep(&self, zellij: &mut Zellij) -> Result<Snapshot, String> {
        let sequence = self.send(zellij, "sleep", &Value::Null)?;
        self.wait_result(zellij, sequence, "sleeping")
    }

    pub fn resume(&self, zellij: &mut Zellij) -> Result<Snapshot, String> {
        let sequence = self.send(zellij, "resume", &Value::Null)?;
        self.wait_result(zellij, sequence, "running")
    }

    pub(crate) fn prepare(
        &self,
        zellij: &mut Zellij,
        operation: &str,
        fields: Value,
    ) -> Result<(), String> {
        let sequence = self.send(zellij, operation, &fields)?;
        wait(zellij, &format!("mock acknowledged {operation}"), || {
            let status: Value = read_json(&self.directory.join("status.json"))?;
            Ok(status["sequence"].as_u64().unwrap_or(0) > sequence
                && status["agent"]["event"] == operation
                && status["error"].is_null())
        })?;
        Ok(())
    }

    pub(crate) fn wait_until_running(&self, zellij: &mut Zellij) -> Result<Snapshot, String> {
        wait(zellij, "agent running", || {
            let snap = self.snapshot()?;
            Ok(snap.state == "running" && snap.error.is_none() && process_exists(snap.pid))
        })?;
        self.snapshot()
    }

    fn send(&self, zellij: &mut Zellij, operation: &str, fields: &Value) -> Result<u64, String> {
        let status: Value = read_json(&self.directory.join("status.json"))?;
        let sequence = status["sequence"].as_u64().unwrap_or(0);
        let mut payload = serde_json::Map::new();
        payload.insert("op".to_owned(), Value::String(operation.to_owned()));
        if let Some(object) = fields.as_object() {
            for (key, value) in object {
                payload.insert(key.clone(), value.clone());
            }
        }
        let line = format!("{}\n", Value::Object(payload));
        zellij.write_chars(&self.session, self.pane_id, &line)?;
        Ok(sequence)
    }

    fn wait_result(
        &self,
        zellij: &mut Zellij,
        sequence: u64,
        success_state: &str,
    ) -> Result<Snapshot, String> {
        wait(
            zellij,
            &format!("operation completed: {success_state} or error"),
            || {
                let result = self.snapshot()?;
                if result.sequence <= sequence {
                    return Ok(false);
                }
                if result.error.is_some() {
                    return Ok(true);
                }
                if result.state != success_state {
                    return Ok(false);
                }
                if success_state == "sleeping" {
                    Ok(result.pid.is_none() && result.clean_exit)
                } else {
                    Ok(process_exists(result.pid))
                }
            },
        )?;
        self.snapshot()
    }
}

pub fn process_exists(pid: Option<u32>) -> bool {
    let Some(pid) = pid else {
        return false;
    };
    unsafe { nix::libc::kill(pid as i32, 0) == 0 }
}

fn read_json(path: &std::path::Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn read_events(path: &std::path::Path) -> Result<Vec<Value>, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|error| error.to_string()))
        .collect()
}

fn wait(
    zellij: &mut Zellij,
    description: &str,
    mut matches: impl FnMut() -> Result<bool, String>,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut next_health = Instant::now();
    let mut last;
    loop {
        // Health checks spawn Zellij CLI processes; poll them less often than
        // the agent status file.
        if Instant::now() >= next_health {
            match judge(&zellij.observe()) {
                Judgement::Held => {}
                Judgement::Broken { hold, evidence } => {
                    return Err(format!("{hold}: {evidence}"));
                }
            }
            next_health = Instant::now() + Duration::from_millis(500);
        }
        match matches() {
            Ok(true) => return Ok(()),
            Ok(false) => last = "condition not met".to_owned(),
            Err(error) => last = error,
        }
        if Instant::now() >= deadline {
            return Err(format!("{description}: {last}"));
        }
        thread::sleep(Duration::from_millis(100));
    }
}
