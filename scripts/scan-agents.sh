#!/usr/bin/env bash
# Host scan for zellij-agent-board. Prints SCAN / HOOK / META lines on stdout.
# Existence comes from live processes with Zellij pane + session env.
set -u
trap 'exit 0' EXIT

spool_dir="${TMPDIR:-/tmp}/zellij-agent-board-spool"
hooks_json="${HOME}/.cursor/hooks.json"

epoch=$(date +%s)
if [[ -f "$hooks_json" ]] && grep -q 'zellij-agent-board-hook' "$hooks_json"; then
  printf 'META hooks=1 epoch=%s\n' "$epoch"
else
  printf 'META hooks=0 epoch=%s\n' "$epoch"
fi

env_blob() {
  local pid=$1
  if [[ -r "/proc/${pid}/environ" ]]; then
    tr '\0' ' ' <"/proc/${pid}/environ" || true
    return
  fi
  ps eww -p "$pid" -o command= 2>/dev/null || true
}

command_line() {
  local pid=$1
  ps -p "$pid" -ww -o args= 2>/dev/null | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' || true
}

is_cursor_agent() {
  local comm=$1
  [[ "$comm" == "agent" || "$comm" == "cursor-agent" ]]
}

# `agent ls` keeps argv=`ls` after you pick a session and chat. A pure picker
# has no chat store open; a live chat holds ~/.cursor/chats/.../store.db.
holding_chat_store() {
  local pid=$1
  lsof -p "$pid" 2>/dev/null | grep -q '/.cursor/chats/.*/store\.db'
}

live_keys=()
# macOS/BSD pgrep omits ancestors by default. Without -a, a scan started
# under an agent pane never sees that agent — the "current" session vanishes.
pids="$(pgrep -a -x agent || true)
$(pgrep -a -x cursor-agent || true)"

while read -r pid; do
  [[ -n "${pid:-}" ]] || continue
  comm=$(ps -p "$pid" -o comm= 2>/dev/null | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' || true)
  comm="${comm##*/}"
  is_cursor_agent "$comm" || continue

  args=$(command_line "$pid")
  [[ -n "$args" ]] || continue

  last=${args##* }
  if [[ "$last" == "ls" ]]; then
    holding_chat_store "$pid" || continue
  fi

  blob=$(env_blob "$pid")
  [[ "$blob" == *ZELLIJ_PANE_ID=* && "$blob" == *ZELLIJ_SESSION_NAME=* ]] || continue

  pane=${blob##*ZELLIJ_PANE_ID=}
  pane=${pane%%[!0-9]*}
  session=${blob##*ZELLIJ_SESSION_NAME=}
  session=${session%%[[:space:]]*}
  [[ -n "$pane" && -n "$session" ]] || continue

  printf 'SCAN %s %s %s %s\n' "$session" "$pane" "$comm" "$args"
  live_keys+=("${session}-${pane}")

  spool="${spool_dir}/${session}-${pane}"
  if [[ -f "$spool" ]]; then
    cat "$spool" || true
  fi
done <<<"$pids"

if [[ -d "$spool_dir" ]]; then
  shopt -s nullglob
  for spool in "$spool_dir"/*; do
    [[ -f "$spool" ]] || continue
    key=${spool##*/}
    keep=0
    for live in "${live_keys[@]+"${live_keys[@]}"}"; do
      if [[ "$live" == "$key" ]]; then
        keep=1
        break
      fi
    done
    if [[ "$keep" -eq 0 ]]; then
      rm -f "$spool"
    fi
  done
fi
