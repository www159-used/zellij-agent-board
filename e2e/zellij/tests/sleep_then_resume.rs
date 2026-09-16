//! 休眠释放 agent 进程，保留 pane，恢复后仍是原会话和完整消息。
use e2e_zellij::{process_exists, MockAgentOptions, Zellij};
use serial_test::serial;

#[test]
#[serial]
fn sleep_then_resume() {
    let mut zellij = Zellij::start().expect("zellij environment");
    let agent = zellij
        .start_mock_agent(MockAgentOptions::idle_with_history([
            "remember this conversation",
        ]))
        .expect("start mock agent");
    let before = agent.snapshot().expect("before snapshot");

    let sleeping = agent.sleep(&mut zellij).expect("sleep");
    assert_eq!(sleeping.state, "sleeping", "{sleeping:?}");
    assert!(sleeping.error.is_none(), "{sleeping:?}");
    assert!(
        !process_exists(before.pid),
        "old process still alive: {:?}",
        before.pid
    );
    assert_eq!(sleeping.supervisor_pid, before.supervisor_pid);
    assert!(process_exists(Some(before.supervisor_pid)));
    assert!(agent.pane_exists(&zellij));

    let after = agent.resume(&mut zellij).expect("resume");
    assert_eq!(after.state, "running", "{after:?}");
    assert!(after.error.is_none(), "{after:?}");
    assert_ne!(after.pid, before.pid);
    assert_ne!(after.instance_id, before.instance_id);
    assert_eq!(after.supervisor_pid, before.supervisor_pid);
    assert!(agent.pane_exists(&zellij));
    assert!(after.resumed);
    assert_eq!(
        after.conversation_id, before.conversation_id,
        "{before:?} {after:?}"
    );
    assert_eq!(
        after.restored_messages, before.messages,
        "{before:?} {after:?}"
    );
    assert_eq!(after.messages, before.messages, "{before:?} {after:?}");
}
