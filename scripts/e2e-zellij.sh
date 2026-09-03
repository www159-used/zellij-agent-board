#!/usr/bin/env bash
# Throwaway Zellij session: start board-tui with new-pane, dump empty-board
# chrome, then send q and wait for the pane to go.
#
# attach --create-background ignores --layout, so the TUI is opened with
# new-pane. A headless session never answers the plugin Allow? prompt.
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
export ZAB_STATE_DIR="$tmp/zab-state"
export ZELLIJ_SOCKET_DIR="$tmp"
export ZELLIJ_AGENT_BOARD_TUI="$tui"

cat > "$tmp/config.kdl" <<'EOF'
keybinds clear-defaults=true {}
session_serialization false
show_startup_tips false
show_release_notes false
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

# attach --create-background (and new-tab --layout on a headless session)
# drop custom layouts. Open the host TUI / WASM with new-pane instead.
zellij --config "$tmp/config.kdl" attach --create-background "$session"
ready=0
for _ in $(seq 1 40); do
  if zellij --session "$session" action list-tabs --json >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
if [[ "$ready" -ne 1 ]]; then
  echo "e2e-zellij: background session did not come up" >&2
  exit 1
fi
zellij --session "$session" action new-pane --name board-tui --close-on-exit -- "$tui" >/dev/null
zellij --session "$session" action new-pane --plugin "file:$wasm" --configuration "tui=$tui" >/dev/null || true

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

if echo "$layout" | grep -q 'zellij-agent-board.wasm'; then
  echo "e2e-zellij: wasm plugin loaded"
else
  echo "e2e-zellij: wasm plugin skipped (headless session never answers Allow?)"
fi

echo "e2e-zellij: board-tui loaded ($tui_pane)"
# dump-layout may mark the command pane start_suspended; wake it.
zellij --session "$session" action write --pane-id "$tui_pane" -- 13 >/dev/null 2>&1 || true

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
