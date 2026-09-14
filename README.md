# zellij-agent-board

English · [中文](docs/zh/README.md)

Floating Zellij dashboard of running coding agents, with live hook status, search, and jump-to-pane across sessions.

Supports Cursor, CodeBuddy, Claude Code, OpenCode, Codex, and Reasonix through a declarative adapter catalog. Process scans create and remove Agents; hooks only update existing Agents. See [ROADMAP.md](ROADMAP.md) for follow-up work.

This project is the dashboard; the separate `zellij-agent` project provides a floating agent launcher.

## Install

Requires Zellij 0.44+, GNU Make 3.81+, and Python 3.11+ for the hook installer. Building from source also requires the Rust toolchain in `rust-toolchain.toml`.

The release workflow produces `.tar.gz` bundles for Linux x86_64/ARM64 (built on Ubuntu 24.04) and macOS Intel/Apple Silicon. Each bundle contains both the WASM bridge and its matching `board-tui`, plus the hook installers. Extract the bundle for your platform and run the commands below from its directory. Bundles do not require Rust. A standalone `.wasm` is only sufficient if a matching host TUI is already installed.

```bash
make install
make install-hooks
```

`make install` copies the bundled binaries, or builds both when run from a source checkout, into `~/.config/zellij/plugins/`. Override the WASM path with `ZELLIJ_AGENT_BOARD_PLUGIN_PATH`; the TUI is installed beside it. The runtime TUI path can be overridden with `ZELLIJ_AGENT_BOARD_TUI` or a `tui` plugin config key.

`make install-hooks` registers hooks from `adapters/catalog.toml` (Cursor, CodeBuddy, Claude Code, OpenCode, Codex, Reasonix). Use `make install-hooks ADAPTER=codex` to install one only; the default is `ADAPTER=all`. A new cc-family CLI is a drop-in TOML under `~/.config/zellij-agent-board/adapters/`.

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

Opening selects the Agent in the pane you opened the board from and scrolls it into view. If that pane has no Agent, selection returns to the most recent Agent jumped to through the board, including across sessions and board reopenings. If that Agent is gone or there is no history, selection starts at the first row. Keyboard navigation and mouse clicks or scrolling take over from automatic selection.

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

The picker supports mouse-wheel scrolling and a scrollbar on the right: click or drag its thumb to scroll. Keyboard behavior is unchanged.

## Runtime state

The TUI starts or connects to a local `board-tui --daemon`, then reads committed snapshots over HTTP on a private Unix socket. The daemon alone opens redb (4 MiB cache), serializes refreshes, and runs at most one scan worker. An open board requests refresh about every two seconds; closing the board leaves the daemon available but does not keep scanning. Scan and title updates commit together with immediate durability. Slow scans do not block snapshot requests.

`state.redb` lives in `$ZAB_STATE_DIR`, `$XDG_DATA_HOME/zellij-agent-board`, or `~/.local/share/zellij-agent-board`, in that order. On first initialization the daemon imports the old `scan`, `places`, and `places.host` files from the cache directory in one transaction. Original files remain for rollback and are never re-imported after initialization. Corrupt databases and unsupported schemas fail explicitly; they are not replaced with an empty store.

Focus and seen/started markers remain under `$ZAB_STATE_DIR`, `$XDG_CACHE_HOME/zellij-agent-board`, or `~/.cache/zellij-agent-board`. Launch focus is passed directly to each TUI. Hooks keep publishing their latest notice under `$TMPDIR/zellij-agent-board-spool` and maintaining the turn-start marker; they do not open the database. Unread completion can emit a terminal notification.

`board-tui --snapshot` prints the committed state as JSON; `GET /v1/snapshot` also works with `curl --unix-socket`. `board-tui --reconcile` requests a background refresh; success acknowledges scheduling, not completion. To restart after a binary upgrade, close open boards, run `board-tui --daemon-stop`, then open the board again. See [the storage design](docs/design/host-state.md) for ownership and recovery.

