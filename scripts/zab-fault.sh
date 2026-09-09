#!/usr/bin/env bash
# Build board artifacts when a scenario needs them, then run Experiments.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

zellij="${ZAB_FAULT_ZELLIJ:-$(command -v zellij || true)}"
if [[ -z "$zellij" || ! -x "$zellij" ]]; then
  echo "zab-fault: zellij is required" >&2
  exit 2
fi
export ZAB_FAULT_ZELLIJ="$zellij"

need_board=1
if [[ $# -gt 0 && "$1" == "native-cross-session-switch" && $# -le 2 ]]; then
  need_board=0
fi
if [[ "$need_board" -eq 1 ]]; then
  cargo wasm
  cargo build --release --bin board-tui
fi

export PYTHONPATH="$root/e2e/zellij"
exec python3 -m harness "$@"
