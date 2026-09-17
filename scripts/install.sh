#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="${ZELLIJ_AGENT_BOARD_PLUGIN_PATH:-$HOME/.config/zellij/plugins/zellij-agent-board.wasm}"

cd "$root"
if [[ -f "$root/zellij-agent-board.wasm" && -f "$root/board-tui" ]]; then
  wasm_src="$root/zellij-agent-board.wasm"
  tui_src="$root/board-tui"
else
  wasm_src="${1:-$root/target/wasm32-wasip1/release/zellij-agent-board.wasm}"
  tui_src="${2:-$root/target/release/board-tui}"
fi
if [[ ! -f "$wasm_src" || ! -f "$tui_src" ]]; then
  echo 'missing binaries; run make install from the project directory' >&2
  exit 2
fi
mkdir -p "$(dirname "$dest")"
# `cp` over a running board-tui rewrites the same inode; new execs then
# get SIGKILL (even `--help`). Replace via temp + mv so leftovers in
# other sessions keep the old inode.
install_file() {
  local src=$1 dest=$2
  local tmp
  tmp="$(mktemp "${dest}.XXXXXX")"
  cp "$src" "$tmp"
  chmod +x "$tmp"
  mv -f "$tmp" "$dest"
}
install_file "$wasm_src" "$dest"
tui_dest="$(dirname "$dest")/board-tui"
install_file "$tui_src" "$tui_dest"
# A running daemon keeps the previous binary in memory until asked to exit, so
# a fresh install has no effect until it restarts. Stop it best-effort here; the
# next board open auto-spawns the just-installed version.
if "$tui_dest" --daemon-stop >/dev/null 2>&1; then
  echo "stopped running daemon; it restarts on next board open"
else
  echo "no running daemon to restart"
fi
chmod +x "$root/scripts/zellij-agent-board-hook.sh" "$root/scripts/install-hooks.sh" \
  "$root/scripts/install-hooks.py"
# Drop previous short names / scan helper if present.
rm -f "$(dirname "$dest")/agent-board.wasm" "$(dirname "$dest")/agent-board-scan.sh" "$(dirname "$dest")/zellij-agent-board-scan.sh"
echo "installed $dest"
echo "tui $tui_dest"
echo "hooks: make install-hooks"
