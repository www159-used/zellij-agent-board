# Board scene E2E (`e2e-scenes`)

In-process Board replay fixtures. Each `scenes/*.scene` file drives
`zellij_agent_board::run_scene`: inputs paint a real frame and `expect`
checks it. No TTY or Zellij process is required.

This is not the attached-PTY suite — that lives in `crates/e2e-zellij`.

## Run

```bash
cargo e2e
# or:
cargo test -p e2e-scenes
cargo run -p zellij-agent-board --bin board-tui -- --replay crates/e2e-scenes/scenes/slash-search-moves.scene
```
