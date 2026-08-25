#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="${ZELLIJ_AGENT_BOARD_PLUGIN_PATH:-$HOME/.config/zellij/plugins/zellij-agent-board.wasm}"

cd "$root"
cargo wasm
cargo build --release --bin board-tui
mkdir -p "$(dirname "$dest")"
cp "$root/target/wasm32-wasip1/release/zellij-agent-board.wasm" "$dest"
tui_dest="$(dirname "$dest")/board-tui"
cp "$root/target/release/board-tui" "$tui_dest"
chmod +x "$tui_dest" "$root/scripts/zellij-agent-board-hook.sh" "$root/scripts/install-hooks.sh"
# Drop previous short names / scan helper if present.
rm -f "$(dirname "$dest")/agent-board.wasm" "$(dirname "$dest")/agent-board-scan.sh" "$(dirname "$dest")/zellij-agent-board-scan.sh"
echo "installed $dest"
echo "tui $tui_dest"
echo "hooks: ./scripts/install-hooks.sh"
