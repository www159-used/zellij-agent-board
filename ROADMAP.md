# Roadmap

Product vision and screenshots live in Obsidian: `learn/notes/ideas/zellij-agent.md`.

Execution is tracked in [GitHub Issues](https://github.com/www159-used/zellij-agent-board/issues). Close with `Fixes #N` in commits.

## Next — v0.5.0

| Issue | Topic |
|-------|--------|
| [#4](https://github.com/www159-used/zellij-agent-board/issues/4) | Verify working elapsed time survives board reopen |
| [#2](https://github.com/www159-used/zellij-agent-board/issues/2) | Search within visible viewport; jump to session first |
| [#1](https://github.com/www159-used/zellij-agent-board/issues/1) | Plan / mode-switch awaiting user choice |
| [#3](https://github.com/www159-used/zellij-agent-board/issues/3) | Notification backlog / tick consumption |
| [#5](https://github.com/www159-used/zellij-agent-board/issues/5) | v0.4.0 CHANGELOG and release notes |

Suggested order: **#4 → #2 → #1 → #3 → #5**.

## Later

Ideas only — open an issue when ready to build:

- Centered layout
- Overview integration (agent tab, legacy session → space)
- Product icon
- Usage telemetry (define metrics and privacy first)

## Won't (for now)

- SQLite or in-memory dual buffer — `places` + `places.host` merge is enough
- `skip_plugin_cache` on everyday Alt+q (overview Alt+y only)
- Changing WASM open path (empty bridge + `new-pane`) without explicit decision

## Released

- **v0.4.0** — Host TUI MVC: cache-first paint, background reconcile, `places.host`, scroll viewport, hook `started/` for working time
- **v0.3.x** — Relative time, unread done, notifications, bridge mode
- **v0.2.x** — Toggle, flash.nvim jump highlight
- **v0.1.x** — Overview-style prototype