Codex `Interrupt` and Cursor `stop`/`afterAgentResponse` with `status=aborted` appear as `■ stopped`. Cursor `status=error`, Claude/CodeBuddy `StopFailure`, and OpenCode `session.error` appear as `✗ failed`. Both clear the working timer without marking the turn done or sending a completion notification. Permission prompts (`PermissionRequest`, CodeBuddy `Notification` `permission_prompt`, OpenCode `permission.asked`) appear as `● waiting` and keep the turn start so elapsed time resumes. Claude/CodeBuddy `Notification` `idle_prompt` appears as `◑ idle-wait` and clears the turn without a completion notice. After upgrading, rerun `make install-hooks` for the CLIs you use and restart those sessions. Notices from before the new hooks were installed cannot be recovered.

## Develop

```bash
make help
make check
make build
make e2e
make replay SCENE=e2e/scenes/slash-search-moves.scene
make e2e-zellij
make fault SCENARIO=board-launch-focus REPEAT=5
```

`make check` runs Rust formatting, Clippy, Rust tests (including board scenes), hook tests, and the Zellij harness unit tests. `make fmt` formats Rust. `make run`, `make stats`, `make scan`, `make reconcile`, and `make catalog` expose the host tools. Hook payloads can be replayed with `make hook EVENT=stop < payload.json`.

`e2e/scenes/` are host scenes: each input paints a frame at the declared size; `expect` checkpoints read that frame. `board-tui --replay` runs them without a TTY. `make e2e-zellij` starts a throwaway Zellij session, dumps the board footer chrome, and checks that `q` closes `board-tui`. A headless session cannot grant plugin permissions, so the TUI is started directly with `new-pane`; plugin loading is best effort. The script skips if `zellij` is absent; set `ZAB_E2E_ZELLIJ_REQUIRED=1` to fail instead. Permission granting, Alt+q toggling, and cross-session jumps still need interactive verification.

To package a release locally, build for the desired Rust target and then package the binaries:

```bash
make build TARGET=aarch64-apple-darwin
make package VERSION=v0.8.1 TARGET=aarch64-apple-darwin
make test-package VERSION=v0.8.1 TARGET=aarch64-apple-darwin
make package-wasm VERSION=v0.8.1
```

`make package` creates an archive and SHA-256 checksum from existing binaries. Override `WASM`, `TUI`, or `OUT_DIR` to use downloaded artifacts or another output directory. `make test-package ARCHIVE=path.tar.gz` extracts and installs a bundle in a temporary directory. `make package-wasm` writes a standalone WASM and checksum to `ASSET_DIR` (default `target/release-assets`). The release workflow runs this check on every platform before publishing.

## Usage stats

The host TUI records local usage facts as JSONL in `$XDG_DATA_HOME/zellij-agent-board/usage.jsonl` (default `~/.local/share/zellij-agent-board/usage.jsonl`). Nothing leaves the machine. Set `ZELLIJ_AGENT_BOARD_NO_STATS` to disable collection, or `ZELLIJ_AGENT_BOARD_STATS` to override the path.

```bash
board-tui --stats
```

Each opening gets a fresh random visit ID. Version 2 records mapped input actions, changed board snapshots, jump requests, pipe results, and normal/error exits. Typed text, titles, paths, and real session/pane identifiers are omitted; temporary row/session IDs only last for that visit. There is no persistent installation ID. Older v1 logs may contain session names; they are not rewritten.

The summary derives mode entries from state transitions and reports operation counts, pipe outcomes, and visits missing a close event. Pipe success does not confirm focus. Logs currently append without rotation. See [the collection and analysis design](docs/design/usage-analytics.md) for fields and limitations.

`make fault` runs repository-local lifecycle experiments through
a real PTY-attached client. It covers native switching, same-session board
focus, and the production cross-session jump path. See
[`e2e/zellij/README.md`](e2e/zellij/README.md) for scenarios, Holds, exact
Zellij version selection, and failure artifacts.
