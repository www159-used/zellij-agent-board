#!/usr/bin/env bash
# Throwaway Zellij session: load the WASM (crash check) and a real board-tui
# pane, dump empty-board chrome, then send q and wait for the pane to go.
#
# board-tui is started from the layout. A headless session never answers the
# plugin Allow? prompt, so the bridge cannot new-pane the TUI itself.
# dump-layout also mentions the plugin's `tui ".../board-tui"` config — that
# is not proof a host pane exists; list-panes is.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if ! command -v zellij >/dev/null 2>&1; then
  if [[ "${ZAB_E2E_ZELLIJ_REQUIRED:-}" == 1 ]]; then
    echo "e2e-zellij: zellij is required" >&2
    exit 1
  fi
  echo "e2e-zellij: skip (zellij not on PATH)"
  exit 0
fi

echo "e2e-zellij: $(zellij --version)"

cargo wasm
cargo build --release --bin board-tui

session="zab-e2e-$$"
tmp="/tmp/$session"
mkdir -p "$tmp"
wasm="$root/target/wasm32-wasip1/release/zellij-agent-board.wasm"
tui="$root/target/release/board-tui"
cleanup() {
  zellij delete-session --force -- "$session" >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT

# Isolated from the developer's config, runtime dir, and enclosing session.
# Keep TMPDIR short — Zellij IPC sockets have a ~103 byte path cap.
# Do not rewrite HOME: attach --create-background then follows the wrong session.
unset ZELLIJ ZELLIJ_SESSION_NAME
export TMPDIR="$tmp"
export ZELLIJ_SOCKET_DIR="$tmp"
export ZELLIJ_AGENT_BOARD_TUI="$tui"

cat > "$tmp/config.kdl" <<'EOF'
keybinds clear-defaults=true {}
session_serialization false
show_startup_tips false
show_release_notes false
EOF

cat > "$tmp/layout.kdl" <<EOF
layout {
    pane command="sleep" {
        args "30"
    }
    floating_panes {
        pane command="$tui" name="board-tui" close_on_exit=true
        pane {
            plugin location="file:$wasm" {
                tui "$tui"
            }
        }
    }
}
EOF

cat > "$tmp/find_tui.py" <<'PY'
import json, sys

panes = json.load(sys.stdin)
for pane in panes:
    if pane.get("is_plugin"):
        continue
    blob = " ".join(
        str(pane.get(key) or "")
        for key in ("title", "pane_command", "terminal_command")
    )
    if "board-tui" not in blob:
        continue
    print("terminal_%s" % pane["id"])
    raise SystemExit(0)
raise SystemExit(1)
PY

find_tui() {
  zellij --session "$session" action list-panes --json --command --all 2>/dev/null \
    | python3 "$tmp/find_tui.py"
}

zellij --config "$tmp/config.kdl" --layout "$tmp/layout.kdl" attach --create-background "$session"

tui_pane=""
layout=""
for _ in $(seq 1 40); do
  layout="$(zellij --session "$session" action dump-layout 2>/dev/null || true)"
  if echo "$layout" | grep -Ei 'panic|plugin crashed'; then
    echo "e2e-zellij: plugin pane looks crashed" >&2
    echo "$layout" >&2
    exit 1
  fi
  if tui_pane="$(find_tui)"; then
    break
  fi
  tui_pane=""
  sleep 0.25
done

if [[ -z "$tui_pane" ]]; then
  echo "e2e-zellij: board-tui pane did not appear" >&2
  zellij --session "$session" action dump-layout 2>&1 || true
  zellij --session "$session" action list-panes --json --command --all 2>&1 || true
  exit 1
fi

if ! echo "$layout" | grep -q 'zellij-agent-board.wasm'; then
  echo "e2e-zellij: wasm plugin missing from dump-layout" >&2
  echo "$layout" >&2
  exit 1
fi

echo "e2e-zellij: board-tui loaded ($tui_pane)"

# Local machines may have live agents; CI usually does not. The footer
# `j/k` pill is on both an empty board and a full one.
screen=""
for _ in $(seq 1 20); do
  screen="$(
    zellij --session "$session" action dump-screen --pane-id "$tui_pane" 2>/dev/null || true
  )"
  if echo "$screen" | grep -q 'j/k'; then
    break
  fi
  sleep 0.25
done

if ! echo "$screen" | grep -q 'j/k'; then
  echo "e2e-zellij: dump-screen missing board footer chrome" >&2
  echo "$screen" >&2
  exit 1
fi

echo "e2e-zellij: board chrome ok"

zellij --session "$session" action write-chars --pane-id "$tui_pane" -- "q"

gone=0
for _ in $(seq 1 20); do
  if ! find_tui >/dev/null; then
    gone=1
    break
  fi
  sleep 0.25
done

if [[ "$gone" -ne 1 ]]; then
  echo "e2e-zellij: q did not close board-tui" >&2
  zellij --session "$session" action list-panes --json --command --all 2>&1 || true
  exit 1
fi

echo "e2e-zellij: q closed board-tui"
