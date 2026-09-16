# Context Map

## Contexts

- [Board](./CONTEXT.md) — live coding-agent panes and the floating dashboard
- [Scene E2E](./crates/e2e-scenes/README.md) — in-process Board replay fixtures
- [Zellij E2E](./crates/e2e-zellij/README.md) — real attached-PTY lifecycle scenarios

## Relationships

- **Scene E2E → Board**: scenes exercise `Board` and paint through `run_scene`
- **Zellij E2E → Board**: scenarios exercise the board and mock-agent lifecycle
  through a real Zellij client; existence still comes only from Scan
- **Board ↛ E2E crates**: the dashboard does not know about the test harnesses
