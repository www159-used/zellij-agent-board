#!/usr/bin/env bash
# Ensure board artifacts exist, then run Experiments. Whether a scenario
# actually needs them is the harness's rule (`_needs_board`), not ours.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

zellij="${ZAB_FAULT_ZELLIJ:-$(command -v zellij || true)}"
if [[ -z "$zellij" || ! -x "$zellij" ]]; then
  echo "zab-fault: zellij is required" >&2
  exit 2
fi
export ZAB_FAULT_ZELLIJ="$zellij"

cargo wasm
cargo build --release --bin board-tui

export PYTHONPATH="$root/e2e/zellij"
exec python3 -m harness "$@"
