//! Local C/S state owner. Clients never open redb; one daemon owns it.
//! Slow Zellij scans run on one worker, leaving snapshot requests responsive.
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::database::{HostDatabase, Snapshot};
use crate::protocol::{ensure_state, runtime_dir};
use crate::reconcile::{refresh_sessions, sessions_from_scan, try_acquire_lock};
use crate::scan::{scan_host_text, scan_places_for};
use crate::store::write_snapshot;

type ScanResult = (String, Vec<(crate::AgentId, crate::PanePlace)>);

pub fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("ZAB_STATE_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(path).join("zellij-agent-board");
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into()))
        .join(".local/share/zellij-agent-board")
}

pub enum Request {
    Snapshot,
    Refresh { home: String },
    Shutdown,
}

fn endpoint(dir: &Path) -> PathBuf {
    dir.join("daemon.endpoint")
}

pub fn request_at(dir: &Path, request: Request) -> io::Result<Option<Snapshot>> {
    crate::daemon_http::request_at(dir, request)
}

pub fn snapshot() -> io::Result<Snapshot> {
    request_at(&data_dir(), Request::Snapshot)?.ok_or_else(|| io::Error::other("missing snapshot"))
}

pub fn refresh(home: String) -> io::Result<()> {
    request_at(&data_dir(), Request::Refresh { home }).map(|_| ())
}

pub fn shutdown() -> io::Result<()> {
    request_at(&data_dir(), Request::Shutdown).map(|_| ())
}

/// Race-safe auto-start. A waiter reaps candidates while the launcher lives;
/// a surviving daemon is adopted by the OS when its launching TUI exits.
pub fn ensure_running(exe: &Path) -> io::Result<()> {
    if snapshot().is_ok() {
        return Ok(());
    }
    let mut command = Command::new(exe);
    command
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    let mut child = command.spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match snapshot() {
            Ok(_) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

pub fn serve() -> io::Result<()> {
    ensure_state();
    serve_at(&data_dir(), &runtime_dir())
}

pub fn serve_at(dir: &Path, legacy: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let _lock = try_acquire_lock(&dir.join("daemon.lock"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "daemon already owns state"))?;
    let db = HostDatabase::open(&dir.join("state.redb"), legacy)?;
    // A private short directory avoids Unix socket path limits for long worktrees.
    let socket_dir = tempfile::Builder::new()
        .prefix("zabd-")
        .tempdir_in("/tmp")?;
    fs::set_permissions(socket_dir.path(), fs::Permissions::from_mode(0o700))?;
    let socket = socket_dir.path().join("s");
    let http = crate::daemon_http::Server::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    write_snapshot(&endpoint(dir), socket.as_os_str().as_encoded_bytes())?;
    let mut worker: Option<thread::JoinHandle<ScanResult>> = None;
    let mut last_refresh: Option<Instant> = None;
    let result = (|| -> io::Result<()> {
        loop {
            if worker.as_ref().is_some_and(|job| job.is_finished()) {
                let (scan, places) = worker
                    .take()
                    .unwrap()
                    .join()
                    .map_err(|_| io::Error::other("scan worker panicked"))?;
                db.publish(&scan, places)?;
            }
            match http.incoming.recv_timeout(Duration::from_millis(20)) {
                Ok(call) => {
                    // A disconnected client must not leave queued side effects.
                    if call.reply.is_closed() {
                        continue;
                    }
                    let mut stop = false;
                    let response = match call.request {
                        Request::Snapshot => db.snapshot().map(Some),
                        Request::Refresh { home } => {
                            if worker.is_none()
                                && last_refresh
                                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(2))
                            {
                                last_refresh = Some(Instant::now());
                                worker = Some(thread::spawn(move || {
                                    let scan = scan_host_text();
                                    let mut places = Vec::new();
                                    for session in
                                        refresh_sessions(&sessions_from_scan(&scan), &home)
                                    {
                                        places.extend(scan_places_for(&[session]));
                                    }
                                    (scan, places)
                                }));
                            }
                            Ok(None)
                        }
                        Request::Shutdown => {
                            stop = true;
                            Ok(None)
                        }
                    };
                    let _ = call.reply.send(response);
                    if stop {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("HTTP server stopped"))
                }
            }
        }
        Ok(())
    })();
    drop(http);
    if let Some(worker) = worker {
        if let Ok((scan, places)) = worker.join() {
            if result.is_ok() {
                db.publish(&scan, places)?;
            }
        }
    }
    let _ = fs::remove_file(endpoint(dir));
    result
}
