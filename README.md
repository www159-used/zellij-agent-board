# zellij-agent-board

English · [中文](docs/zh/README.md)

Floating Zellij dashboard of running coding agents, with live status from Cursor hooks, and jump-to-pane.

> Framework only: module seams and a dismissable placeholder pane. Product: [docs/zh/PRODUCT.md](docs/zh/PRODUCT.md). Roadmap: [ROADMAP.md](ROADMAP.md).

This is not [zellij-agent](../zellij-agent/README.md) (the daily floating agent launcher).

## Install

Zellij 0.44 or newer is required.

```bash
./scripts/install.sh
./scripts/install-hooks.sh
```

Builds the WASM bridge and the host `board-tui`, then copies both to `~/.config/zellij/plugins/`. Override the WASM path with `ZELLIJ_AGENT_BOARD_PLUGIN_PATH`. The TUI path can be overridden with `ZELLIJ_AGENT_BOARD_TUI` or a `tui` plugin config key.

## Keybinding

Add to `keybinds` in `~/.config/zellij/config.kdl`. Use a `file:` URL.

```kdl
shared {
    bind "Alt q" {
        LaunchPlugin "file:~/.config/zellij/plugins/zellij-agent-board.wasm" {
            floating true
        }
    }
}
```

`Alt+q` opens the board; press again to close. Change it if it conflicts.

`skip_plugin_cache true` is only for developing the plugin. Leave it off day to day — each Alt+q otherwise reloads WASM from disk and the host occupancy climbs.

The WASM pane hides itself and opens `board-tui` in a command pane. Cursor hooks only write `$TMPDIR/zellij-agent-board-spool`; they must not `zellij pipe --plugin`. Jump is `zellij pipe --name zellij-agent-board -- JUMP <session> <pane>` to the already-running bridge.

## Develop

```bash
cargo fmt --check
cargo lint
cargo test --lib
cargo wasm
cargo build --release --bin board-tui
```
