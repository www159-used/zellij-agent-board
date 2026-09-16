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
./scripts/e2e-zellij.sh -- --test sleep_then_resume
# or:
cargo build -p zellij-agent-board --bin agent-supervisor
cargo build -p mock-agent
cargo test -p e2e-zellij
```

`ZAB_E2E_ZELLIJ` selects the Zellij binary. Failure artifacts land under
`target/e2e-zellij/` when a case records them.

## Current coverage

- `sleep_then_resume`: idle mock sleeps (process gone, pane/supervisor kept),
  then resumes the exact conversation id and messages.
