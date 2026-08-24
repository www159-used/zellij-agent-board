#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="${AGENT_BOARD_PLUGIN_PATH:-$HOME/.config/zellij/plugins/agent-board.wasm}"

cd "$root"
cargo wasm
mkdir -p "$(dirname "$dest")"
cp "$root/target/wasm32-wasip1/release/agent-board.wasm" "$dest"
scan_dest="$(dirname "$dest")/agent-board-scan.sh"
cp "$root/scripts/scan-agents.sh" "$scan_dest"
chmod +x "$scan_dest" "$root/scripts/scan-agents.sh" "$root/scripts/agent-board-hook.sh" "$root/scripts/install-hooks.sh"
echo "installed $dest"
echo "scan $scan_dest"
echo "hooks: ./scripts/install-hooks.sh"
