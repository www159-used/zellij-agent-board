//! 忽略退出请求时超时失败，不杀原进程，也不能重复 resume。
use e2e_zellij::{process_exists, MockAgentOptions, Zellij};
use serial_test::serial;

#[test]
#[serial]
fn ignored_exit_does_not_kill_agent() {
    let mut zellij = Zellij::start().expect("zellij environment");
    let agent = zellij
        .start_mock_agent(MockAgentOptions::default().with_exit_behavior("ignore"))
        .expect("start mock agent");
    let before = agent.snapshot().expect("before snapshot");

    let timed_out = agent.sleep(&mut zellij).expect("sleep result");
    assert_eq!(
        timed_out.error.as_deref(),
        Some("sleep_timeout"),
        "{timed_out:?}"
    );
    assert_eq!(timed_out.state, "running", "{timed_out:?}");
    assert_eq!(timed_out.pid, before.pid);
    assert!(
        process_exists(before.pid),
        "timeout must not kill the agent"
    );
    assert!(agent.pane_exists(&zellij));

    let rejected = agent.resume(&mut zellij).expect("resume result");
    assert_eq!(
        rejected.error.as_deref(),
        Some("agent_still_running"),
        "{rejected:?}"
    );
    assert_eq!(rejected.pid, before.pid);
    assert!(process_exists(before.pid));
}
