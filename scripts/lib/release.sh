#!/usr/bin/env bash
# Shared release helpers. `package-release.sh` (platform bundles) and the
# Makefile's `package-wasm` target (standalone WASM) both publish artifacts
# from the same workflow run, so the accepted name charset and the platform
# hash fallback live here once.

# Reject a version or target that would break out of the artifact name.
require_safe_name() {
  case "$1" in
    ''|*[!a-zA-Z0-9._-]*) echo "invalid version or target: $1" >&2; return 2 ;;
  esac
}

# Print the "<hex>  <name>" line for "$1", GNU coreutils or BSD.
sha256_line() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1"
  else
    shasum -a 256 "$1"
  fi
}
