//! Deterministic Claude Code stand-in for daemon-managed sleep tests.
//!
//! Speaks enough of the Claude CLI surface for the daemon's sleep/resume path:
//! starts interactive (default) or `claude -r <uuid>`, publishes
//! `$CLAUDE_CONFIG_DIR/sessions/<pid>.json`, and exits cleanly on `/exit`.
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use uuid::Uuid;

/// Put stdin in the same shape a real Claude TUI uses: no canonical line
/// buffering, no echo, and crucially no `ICRNL` so a carriage return arrives as
/// a raw `\r` rather than being rewritten to `\n`. This is what lets the E2E
/// tell a correct `/exit\r` injection apart from a broken `/exit\n` one; a
/// cooked terminal would fold both to a newline and hide the bug.
struct RawMode {
    fd: RawFd,
    saved: libc::termios,
}

impl RawMode {
    fn enable() -> Option<Self> {
        let fd = io::stdin().as_raw_fd();
        unsafe {
            let mut term: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut term) != 0 {
                return None;
            }
            let saved = term;
            term.c_iflag &= !(libc::ICRNL | libc::INLCR | libc::IGNCR);
            term.c_lflag &= !(libc::ICANON | libc::ECHO);
            term.c_cc[libc::VMIN] = 1;
            term.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(fd, libc::TCSANOW, &term) != 0 {
                return None;
            }
            Some(RawMode { fd, saved })
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
        }
    }
}

fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    dirs_home()
        .map(|home| home.join(".claude"))
        .unwrap_or_else(|| PathBuf::from(".claude"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn persist(path: &Path, value: &Value) -> io::Result<()> {
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(file.as_file_mut(), value)?;
    file.as_file_mut().write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    File::open(parent)?.sync_all()
}

fn publish_session(config: &Path, session_id: &str, status: &str) -> io::Result<()> {
    let pid = process::id();
    let cwd = std::env::current_dir()?;
    let path = config.join("sessions").join(format!("{pid}.json"));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    persist(
        &path,
        &json!({
            "sessionId": session_id,
            "pid": pid,
            "cwd": cwd,
            "status": status,
            "peerFeatures": ["notify_idle"],
            "updatedAt": now,
        }),
    )
}

fn clear_session(config: &Path) {
    let path = config
        .join("sessions")
        .join(format!("{}.json", process::id()));
    let _ = fs::remove_file(path);
}

fn parse_args(args: &[String]) -> io::Result<Option<String>> {
    let mut resume = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-r" | "--resume" => {
                i += 1;
                let id = args
                    .get(i)
                    .cloned()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| io::Error::other("resume requires a session id"))?;
                resume = Some(id);
            }
            "-c" | "--continue" => {
                return Err(io::Error::other(
                    "fake-claude rejects --continue; use exact -r <uuid>",
                ));
            }
            "-h" | "--help" => {
                println!("fake-claude [-r SESSION_ID]");
                process::exit(0);
            }
            other if other.starts_with('-') => {
                return Err(io::Error::other(format!("unsupported flag {other}")));
            }
            _ => {}
        }
        i += 1;
    }
    Ok(resume)
}

fn run() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let resume = parse_args(&args)?;
    let config = config_dir();
    let session_id = match resume {
        Some(id) => {
            let transcript = config
                .join("projects")
                .join("_")
                .join(format!("{id}.jsonl"));
            if !transcript.is_file() {
                return Err(io::Error::other(format!("unknown session {id}")));
            }
            id
        }
        None => {
            let id = Uuid::new_v4().to_string();
            let project = config.join("projects").join("_");
            fs::create_dir_all(&project)?;
            fs::write(project.join(format!("{id}.jsonl")), b"{}\n")?;
            id
        }
    };
    publish_session(&config, &session_id, "idle")?;
    println!("fake-claude ready session_id={session_id}");
    let _ = io::stdout().flush();
    // A real Enter is a carriage return; in raw mode only `\r` submits. When
    // stdin is not a tty (raw mode unavailable) fall back to accepting `\n` too
    // so piped drivers still work.
    let raw = RawMode::enable();
    let submit_on_lf = raw.is_none();
    let mut stdin = io::stdin().lock();
    let mut line: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if stdin.read(&mut byte)? == 0 {
            break;
        }
        let submit = byte[0] == b'\r' || (submit_on_lf && byte[0] == b'\n');
        if byte[0] == 0x03 {
            // Ctrl+C: the daemon clears a draft before typing `/exit`.
            line.clear();
            continue;
        }
        if !submit {
            // Ignore a stray `\n` in raw mode; buffer everything else.
            if byte[0] != b'\n' {
                line.push(byte[0]);
            }
            continue;
        }
        let text = String::from_utf8_lossy(&line).trim().to_string();
        line.clear();
        if text.is_empty() {
            continue;
        }
        if text == "/exit" || text == "exit" {
            clear_session(&config);
            return Ok(());
        }
        if text.starts_with('{') {
            // Ignore stray control JSON that leaked onto the input stream.
            continue;
        }
        publish_session(&config, &session_id, "busy")?;
        println!("fake-claude echo: {text}");
        publish_session(&config, &session_id, "idle")?;
        let _ = io::stdout().flush();
    }
    clear_session(&config);
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fake-claude: {error}");
        process::exit(1);
    }
}
