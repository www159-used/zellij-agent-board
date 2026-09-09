# Context Map

## Contexts

- [Board](./CONTEXT.md) — live coding-agent panes and the floating dashboard
- [Fault experiment](./e2e/zellij/CONTEXT.md) — reusable Zellij lifecycle experiments

## Relationships

- **Fault experiment → Board**: an Experiment may plant Agents and open
  the board; existence still comes only from Scan
- **Board ↛ Fault experiment**: the dashboard does not know about
  Experiments, Holds, or the harness
