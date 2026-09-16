//! Deterministic agent process for lifecycle tests; JSON lines in/out.
//!
//! Session files belong to this mock CLI, never to the board or its database.
//! Resume requires an exact id. EOF is a graceful exit; SIGKILL cannot emit exit.
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::Duration;

use fs2::FileExt;
use serde_json::{json, Value};
use uuid::Uuid;

fn persist(path: &Path, value: &Value) -> io::Result<()> {
    let parent = path.parent().unwrap();
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(file.as_file_mut(), value)?;
    file.as_file_mut().write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    File::open(parent)?.sync_all()
}

/// One session's event stream. Every line repeats the same identity and the
/// current status, so the emitter owns them instead of each call site.
struct Emitter {
    store: PathBuf,
    session_id: String,
    instance: String,
    status: String,
    draft: String,
}

impl Emitter {
    fn emit(&self, event: &str, extra: Value) -> io::Result<()> {
        let mut value = json!({
            "event": event,
            "session_id": self.session_id,
            "instance_id": self.instance,
            "pid": process::id(),
            "status": self.status,
            "draft": self.draft,
        });
        if let (Some(object), Some(map)) = (value.as_object_mut(), extra.as_object()) {
            for (key, item) in map {
                object.insert(key.clone(), item.clone());
            }
        }
        let line = serde_json::to_string(&value)?;
        let mut events = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.store.join("events.jsonl"))?;
        writeln!(events, "{line}")?;
        events.sync_all()?;
        println!("{line}");
        Ok(())
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: mock-agent --store DIR [--exit-mode normal|ignore|crash] [--exit-delay SECS] new|resume [SESSION_ID]"
    );
    process::exit(2);
}

fn main() {
    if let Err(error) = run() {
        eprintln!("mock-agent: {error}");
        process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut store = None;
    let mut exit_mode = "normal".to_owned();
    let mut exit_delay = 0.0_f64;
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--store" => store = Some(PathBuf::from(args.next().unwrap_or_else(|| usage()))),
            "--exit-mode" => exit_mode = args.next().unwrap_or_else(|| usage()),
            "--exit-delay" => {
                exit_delay = args
                    .next()
                    .unwrap_or_else(|| usage())
                    .parse()
                    .unwrap_or_else(|_| usage());
            }
            other => positional.push(other.to_owned()),
        }
    }
    let store = store.unwrap_or_else(|| usage());
    if exit_delay < 0.0 || !matches!(exit_mode.as_str(), "normal" | "ignore" | "crash") {
        usage();
    }
    let action = positional
        .first()
        .map(String::as_str)
        .unwrap_or_else(|| usage());
    let session_arg = positional.get(1).map(String::as_str);
    match (action, session_arg) {
        ("new", None) | ("resume", Some(_)) => {}
        _ => usage(),
    }
    let session_id = match session_arg {
        Some(id) => Uuid::parse_str(id).unwrap_or_else(|_| usage()).to_string(),
        None => Uuid::new_v4().to_string(),
    };
    fs::create_dir_all(&store)?;
    let path = store.join(format!("{session_id}.json"));
    let instance = Uuid::new_v4().to_string();
    let lock = OpenOptions::new()
        .create(true)
        .append(true)
        .open(store.join(format!("{session_id}.lock")))?;
    if lock.try_lock_exclusive().is_err() {
        eprintln!("session already running");
        process::exit(2);
    }
    let mut session = if action == "resume" {
        let loaded: Value = serde_json::from_slice(&fs::read(&path)?)?;
        if loaded["session_id"].as_str() != Some(session_id.as_str())
            || !loaded["messages"].is_array()
        {
            return Err(io::Error::other("cannot resume: invalid session"));
        }
        loaded
    } else {
        json!({"session_id": session_id, "messages": []})
    };
    persist(&path, &session)?;
    let mut emitter = Emitter {
        store,
        session_id,
        instance,
        status: "idle".to_owned(),
        draft: String::new(),
    };
    emitter.emit(
        "ready",
        json!({
            "resumed": action == "resume",
            "messages": session["messages"].clone(),
        }),
    )?;

    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let command: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                emitter.emit("error", json!({"error": error.to_string()}))?;
                continue;
            }
        };
        let op = command["op"].as_str().unwrap_or("");
        let outcome = handle_op(
            op,
            &command,
            &mut session,
            &mut emitter,
            &path,
            &exit_mode,
            exit_delay,
        );
        match outcome {
            OpResult::Emit(event) => emitter.emit(event, json!({}))?,
            OpResult::Error(error) => emitter.emit("error", json!({"error": error}))?,
            OpResult::Ignored => emitter.emit("exit_ignored", json!({}))?,
            OpResult::Exit => break,
            OpResult::Crash => process::exit(17),
        }
    }
    persist(&path, &session)?;
    emitter.emit("exited", json!({}))?;
    Ok(())
}

enum OpResult {
    Emit(&'static str),
    Error(String),
    Ignored,
    Exit,
    Crash,
}

fn handle_op(
    op: &str,
    command: &Value,
    session: &mut Value,
    emitter: &mut Emitter,
    path: &Path,
    exit_mode: &str,
    exit_delay: f64,
) -> OpResult {
    match op {
        "submit" => {
            if emitter.status != "idle" {
                return OpResult::Error("agent is not idle".into());
            }
            let Some(text) = command["text"].as_str() else {
                return OpResult::Error("text must be a string".into());
            };
            session["messages"]
                .as_array_mut()
                .unwrap()
                .push(json!({"role":"user","text":text}));
            if let Err(error) = persist(path, session) {
                return OpResult::Error(error.to_string());
            }
            emitter.draft.clear();
            emitter.status = "working".into();
            OpResult::Emit("submit")
        }
        "finish" => {
            if emitter.status != "working" {
                return OpResult::Error("agent is not working".into());
            }
            let Some(text) = command["text"].as_str() else {
                return OpResult::Error("text must be a string".into());
            };
            session["messages"]
                .as_array_mut()
                .unwrap()
                .push(json!({"role":"assistant","text":text}));
            if let Err(error) = persist(path, session) {
                return OpResult::Error(error.to_string());
            }
            emitter.status = "idle".into();
            OpResult::Emit("finish")
        }
        "permission" => {
            if emitter.status != "working" {
                return OpResult::Error("agent is not working".into());
            }
            emitter.status = "waiting".into();
            OpResult::Emit("permission")
        }
        "approve" => {
            if emitter.status != "waiting" {
                return OpResult::Error("no pending permission".into());
            }
            emitter.status = "working".into();
            OpResult::Emit("approve")
        }
        "draft" => {
            if emitter.status != "idle" {
                return OpResult::Error("draft requires idle agent and string text".into());
            }
            let Some(text) = command["text"].as_str() else {
                return OpResult::Error("draft requires idle agent and string text".into());
            };
            emitter.draft = text.to_owned();
            OpResult::Emit("draft")
        }
        "exit" => {
            if emitter.status != "idle" || !emitter.draft.is_empty() {
                return OpResult::Error("exit requires idle agent without a draft".into());
            }
            match exit_mode {
                "ignore" => OpResult::Ignored,
                "crash" => OpResult::Crash,
                _ => {
                    if exit_delay > 0.0 {
                        thread::sleep(Duration::from_secs_f64(exit_delay));
                    }
                    OpResult::Exit
                }
            }
        }
        "inspect" => OpResult::Emit("inspect"),
        other => OpResult::Error(format!("unknown operation: {other}")),
    }
}
