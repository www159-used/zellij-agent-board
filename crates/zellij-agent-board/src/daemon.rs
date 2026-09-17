//! Local C/S state owner. Clients never open redb; one daemon owns it.
//! Slow Zellij scans run on one worker, leaving snapshot requests responsive.
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::agent::{AgentId, PanePlace};
use crate::database::{HostDatabase, SleepScan, Snapshot};
use crate::protocol::{ensure_state, runtime_dir};
use crate::reconcile::{refresh_sessions, sessions_from_scan, try_acquire_lock};
use crate::scan::{scan_host_text, scan_places_for, scan_sleep_observations};
use crate::store::write_snapshot;

pub use crate::protocol::data_dir;

type ScanResult = (String, Vec<(AgentId, PanePlace)>, SleepScan);

/// A running scan worker is reaped from this tick; an idle daemon only wakes
/// for client traffic, so it can afford a long one.
const WORKER_POLL: Duration = Duration::from_millis(20);
const IDLE_POLL: Duration = Duration::from_secs(1);

/// Minimum spacing between accepted refreshes. Clients pace their own requests
/// from this too, so the two cannot drift apart.
pub const SCAN_EVERY: Duration = Duration::from_secs(2);

pub enum Request {
    Snapshot,
    Refresh { home: String },
    Sleep { id: AgentId },
    Resume { id: AgentId },
    Shutdown,
}

/// Published socket path clients read. The daemon removes it on clean shutdown.
pub fn endpoint(dir: &Path) -> PathBuf {
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

/// Ask the daemon to sleep the agent in `id`'s pane (process out, pane kept).
/// A rejection (busy/unknown/no record) surfaces as an error.
pub fn sleep(id: AgentId) -> io::Result<()> {
    request_at(&data_dir(), Request::Sleep { id }).map(|_| ())
}

/// Ask the daemon to resume the exact conversation in `id`'s pane.
pub fn resume(id: AgentId) -> io::Result<()> {
    request_at(&data_dir(), Request::Resume { id }).map(|_| ())
}

pub fn shutdown() -> io::Result<()> {
    request_at(&data_dir(), Request::Shutdown).map(|_| ())
}

/// Wait for a shutdown to unlink the published endpoint.
pub fn wait_until_stopped(timeout: Duration) -> io::Result<()> {
    let path = endpoint(&data_dir());
    let deadline = Instant::now() + timeout;
    while path.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::other("daemon shutdown timed out"));
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Race-safe auto-start. A waiter reaps candidates while the launcher lives;
/// a surviving daemon is adopted by the OS when its launching TUI exits.
pub fn ensure_running() -> io::Result<()> {
    if snapshot().is_ok() {
        return Ok(());
    }
    let mut command = Command::new(std::env::current_exe()?);
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
    let result = pump(&http, &db, &mut worker, &mut last_refresh);
    drop(http);
    // An accepted refresh still owes the caller a committed scan. A panicked
    // worker and a failing loop are already reported by `result`.
    if let Some(job) = worker.filter(|_| result.is_ok()) {
        let _ = commit(&db, job);
    }
    let _ = fs::remove_file(endpoint(dir));
    result
}

/// Serve until shutdown. Only a queued scan worker needs the short tick; with
/// no worker running, a client request is the only thing that can change state.
fn pump(
    http: &crate::daemon_http::Server,
    db: &HostDatabase,
    worker: &mut Option<thread::JoinHandle<ScanResult>>,
    last_refresh: &mut Option<Instant>,
) -> io::Result<()> {
    loop {
        if worker.as_ref().is_some_and(|job| job.is_finished()) {
            commit(db, worker.take().expect("finished worker was present"))?;
        }
        let tick = if worker.is_some() {
            WORKER_POLL
        } else {
            IDLE_POLL
        };
        match http.incoming.recv_timeout(tick) {
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
                            && last_refresh.is_none_or(|last| last.elapsed() >= SCAN_EVERY)
                        {
                            *last_refresh = Some(Instant::now());
                            // Sessions that hold sleep rows must be re-probed even
                            // with no live agent, so a closed pane can be pruned.
                            let record_sessions = record_sessions(db);
                            *worker = Some(thread::spawn(move || {
                                let scan = scan_host_text();
                                let mut places = Vec::new();
                                for session in refresh_sessions(&sessions_from_scan(&scan), &home) {
                                    places.extend(scan_places_for(&[session]));
                                }
                                let sleep = scan_sleep_observations(&record_sessions);
                                (scan, places, sleep)
                            }));
                        }
                        Ok(None)
                    }
                    Request::Sleep { id } => {
                        // A first `z` often arrives before the background scan
                        // has written the relationship row. Observe now so a
                        // live claude is sleepable instead of `no_record`.
                        prime_sleep_row(db, &id);
                        control(db, Op::Sleep, &id)
                    }
                    Request::Resume { id } => control(db, Op::Resume, &id),
                    Request::Shutdown => {
                        stop = true;
                        Ok(None)
                    }
                };
                let _ = call.reply.send(response);
                if stop {
                    return Ok(());
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("HTTP server stopped"))
            }
        }
    }
}

enum Op {
    Sleep,
    Resume,
}

/// Fold a just-in-time observation into the relationship table so sleep can
/// see a live claude that the last background scan has not committed yet.
fn prime_sleep_row(db: &HostDatabase, id: &AgentId) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let scan = scan_sleep_observations(std::slice::from_ref(&id.session));
    let _ = db.reconcile_sleep(&scan, now);
}

/// Run one control op on the database owner thread, injecting into the pane
/// via the real Zellij seam. A rejection becomes an error carrying its reason
/// (e.g. `not_sleepable`); acceptance acknowledges with no snapshot.
fn control(db: &HostDatabase, op: Op, id: &AgentId) -> io::Result<Option<Snapshot>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let inject = &crate::sleep::zellij_injector;
    let outcome = match op {
        Op::Sleep => crate::sleep::sleep(db, inject, now, id)?,
        Op::Resume => crate::sleep::resume(db, inject, now, id)?,
    };
    match outcome {
        crate::sleep::Outcome::Accepted => Ok(None),
        crate::sleep::Outcome::Rejected(reason) => Err(io::Error::other(reason)),
    }
}

/// Unique sessions that currently hold sleep rows; used to probe panes for
/// agents that have gone to sleep and no longer appear in a live scan.
fn record_sessions(db: &HostDatabase) -> Vec<String> {
    let mut sessions: Vec<String> = db
        .sleep_records()
        .unwrap_or_default()
        .into_iter()
        .map(|(id, _)| id.session)
        .collect();
    sessions.sort();
    sessions.dedup();
    sessions
}

fn commit(db: &HostDatabase, job: thread::JoinHandle<ScanResult>) -> io::Result<()> {
    let (scan, places, sleep) = job
        .join()
        .map_err(|_| io::Error::other("scan worker panicked"))?;
    db.publish(&scan, places)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    db.reconcile_sleep(&sleep, now)
}
