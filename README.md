# zellij-agent-board

English · [中文](docs/zh/README.md)

Floating Zellij dashboard of running coding agents, with live hook status, search, and jump-to-pane across sessions.

Supports Cursor, CodeBuddy, Claude Code, OpenCode, Codex, Reasonix, and Pi through a declarative adapter catalog. Process scans create and remove Agents; hooks only update existing Agents. See [ROADMAP.md](ROADMAP.md) for follow-up work.

This project is the dashboard; the separate `zellij-agent` project provides a floating agent launcher.

## Install

Requires Zellij 0.44+, GNU Make 3.81+, and Python 3.11+ for the hook installer. Building from source also requires the Rust toolchain in `rust-toolchain.toml`.

The release workflow produces `.tar.gz` bundles for Linux x86_64/ARM64 (built on Ubuntu 24.04) and macOS Intel/Apple Silicon. Each bundle contains both the WASM bridge and its matching `board-tui`, plus the hook installers. Extract the bundle for your platform and run the commands below from its directory. Bundles do not require Rust. A standalone `.wasm` is only sufficient if a matching host TUI is already installed.

```bash
make install
make install-hooks
```

`make install` copies the bundled binaries, or builds both when run from a source checkout, into `~/.config/zellij/plugins/`. Override the WASM path with `ZELLIJ_AGENT_BOARD_PLUGIN_PATH`; the TUI is installed beside it. The runtime TUI path can be overridden with `ZELLIJ_AGENT_BOARD_TUI` or a `tui` plugin config key.

`make install-hooks` registers hooks from `adapters/catalog.toml` (Cursor, CodeBuddy, Claude Code, OpenCode, Codex, Reasonix, Pi). Use `make install-hooks ADAPTER=codex` to install one only; the default is `ADAPTER=all`. A new cc-family CLI is a drop-in TOML under `~/.config/zellij-agent-board/adapters/`.

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
| `Space f`, then a state key | Filter Agents by state; `w` working, `W` waiting, `d` done, `a` all |
| `Enter`, mouse click | Jump to the selected Agent |
| `?` | Show help |
| `S` | Open Settings from Home or Monitor |
| `h` | Switch to the Agents home page (default) |
| `t` | Switch to the full bottom monitor |
| `q` | Exit the entire GUI immediately, including search, picker and overlays |
| `Esc` | Cancel the current Home input mode or overlay; otherwise dismiss Home |
| `Ctrl+l` | Enter the privacy screensaver immediately |

Press `Space f` to see the state keys. The active filter appears in the header
and updates as hooks arrive. Movement, search, Flash labels and the picker use
the filtered Agents. `Space f a` restores all Agents; `Esc` cancels the menu.

The top navigation is always visible and clickable, with an active-page highlight
and pointer hover. There are two pages: `h` (Agents) and `t` (Monitor). `p` opens
Home's picker; it is not a third page. The Settings navigation button is clickable;
`S` or `Esc` closes Settings and returns to the previous page.

Settings contains a keyboard- and mouse-operated privacy form. Tab or Up/Down
moves between fields; Left/Right or Enter/Space changes a choice. Clicking an
option selects it directly (narrow panes show one value and cycle on click).
Changes apply immediately and are saved atomically to
`$XDG_CONFIG_HOME/zellij-agent-board/settings.json`, or
`~/.config/zellij-agent-board/settings.json`. `ZAB_SETTINGS_PATH` overrides the
file path. Load/save errors are shown in Settings; failed saves keep the change
active for the current GUI.

The Language field switches between English and 简体中文 immediately and saves
the choice for the next opening. Settings, navigation and the screensaver use
this language; Home's remaining chrome and embedded bottom retain their original
text for now. Existing settings files default to English, and unknown language
tags fall back to English without resetting the other preferences.

UI translations use `fluent-bundle` behind the shared `src/i18n.rs` interface.
Embedded `locales/en.ftl` and `locales/zh-CN.ftl` contain stable message keys and
named arguments. Renderers receive an `I18n` value instead of mutating a global
locale. Add messages to `Message` and the Fluent resources when extending a
page; add locales to `Locale` and the resource loader to extend the Language
field. Missing or invalid translations fall back to English. Keep shortcuts,
protocol values and serialized settings identifiers independent of translated
labels, and calculate layout and mouse hit areas using terminal cell widths.

By default, 30 seconds without input in ZAB activates a privacy screensaver.
Background updates and terminal resize do not reset the timer. Settings offers
Screensaver, Monitor, Close GUI or Disabled actions; 30s, 1min or 5min delays; and
Blank or Clock screensavers. The Monitor action switches to `t` once per idle
period. Closing the GUI exposes the terminal below it; Monitor can display
process names and command lines.

The screensaver hides the entire board including navigation. Space or left-clicking
anywhere inside the screensaver restores the previous page, selection and input; the wake
operation is consumed. Pointer motion and other keys do not wake it. `q` still
exits the GUI. Settings has a Preview screensaver button. This covers only the
ZAB pane and does not authenticate the user; use the system lock screen to
protect the whole desktop.

