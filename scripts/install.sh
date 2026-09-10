#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="${ZELLIJ_AGENT_BOARD_PLUGIN_PATH:-$HOME/.config/zellij/plugins/zellij-agent-board.wasm}"

cd "$root"
if [[ -f "$root/zellij-agent-board.wasm" && -f "$root/board-tui" ]]; then
  wasm_src="$root/zellij-agent-board.wasm"
  tui_src="$root/board-tui"
else
  cargo wasm
  cargo build --release --bin board-tui
  wasm_src="$root/target/wasm32-wasip1/release/zellij-agent-board.wasm"
  tui_src="$root/target/release/board-tui"
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
chmod +x "$root/scripts/zellij-agent-board-hook.sh" "$root/scripts/install-hooks.sh" \
  "$root/scripts/install-hooks.py"
# Drop previous short names / scan helper if present.
rm -f "$(dirname "$dest")/agent-board.wasm" "$(dirname "$dest")/agent-board-scan.sh" "$(dirname "$dest")/zellij-agent-board-scan.sh"
echo "installed $dest"
echo "tui $tui_dest"
echo "hooks: ./scripts/install-hooks.sh"
