#!/usr/bin/env bash
# Exercise the install path from an extracted archive, without user config.
set -euo pipefail

archive=${1:?usage: test-release-package.sh ARCHIVE}
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT
tar -xzf "$archive" -C "$work_dir"
bundle="$work_dir/$(basename "$archive" .tar.gz)"
dest="$work_dir/plugins/zellij-agent-board.wasm"
ZELLIJ_AGENT_BOARD_PLUGIN_PATH="$dest" bash "$bundle/scripts/install.sh"
cmp "$bundle/zellij-agent-board.wasm" "$dest"
cmp "$bundle/board-tui" "$work_dir/plugins/board-tui"
"$work_dir/plugins/board-tui" --help
for resource in adapters/catalog.toml scripts/install-hooks.py scripts/install-hooks.sh \
  scripts/zellij-agent-board-hook.sh scripts/event-map.txt scripts/opencode-plugin.js \
  scripts/lib/catalog.py scripts/lib/__init__.py; do
  test -f "$bundle/$resource"
done
echo "release package: install and host executable OK"