The monitor embeds the bottom 0.14.9 library in the native binary, so no separate
`btm` installation is required. It runs in a private PTY, preserving bottom's
CPU, memory, network, disks, process list, help, keyboard and mouse controls while
keeping the host navigation available. Switching pages preserves Agent selection,
scroll position and monitor history. Bottom uses your usual configuration; set
`ZAB_BOTTOM_CONFIG` to use a specific config file. A failed monitor shows an error
and can be restarted with `t` or its navigation button.

Bare `h`, `t`, `S` and `q` are reserved for navigation, Settings and exit on the Monitor page.
Use arrows for bottom's horizontal movement and `F5` for process tree mode.
Home's search, hint and picker inputs accept `h` and `t` as text; clicking the
navigation still switches pages. `q` always exits. Closing the GUI also kills
and reaps its monitor child.

The picker supports mouse-wheel scrolling and a scrollbar on the right: click or drag its thumb to scroll. Keyboard behavior is unchanged.

## Runtime state

The TUI first loads the cached scan, then requests a background reconcile about every two seconds. Each TUI keeps one child at a time and reaps it before starting another; a process lock permits only one reconciler across boards. Reconcile publishes scan/title snapshots with atomic file replacement. Missing home-session titles are filled in memory before the first paint.

Titles, the last scan, and seen/started markers live in `$ZAB_STATE_DIR`, `$XDG_CACHE_HOME/zellij-agent-board`, or `~/.cache/zellij-agent-board`, in that order. Launch focus is passed directly to each TUI. Hooks publish their latest notice under `$TMPDIR/zellij-agent-board-spool` and separately maintain the turn-start marker. Hooks never launch a WASM plugin. Unread completion can emit a terminal notification.

Codex `Interrupt`, Pi `agent_settled` with `aborted`, and Cursor `stop`/`afterAgentResponse` with `status=aborted` appear as `■ stopped`. Cursor `status=error`, Claude/CodeBuddy `StopFailure`, OpenCode `session.error`, and Pi `agent_settled` with `error` appear as `✗ failed`. Both clear the working timer without marking the turn done or sending a completion notification. Permission prompts (`PermissionRequest`, CodeBuddy `Notification` `permission_prompt`, OpenCode `permission.asked`) appear as `● waiting` and keep the turn start so elapsed time resumes. Claude/CodeBuddy `Notification` `idle_prompt` appears as `◑ idle-wait` and clears the turn without a completion notice. After upgrading, rerun `make install-hooks` for the CLIs you use and restart those sessions. Notices from before the new hooks were installed cannot be recovered.

Codex hooks cache a resolved thread-to-pane identity under `codex-panes/` in the state directory. Before reusing it, they check the live Codex PID, pane, working directory, and current thread title. This avoids enumerating every agent during the one-second `Interrupt` deadline. Title queries cover all tabs and prefer `~/.cargo/bin/zellij` over PATH, matching the host scanner's preference; `ZAB_ZELLIJ` overrides that choice.

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

`make check` runs Rust formatting, Clippy, Rust tests (including board scenes), hook tests, the Zellij harness unit tests, and terminal integration tests. `make fmt` formats Rust. `make run`, `make stats`, `make scan`, `make reconcile`, and `make catalog` expose the host tools. Hook payloads can be replayed with `make hook EVENT=stop < payload.json`.

`e2e/scenes/` are host scenes: each input paints a frame at the declared size; `expect` checkpoints read that frame. `board-tui --replay` runs them without a TTY. `make e2e-zellij` starts a throwaway Zellij session, dumps the board footer chrome, and checks that `q` closes `board-tui`. A headless session cannot grant plugin permissions, so the TUI is started directly with `new-pane`; plugin loading is best effort. The script skips if `zellij` is absent; set `ZAB_E2E_ZELLIJ_REQUIRED=1` to fail instead. Permission granting, Alt+q toggling, and cross-session jumps still need interactive verification.

`make test-deleted-cwd` attaches a PTY client to a throwaway session, removes its pane's working directory, and presses Alt+q to test the actual WASM bridge and native board. It checks rendering and `q` dismissal without changing user sessions or config. The WASM path must already have Zellij plugin permissions granted; use `WASM=/absolute/path/to/trusted/plugin.wasm` to test an installed plugin.

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

The summary derives mode entries (including Space menus and status filters) from state transitions, and reports operation counts, pipe outcomes, page/settings/privacy events, monitor lifecycle results, hook resolution results and measured durations. Idle exits and hook attempts missing a result are reported separately from normal dismissal and completed attempts. The hook resolver uses the same local sink and opt-out; it records no payload or real process/session identifiers. Pipe success does not confirm focus. Logs currently append without rotation. See [the collection and analysis design](docs/design/usage-analytics.md) for fields and limitations.

`make fault` runs repository-local lifecycle experiments through
a real PTY-attached client. It covers native switching, same-session board
focus, and the production cross-session jump path. See
[`e2e/zellij/README.md`](e2e/zellij/README.md) for scenarios, Holds, exact
Zellij version selection, and failure artifacts.
