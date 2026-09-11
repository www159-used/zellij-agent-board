# zellij-agent-board

English · [中文](docs/zh/README.md)

Floating Zellij dashboard of running coding agents, with live hook status, search, and jump-to-pane across sessions.

Supports Cursor, CodeBuddy, Claude Code, OpenCode, Codex, and Reasonix through a declarative adapter catalog. Process scans create and remove Agents; hooks only update existing Agents. See [ROADMAP.md](ROADMAP.md) for follow-up work.

This project is the dashboard; the separate `zellij-agent` project provides a floating agent launcher.

## Install

Requires Zellij 0.44+ and Python 3.11+ for the hook installer. Building from source also requires the Rust toolchain in `rust-toolchain.toml`.

The release workflow produces `.tar.gz` bundles for Linux x86_64/ARM64 (built on Ubuntu 24.04) and macOS Intel/Apple Silicon. Each bundle contains both the WASM bridge and its matching `board-tui`, plus the hook installers. Extract the bundle for your platform and run the commands below from its directory. Bundles do not require Rust. A standalone `.wasm` is only sufficient if a matching host TUI is already installed.

```bash
./scripts/install.sh
./scripts/install-hooks.sh
```

`install.sh` copies the bundled binaries, or builds both when run from a source checkout, into `~/.config/zellij/plugins/`. Override the WASM path with `ZELLIJ_AGENT_BOARD_PLUGIN_PATH`; the TUI is installed beside it. The runtime TUI path can be overridden with `ZELLIJ_AGENT_BOARD_TUI` or a `tui` plugin config key.

`install-hooks.sh` registers hooks from `adapters/catalog.toml` (Cursor, CodeBuddy, Claude Code, OpenCode, Codex, Reasonix). Pass an adapter id to install one only; the default is all. A new cc-family CLI is a drop-in TOML under `~/.config/zellij-agent-board/adapters/`.

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

The WASM pane hides itself and opens `board-tui` with `new-pane --floating --close-on-exit`. Jump is `zellij pipe --name zellij-agent-board -- JUMP <session> <pane>` to the already-running bridge.

| Key | Action |
| --- | --- |
| `j/k`, arrows, `gg/G` | Move through Agents |
| `Ctrl+d/u`, `Ctrl+f/b` | Half-page or page movement |
| `/`, then `n/N` | Incremental search, next/previous match |
| `s` | Flash labels within the visible list |
| `p`, then `Tab` | Open the full-list picker, switch between query and labels |
| `Enter`, mouse click | Jump to the selected Agent |
| `?` | Show help |
| `Esc`, `q` | Dismiss the board; `Esc` first cancels an active overlay, while `q` remains input during search |

## Runtime state

The TUI first loads the cached scan, then requests a background reconcile about every two seconds. Each TUI keeps one child at a time and reaps it before starting another; a process lock permits only one reconciler across boards. Reconcile publishes scan/title snapshots with atomic file replacement. Missing home-session titles are filled in memory before the first paint.

Titles, the last scan, focus, and seen/started markers live in `$ZAB_STATE_DIR`, `$XDG_CACHE_HOME/zellij-agent-board`, or `~/.cache/zellij-agent-board`, in that order. Hooks publish their latest notice under `$TMPDIR/zellij-agent-board-spool` and separately maintain the turn-start marker. Hooks never launch a WASM plugin. Unread completion can emit a terminal notification.

Codex `Interrupt` and Cursor `stop`/`afterAgentResponse` with `status=aborted` appear as `■ stopped`. Cursor `status=error`, Claude/CodeBuddy `StopFailure`, and OpenCode `session.error` appear as `✗ failed`. Both clear the working timer without marking the turn done or sending a completion notification. Permission prompts (`PermissionRequest`, CodeBuddy `Notification` `permission_prompt`, OpenCode `permission.asked`) appear as `● waiting` and keep the turn start so elapsed time resumes. Claude/CodeBuddy `Notification` `idle_prompt` appears as `◑ idle-wait` and clears the turn without a completion notice. After upgrading, rerun `./scripts/install-hooks.sh` for the CLIs you use and restart those sessions. Notices from before the new hooks were installed cannot be recovered.

## Develop

```bash
cargo fmt --check
cargo lint
cargo test --locked --lib --bin board-tui
python3 scripts/test-hooks.py
cargo e2e
cargo run --bin board-tui -- --replay e2e/scenes/slash-search-moves.scene
./scripts/e2e-zellij.sh
./scripts/zab-fault.sh board-cross-session-jump 20
cargo wasm
cargo build --release --bin board-tui
```

`e2e/scenes/` are host scenes: each input paints a frame at the declared size; `expect` checkpoints read that frame. `board-tui --replay` runs them without a TTY. `./scripts/e2e-zellij.sh` starts a throwaway Zellij session, dumps the board footer chrome, and checks that `q` closes `board-tui`. A headless session cannot grant plugin permissions, so the TUI is started directly with `new-pane`; plugin loading is best effort. The script skips if `zellij` is absent; set `ZAB_E2E_ZELLIJ_REQUIRED=1` to fail instead. Permission granting, Alt+q toggling, and cross-session jumps still need interactive verification.

`scripts/package-release.sh VERSION TARGET WASM TUI [OUT_DIR]` creates a platform bundle and SHA-256 checksum. Run it with `bash`, then run `bash scripts/test-release-package.sh ARCHIVE` to extract and install the archive in a temporary directory. The release workflow runs this check on every platform before publishing.

## Usage stats

The host TUI records local usage facts as JSONL in `$XDG_DATA_HOME/zellij-agent-board/usage.jsonl` (default `~/.local/share/zellij-agent-board/usage.jsonl`). Nothing leaves the machine. Set `ZELLIJ_AGENT_BOARD_NO_STATS` to disable collection, or `ZELLIJ_AGENT_BOARD_STATS` to override the path.

```bash
board-tui --stats
```

Each opening gets a fresh random visit ID. Version 2 records mapped input actions, changed board snapshots, jump requests, pipe results, and normal/error exits. Typed text, titles, paths, and real session/pane identifiers are omitted; temporary row/session IDs only last for that visit. There is no persistent installation ID. Older v1 logs may contain session names; they are not rewritten.

The summary derives mode entries from state transitions and reports operation counts, pipe outcomes, and visits missing a close event. Pipe success does not confirm focus. Logs currently append without rotation. See [the collection and analysis design](docs/design/usage-analytics.md) for fields and limitations.

`./scripts/zab-fault.sh` runs repository-local lifecycle experiments through
a real PTY-attached client. It covers native switching, same-session board
focus, and the production cross-session jump path. See
[`e2e/zellij/README.md`](e2e/zellij/README.md) for scenarios, Holds, exact
Zellij version selection, and failure artifacts.
