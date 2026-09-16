//! 忙碌中的 agent 不能休眠，进程与 pane 保持原样。
use e2e_zellij::{process_exists, MockAgentOptions, Zellij};
use serial_test::serial;

#[test]
#[serial]
fn busy_agent_rejects_sleep() {
    let mut zellij = Zellij::start().expect("zellij environment");
    let agent = zellij
        .start_mock_agent(MockAgentOptions::default().with_state("working"))
        .expect("start mock agent");
    let before = agent.snapshot().expect("before snapshot");
    assert_eq!(before.activity, "working", "{before:?}");

    let rejected = agent.sleep(&mut zellij).expect("sleep result");
    assert_eq!(
        rejected.error.as_deref(),
        Some("not_sleepable"),
        "{rejected:?}"
    );
    assert_eq!(rejected.state, "running", "{rejected:?}");
    assert_eq!(rejected.pid, before.pid);
    assert!(process_exists(before.pid));
    assert!(agent.pane_exists(&zellij));
}
