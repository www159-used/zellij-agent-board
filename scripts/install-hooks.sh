#!/usr/bin/env bash
# Thin wrapper — the installer is install-hooks.py.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 -B "$root/scripts/install-hooks.py" "$@"
