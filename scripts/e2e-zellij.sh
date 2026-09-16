#!/usr/bin/env bash
# Build runtime artifacts, then run the Rust Zellij E2E crate.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if [[ "${1:-}" == "--list" ]]; then
  cargo test -p e2e-zellij -- --list
  exit 0
fi

zellij="${ZAB_E2E_ZELLIJ:-$(command -v zellij || true)}"
if [[ -z "$zellij" || ! -x "$zellij" ]]; then
  echo "e2e-zellij: zellij is required" >&2
  exit 2
fi
export ZAB_E2E_ZELLIJ="$zellij"

cargo build --bin agent-supervisor
exec cargo test -p e2e-zellij "$@"
