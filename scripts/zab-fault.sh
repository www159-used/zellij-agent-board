#!/usr/bin/env bash
# Run Experiments with the artifacts built by make fault.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

zellij="${ZAB_FAULT_ZELLIJ:-$(command -v zellij || true)}"
if [[ -z "$zellij" || ! -x "$zellij" ]]; then
  echo "zab-fault: zellij is required" >&2
  exit 2
fi
export ZAB_FAULT_ZELLIJ="$zellij"

export PYTHONPATH="$root/e2e/zellij"
exec python3 -m harness "$@"
