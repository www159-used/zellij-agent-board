#!/usr/bin/env bash
# Cursor hook → zellij-agent-board. Always exit 0. Only report inside a Zellij pane.
set -u
trap 'exit 0' EXIT

[ -n "${ZELLIJ_PANE_ID:-}" ] || exit 0
[ -n "${ZELLIJ_SESSION_NAME:-}" ] || exit 0

event="${1:-}"
payload=$(cat || true)

if command -v python3 >/dev/null 2>&1; then
  parsed=$(EVENT="$event" python3 -c '
import json, os, sys
raw = sys.stdin.read()
event = os.environ.get("EVENT", "")
try:
    data = json.loads(raw) if raw.strip() else {}
except Exception:
    data = {}
if not isinstance(data, dict):
    data = {}
if not event:
    event = str(data.get("hook_event_name") or "")
tool = str(data.get("tool_name") or "")
inp = data.get("tool_input") if isinstance(data.get("tool_input"), dict) else {}
cmd = str(inp.get("command") or data.get("command") or "")
path = str(inp.get("file_path") or inp.get("path") or "")
msg = str(data.get("agent_message") or "")
extra = cmd or path or msg
bits = [bit for bit in (tool, extra) if bit]
detail = " ".join(" ".join(bits).split())[:160]
print(event.replace("\n", " "))
print(detail.replace("\n", " "))
' <<<"$payload" || true)
  event=$(printf '%s\n' "$parsed" | sed -n '1p')
  detail=$(printf '%s\n' "$parsed" | sed -n '2p')
else
  [ -n "$event" ] || event=$(printf '%s' "$payload" | sed -n 's/.*"hook_event_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
  detail=""
fi

[ -n "$event" ] || exit 0

epoch=$(date +%s)
stamp=$(date '+%m-%dT%H:%M')
line="HOOK ${ZELLIJ_SESSION_NAME} ${ZELLIJ_PANE_ID} ${event} @${epoch} +${stamp}"
if [ -n "${detail:-}" ]; then
  line="${line} ${detail}"
fi

# Spool only. Never `zellij pipe --plugin` — that launches WASM on every
# Cursor hook and is what pushed the host session to hundreds of percent CPU.
spool_dir="${TMPDIR:-/tmp}/zellij-agent-board-spool"
mkdir -p "$spool_dir"
tmp="${spool_dir}/${ZELLIJ_SESSION_NAME}-${ZELLIJ_PANE_ID}.tmp.$$"
printf '%s\n' "$line" >"$tmp"
mv -f "$tmp" "${spool_dir}/${ZELLIJ_SESSION_NAME}-${ZELLIJ_PANE_ID}"
