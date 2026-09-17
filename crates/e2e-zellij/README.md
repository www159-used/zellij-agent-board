# Real Zellij E2E (`e2e-zellij`)

Workspace member under `crates/e2e-zellij`. One `tests/*.rs` file is one
case. Authoring types live here (`Zellij`, `DaemonClaudeAgent`,
`DaemonSnapshot`).

Related crates:

- `crates/zellij-agent-board` — product (`board-tui`, which also hosts the
  daemon that owns the sleep/resume relationship table)
- `crates/mock-agent` — deterministic `fake-claude` CLI used during test init
- `crates/e2e-scenes/` — in-process Board replay fixtures (`cargo e2e`), not this suite

## Run

```bash
./scripts/e2e-zellij.sh
./scripts/e2e-zellij.sh --test claude_daemon_sleep_resume
# or:
cargo build -p zellij-agent-board --bin board-tui
cargo build -p mock-agent --bin fake-claude
cargo test -p e2e-zellij
```

`ZAB_E2E_ZELLIJ` selects the Zellij binary. Failing cases keep isolates under
`target/e2e-zellij/`.

## Current coverage

- `claude_daemon_sleep_then_resume`: no supervisor — the daemon owns the
  relationship table and drives a shell-hosted `claude` (fake-claude run as
  `claude`) through pane injection; sleep drops the process while keeping the
  pane, resume relaunches the exact `-r <sessionId>` with a new pid.
