//! No-supervisor sleep/resume: the daemon owns the relationship table and
//! drives the pane. A plain shell hosts claude, so its exit keeps the pane;
//! the daemon injects `/exit` to sleep and exact `-r <sessionId>` to resume.
use e2e_zellij::{process_exists, Zellij};
use serial_test::serial;

#[test]
#[serial]
fn claude_daemon_sleep_then_resume() {
    let mut zellij = Zellij::start().expect("zellij environment");
    let agent = zellij
        .start_daemon_claude_agent()
        .expect("start daemon-managed claude");

    let before = agent.observe(&zellij).expect("before snapshot");
    assert_eq!(before.phase, "running", "{before:?}");
    assert_eq!(before.status, "idle", "{before:?}");
    assert!(!before.session_id.is_empty(), "{before:?}");
    assert_eq!(before.live_pids.len(), 1, "{before:?}");
    let before_pid = before.live_pids[0];
    assert!(process_exists(Some(before_pid)));

    // Sleep: the agent process leaves, the pane stays, the session id is kept.
    let sleeping = agent.sleep(&mut zellij).expect("sleep");
    assert_eq!(sleeping.phase, "sleeping", "{sleeping:?}");
    assert!(sleeping.live_pids.is_empty(), "{sleeping:?}");
    assert!(!process_exists(Some(before_pid)), "old process still alive");
    assert!(agent.pane_exists(&zellij), "pane closed on sleep");
    assert_eq!(
        sleeping.session_id, before.session_id,
        "{before:?} {sleeping:?}"
    );

    // Resume: exact -r in the same pane brings the same conversation back.
    let after = agent.resume(&mut zellij).expect("resume");
    assert_eq!(after.phase, "running", "{after:?}");
    assert_eq!(after.status, "idle", "{after:?}");
    assert!(agent.pane_exists(&zellij));
    assert_eq!(after.live_pids.len(), 1, "{after:?}");
    assert_ne!(after.live_pids[0], before_pid, "resume reused the old pid");
    assert_eq!(after.session_id, before.session_id, "{before:?} {after:?}");
}
