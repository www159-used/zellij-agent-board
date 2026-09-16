//! 退出过程中崩溃不算成功休眠，pane 与 supervisor 仍在。
use e2e_zellij::{process_exists, MockAgentOptions, Zellij};
use serial_test::serial;

#[test]
#[serial]
fn crash_does_not_count_as_sleep() {
    let mut zellij = Zellij::start().expect("zellij environment");
    let agent = zellij
        .start_mock_agent(MockAgentOptions::default().with_exit_behavior("crash"))
        .expect("start mock agent");
    let before = agent.snapshot().expect("before snapshot");

    let failed = agent.sleep(&mut zellij).expect("sleep result");
    assert_eq!(failed.state, "failed", "{failed:?}");
    assert_eq!(
        failed.error.as_deref(),
        Some("unexpected_agent_exit"),
        "{failed:?}"
    );
    assert!(failed.pid.is_none(), "{failed:?}");
    assert!(!process_exists(before.pid));
    assert!(!failed.clean_exit, "{failed:?}");
    assert_eq!(failed.supervisor_pid, before.supervisor_pid);
    assert!(process_exists(Some(before.supervisor_pid)));
    assert!(agent.pane_exists(&zellij));
}
