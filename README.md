# agent-board

English · [中文](docs/zh/README.md)

Floating Zellij dashboard of running coding agents, with live status from Cursor hooks, and jump-to-pane.

> Framework only: module seams and a dismissable placeholder pane. Product: [docs/zh/PRODUCT.md](docs/zh/PRODUCT.md).

This is not [zellij-agent](../zellij-agent/README.md) (the `Alt+a` daily-agent launcher).

## Install

Zellij 0.44 or newer is required.

```bash
./scripts/install.sh
```

Builds the WASM and copies it to `~/.config/zellij/plugins/agent-board.wasm`. Override with `AGENT_BOARD_PLUGIN_PATH`. Hook install is not implemented yet.

## Keybinding

Add to `keybinds` in `~/.config/zellij/config.kdl`. Use a `file:` URL.

```kdl
shared {
    bind "Alt g" {
        LaunchOrFocusPlugin "file:~/.config/zellij/plugins/agent-board.wasm" {
            floating true
            move_to_focused_tab true
            skip_plugin_cache true
        }
    }
}
```

`Alt+g` opens the board. Change it if it conflicts.

## Develop

```bash
cargo fmt --check
cargo lint
cargo test --lib
cargo wasm
zellij -l zellij.kdl
```

## License

[GNU Affero General Public License v3.0 only](LICENSE).
