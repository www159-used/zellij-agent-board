# Real Zellij E2E (`e2e-zellij`)

Workspace member under `crates/e2e-zellij`. One `tests/*.rs` file is one
case. Authoring types live here (`Zellij`, `MockAgent`, `Snapshot`).

Related crates:

- `crates/zellij-agent-board` — product, including `agent-supervisor`
- `crates/mock-agent` — deterministic agent CLI used during test init
- `crates/e2e-scenes/` — in-process Board replay fixtures (`cargo e2e`), not this suite

## Run

```bash
./scripts/e2e-zellij.sh
./scripts/e2e-zellij.sh --test sleep_then_resume
# or:
cargo build -p zellij-agent-board --bin board-tui --bin agent-supervisor
cargo build -p mock-agent
cargo test -p e2e-zellij
```

`ZAB_E2E_ZELLIJ` selects the Zellij binary. Failing cases keep isolates under
`target/e2e-zellij/`.

## Current coverage

- `sleep_then_resume`: idle mock sleeps (process gone, pane/supervisor kept),
  then resumes the exact conversation id and messages.
- `busy_agent_rejects_sleep` / `pending_confirmation_rejects_sleep` /
  `unsent_draft_rejects_sleep`: non-idle or draft state returns
  `not_sleepable` without killing the process.
- `ignored_exit_does_not_kill_agent`: exit ignore yields `sleep_timeout`,
  keeps the agent, and blocks duplicate resume with `agent_still_running`.
- `crash_does_not_count_as_sleep`: crash during exit is `failed` /
  `unexpected_agent_exit`, not a successful sleep.
