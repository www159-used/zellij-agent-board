//! Actual daemon process + Unix IPC + redb. No live user state is touched.
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use zellij_agent_board::daemon::{request_at, Request};

struct Server(Child);
impl Server {
    fn start(dir: &Path) -> Self {
        Self(
            Command::new(env!("CARGO_BIN_EXE_board-tui"))
                .arg("--daemon")
                .env("ZAB_STATE_DIR", dir)
                .env("TMPDIR", dir)
                .env("ZAB_SCAN_SESSION_PREFIX", "zab-test-no-live-session-")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "daemon condition timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn committed_state_survives_kill_and_restart_and_ignores_stale_migration_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("scan"),
        "META hooks=1\nSCAN stale 8 codex codex\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("places"),
        "PLACE stale 8 0\tmain\tSaved title\n",
    )
    .unwrap();
    let mut server = Server::start(dir.path());
    wait_until(|| request_at(dir.path(), Request::Snapshot).is_ok());
    let initial = request_at(dir.path(), Request::Snapshot).unwrap().unwrap();
    assert!(initial.scan.unwrap().contains("SCAN stale"));
    request_at(
        dir.path(),
        Request::Refresh {
            home: String::new(),
        },
    )
    .unwrap();
    wait_until(|| {
        request_at(dir.path(), Request::Snapshot)
            .unwrap()
            .unwrap()
            .revision
            > initial.revision
    });
    let committed = request_at(dir.path(), Request::Snapshot).unwrap().unwrap();
    assert!(!committed.scan.as_ref().unwrap().contains("SCAN stale"));
    assert!(committed.places.contains("Saved title"));
    let socket = std::fs::read_to_string(dir.path().join("daemon.endpoint")).unwrap();
    server.0.kill().unwrap();
    server.0.wait().unwrap();
    let _ = std::fs::remove_dir_all(Path::new(&socket).parent().unwrap());
    let mut restarted = Server::start(dir.path());
    wait_until(|| request_at(dir.path(), Request::Snapshot).is_ok());
    assert_eq!(
        request_at(dir.path(), Request::Snapshot).unwrap().unwrap(),
        committed
    );
    request_at(dir.path(), Request::Shutdown).unwrap();
    wait_until(|| restarted.0.try_wait().unwrap().is_some());
    assert!(!dir.path().join("daemon.endpoint").exists());
}

#[test]
fn competing_owner_and_malformed_client_do_not_disrupt_concurrent_readers() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Server::start(dir.path());
    wait_until(|| request_at(dir.path(), Request::Snapshot).is_ok());
    let expected = request_at(dir.path(), Request::Snapshot).unwrap().unwrap();
    let mut competitor = Server::start(dir.path());
    wait_until(|| competitor.0.try_wait().unwrap().is_some());
    assert!(!competitor.0.wait().unwrap().success());
    let socket = std::fs::read_to_string(dir.path().join("daemon.endpoint")).unwrap();
    let mut malformed = UnixStream::connect(socket).unwrap();
    malformed.write_all(b"NOT HTTP\r\n\r\n").unwrap();
    drop(malformed);
    thread::scope(|scope| {
        let jobs: Vec<_> = (0..6)
            .map(|_| scope.spawn(|| request_at(dir.path(), Request::Snapshot)))
            .collect();
        for job in jobs {
            let result = job.join().unwrap();
            assert!(
                result.is_ok(),
                "request={result:?}, server={:?}, log={:?}",
                server.0.try_wait(),
                std::fs::read_to_string(dir.path().join("board.log"))
            );
            assert_eq!(result.unwrap().unwrap(), expected);
        }
    });
    request_at(dir.path(), Request::Shutdown).unwrap();
    wait_until(|| !dir.path().join("daemon.endpoint").exists());
}

fn http(dir: &Path, request: &[u8]) -> String {
    use std::io::Read;
    let socket = std::fs::read_to_string(dir.join("daemon.endpoint")).unwrap();
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream.write_all(request).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn http_routes_reject_invalid_mutations_and_work_with_curl() {
    let dir = tempfile::tempdir().unwrap();
    let _server = Server::start(dir.path());
    wait_until(|| request_at(dir.path(), Request::Snapshot).is_ok());
    let cases = [
        ("GET /v1/shutdown HTTP/1.1\r\nHost: localhost\r\n\r\n", "405 Method Not Allowed"),
        ("GET /v2/snapshot HTTP/1.1\r\nHost: localhost\r\n\r\n", "404 Not Found"),
        ("POST /v1/refresh HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}", "415 Unsupported Media Type"),
        ("POST /v1/refresh HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}", "400 Bad Request"),
        ("POST /v1/shutdown HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}", "400 Bad Request"),
    ];
    for (request, expected) in cases {
        let response = http(dir.path(), request.as_bytes());
        assert!(
            response.starts_with(&format!("HTTP/1.1 {expected}")),
            "{response}"
        );
        assert!(response.contains("application/json"));
        if expected.starts_with("405") {
            assert!(response.to_ascii_lowercase().contains("allow: post"));
        }
    }
    let oversized = format!("POST /v1/refresh HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 65537\r\n\r\n{}", "x".repeat(65537));
    assert!(http(dir.path(), oversized.as_bytes()).starts_with("HTTP/1.1 413"));
    // Ordinary tooling uses the same routes, with no private framing library.
    let socket = std::fs::read_to_string(dir.path().join("daemon.endpoint")).unwrap();
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "3",
            "--noproxy",
            "*",
            "--unix-socket",
            &socket,
            "http://localhost/v1/snapshot",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot["revision"], 1);
    // Standard chunked transfer encoding is accepted by Hyper too.
    let response = http(dir.path(), b"POST /v1/refresh HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\nb\r\n{\"home\":\"\"}\r\n0\r\n\r\n");
    assert!(response.starts_with("HTTP/1.1 202 Accepted"), "{response}");
    request_at(dir.path(), Request::Shutdown).unwrap();
    wait_until(|| !dir.path().join("daemon.endpoint").exists());
}

#[test]
fn incomplete_http_headers_and_bodies_do_not_block_other_clients() {
    let dir = tempfile::tempdir().unwrap();
    let _server = Server::start(dir.path());
    wait_until(|| request_at(dir.path(), Request::Snapshot).is_ok());
    let socket = std::fs::read_to_string(dir.path().join("daemon.endpoint")).unwrap();
    let mut headers = UnixStream::connect(&socket).unwrap();
    headers
        .write_all(b"GET /v1/snapshot HTTP/1.1\r\nHost:")
        .unwrap();
    let mut body = UnixStream::connect(&socket).unwrap();
    body.write_all(b"POST /v1/refresh HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{").unwrap();
    // The regular client has a 500 ms deadline, shorter than the stalled
    // connection deadline. Its success demonstrates independent progress.
    assert!(request_at(dir.path(), Request::Snapshot).unwrap().is_some());
    drop(headers);
    drop(body);
    request_at(dir.path(), Request::Shutdown).unwrap();
    wait_until(|| !dir.path().join("daemon.endpoint").exists());
}
