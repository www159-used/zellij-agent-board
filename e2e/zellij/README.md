# Real Zellij E2E (`e2e-zellij`)

Workspace member for attached-PTY Zellij scenarios. One `tests/*.rs` file is
one case. Authoring types live in this crate (`Zellij`, `MockAgent`,
`Snapshot`).

`e2e/scenes/` is separate: in-process Board replay fixtures for the main
package (`board-tui --replay` / `cargo e2e`). Do not merge the two suites.

## Run

```bash
./scripts/e2e-zellij.sh
./scripts/e2e-zellij.sh -- --test sleep_then_resume
# or:
cargo build --bin agent-supervisor
cargo test -p e2e-zellij
```

`ZAB_E2E_ZELLIJ` selects the Zellij binary. Failure artifacts land under
`target/e2e-zellij/` when a case records them.

## Current coverage

- `sleep_then_resume`: idle mock sleeps (process gone, pane/supervisor kept),
  then resumes the exact conversation id and messages.
