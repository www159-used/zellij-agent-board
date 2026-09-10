#!/usr/bin/env bash
# Bundle the platform TUI, portable WASM and hook installers together.
set -euo pipefail

if [[ $# -lt 4 || $# -gt 5 ]]; then
  echo "usage: package-release.sh VERSION TARGET WASM TUI [OUT_DIR]" >&2
  exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
version=$1
target=$2
wasm=$3
tui=$4
out_dir=${5:-"$root/target/packages"}
case "$version-$target" in
  *[!a-zA-Z0-9._-]*) echo "invalid version or target" >&2; exit 2 ;;
esac
mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

name="zellij-agent-board-$version-$target"
bundle="$work_dir/$name"
mkdir -p "$bundle/scripts/lib" "$bundle/adapters" "$bundle/docs/zh"
cp "$wasm" "$bundle/zellij-agent-board.wasm"
cp "$tui" "$bundle/board-tui"
chmod +x "$bundle/board-tui"
cp "$root/README.md" "$root/ROADMAP.md" "$root/LICENSE" "$bundle/"
cp "$root/docs/zh/README.md" "$bundle/docs/zh/"
cp "$root/adapters/catalog.toml" "$bundle/adapters/"
cp "$root/scripts/"{install.sh,install-hooks.sh,install-hooks.py,zellij-agent-board-hook.sh,event-map.txt,opencode-plugin.js} "$bundle/scripts/"
cp "$root/scripts/lib/"{__init__.py,catalog.py} "$bundle/scripts/lib/"
chmod +x "$bundle/scripts/"*.sh "$bundle/scripts/install-hooks.py"
COPYFILE_DISABLE=1 tar -czf "$out_dir/$name.tar.gz" -C "$work_dir" "$name"
(
  cd "$out_dir"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$name.tar.gz" >"$name.tar.gz.sha256"
  else
    shasum -a 256 "$name.tar.gz" >"$name.tar.gz.sha256"
  fi
)
echo "$out_dir/$name.tar.gz"
