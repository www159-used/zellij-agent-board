//! Manual lifecycle supervisor for the JSON-line mock adapter.
//! Deliberately not a Codex terminal adapter: no Codex exit protocol is assumed.
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde_json::{json, Value};

struct Agent {
    child: Child,
    input: ChildStdin,
    events: Receiver<Value>,
    reader: Option<thread::JoinHandle<()>>,
}
impl Drop for Agent {
    fn drop(&mut self) {
        // Owner termination only. Sleep never takes this path while the child lives.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
impl Agent {
    fn start(command: &[String], session: Option<&str>) -> io::Result<Self> {
        let mut process = Command::new(&command[0]);
        process.args(&command[1..]);
        match session {
            Some(id) => {
                process.args(["resume", id]);
            }
            None => {
                process.arg("new");
            }
        }
        let mut child = process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (send, events) = mpsc::channel();
        let reader = thread::spawn(move || read_lines(BufReader::new(output), send));
        Ok(Self {
            child,
            input,
            events,
            reader: Some(reader),
        })
    }
    fn send(&mut self, value: &Value) -> io::Result<()> {
        writeln!(self.input, "{value}")?;
        self.input.flush()
    }
}

fn read_lines(reader: impl BufRead, send: Sender<Value>) {
    for line in reader.lines() {
        let value = match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => serde_json::from_str(&line)
                .unwrap_or_else(|_| json!({"op":"invalid", "event":"invalid"})),
            Err(_) => break,
        };
        if send.send(value).is_err() {
            break;
        }
    }
}

fn save(path: &Path, value: &Value) -> io::Result<()> {
    // Same durable write pattern as e2e mock-agent `persist`: tempfile, fsync,
    // rename, then fsync the parent directory.
    let parent = path.parent().unwrap();
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    writeln!(file, "{value}")?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    File::open(parent)?.sync_all()
}

struct Supervisor {
    directory: PathBuf,
    command: Vec<String>,
    cwd: PathBuf,
    session: Option<String>,
    agent: Option<Agent>,
    phase: &'static str,
    last: Value,
    clean_exit: bool,
    sleep_requested: bool,
    deadline: Option<Instant>,
    timeout: Duration,
    sequence: u64,
}
impl Supervisor {
    fn publish(&mut self, error: Option<&str>) -> io::Result<()> {
        self.sequence += 1;
        let value = json!({"sequence":self.sequence, "state":self.phase,
            "session_id":self.session, "supervisor_pid":std::process::id(),
            "agent_pid":self.agent.as_ref().map(|agent| agent.child.id()),
            "agent":self.last, "error":error});
        save(&self.directory.join("status.json"), &value)?;
        println!("{value}");
        Ok(())
    }
    fn start(&mut self) -> io::Result<()> {
        self.agent = Some(Agent::start(&self.command, self.session.as_deref())?);
        self.phase = "starting";
        self.last = Value::Null;
        self.clean_exit = false;
        self.sleep_requested = false;
        self.deadline = None;
        self.publish(None)
    }
    fn observe(&mut self) -> io::Result<()> {
        let events: Vec<_> = self
            .agent
            .as_ref()
            .map(|agent| agent.events.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            if event["event"] == "ready" {
                let id = event["session_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| io::Error::other("agent ready event lacks session id"))?;
                if self
                    .session
                    .as_deref()
                    .is_some_and(|expected| expected != id)
                {
                    return Err(io::Error::other("agent resumed a different conversation"));
                }
                self.session = Some(id.to_owned());
                // Exact resume identity is durable before accepting a sleep command.
                save(
                    &self.directory.join("resume.json"),
                    &json!({"session_id":id,"command":self.command,"cwd":self.cwd}),
                )?;
                self.phase = "running";
            }
            if event["event"] == "exited" {
                self.clean_exit = true;
            }
            self.last = event;
            self.publish(None)?;
        }
        if let Some(status) = self
            .agent
            .as_mut()
            .map(|agent| agent.child.try_wait())
            .transpose()?
            .flatten()
        {
            // wait() can win the race against the stdout reader. Join and drain first.
            if let Some(reader) = self.agent.as_mut().unwrap().reader.take() {
                reader
                    .join()
                    .map_err(|_| io::Error::other("agent reader panicked"))?;
            }
            for event in self.agent.as_ref().unwrap().events.try_iter() {
                if event["event"] == "exited" {
                    self.clean_exit = true;
                }
                self.last = event;
            }
            self.agent = None;
            self.deadline = None;
            self.phase = if self.sleep_requested && status.success() && self.clean_exit {
                "sleeping"
            } else {
                "failed"
            };
            self.publish(if self.phase == "failed" {
                Some("unexpected_agent_exit")
            } else {
                None
            })?;
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.deadline = None;
            self.phase = "running";
            // Retain the process. A later exit is classified only after wait confirms it.
            self.publish(Some("sleep_timeout"))?;
        }
        Ok(())
    }
    fn command(&mut self, command: Value) -> io::Result<()> {
        match command["op"].as_str() {
            Some("sleep") => {
                if self.phase != "running"
                    || self.last["status"] != "idle"
                    || self.last["draft"] != ""
                    || self.session.is_none()
                {
                    return self.publish(Some("not_sleepable"));
                }
                if self
                    .agent
                    .as_mut()
                    .unwrap()
                    .send(&json!({"op":"exit"}))
                    .is_err()
                {
                    return self.publish(Some("agent_input_closed"));
                }
                self.sleep_requested = true;
                self.phase = "stopping";
                self.deadline = Some(Instant::now() + self.timeout);
                self.publish(None)
            }
            Some("resume") => {
                if self.agent.is_some() {
                    return self.publish(Some("agent_still_running"));
                }
                if self.session.is_none() {
                    return self.publish(Some("missing_session"));
                }
                if self.start().is_err() {
                    self.phase = "failed";
                    self.publish(Some("resume_start_failed"))
                } else {
                    Ok(())
                }
            }
            Some("inspect") => self.publish(None),
            Some("submit" | "finish" | "draft" | "permission" | "approve") => {
                if self.phase != "running" || self.sleep_requested {
                    return self.publish(Some("agent_unavailable"));
                }
                if self.agent.as_mut().unwrap().send(&command).is_err() {
                    self.publish(Some("agent_input_closed"))
                } else {
                    Ok(())
                }
            }
            _ => self.publish(Some("unknown_command")),
        }
    }
}

fn run() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--mock-agent") {
        return Err(io::Error::other(
            "usage: agent-supervisor --mock-agent STATE_DIR -- COMMAND [ARGS...]",
        ));
    }
    let directory = PathBuf::from(
        args.next()
            .ok_or_else(|| io::Error::other("missing state directory"))?,
    );
    if args.next().as_deref() != Some("--") {
        return Err(io::Error::other("expected -- before agent command"));
    }
    let command: Vec<_> = args.collect();
    if command.is_empty() {
        return Err(io::Error::other("missing agent command"));
    }
    fs::create_dir_all(&directory)?;
    let lock = File::create(directory.join("supervisor.lock"))?;
    lock.try_lock_exclusive()?;
    let session = match fs::read(directory.join("resume.json")) {
        Ok(bytes) => {
            let saved: Value = serde_json::from_slice(&bytes)?;
            if saved["command"] != json!(command) || saved["cwd"] != json!(std::env::current_dir()?)
            {
                return Err(io::Error::other("resume command changed"));
            }
            Some(
                saved["session_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| io::Error::other("invalid saved session"))?
                    .to_owned(),
            )
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut supervisor = Supervisor {
        directory,
        command,
        cwd: std::env::current_dir()?,
        session,
        agent: None,
        phase: "starting",
        last: Value::Null,
        clean_exit: false,
        sleep_requested: false,
        deadline: None,
        timeout: Duration::from_secs(2),
        sequence: 0,
    };
    supervisor.start()?;
    let (send, input) = mpsc::channel();
    thread::spawn(move || read_lines(io::stdin().lock(), send));
    loop {
        supervisor.observe()?;
        match input.recv_timeout(Duration::from_millis(20)) {
            Ok(command) => supervisor.command(command)?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("agent-supervisor: {error}");
        std::process::exit(1);
    }
}
