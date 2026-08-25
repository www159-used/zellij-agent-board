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

is_cursor_agent() {
  local comm=$1
  [[ "$comm" == "agent" || "$comm" == "cursor-agent" ]]
}

# `agent ls` keeps argv=`ls` after you pick a session and chat. A pure picker
# has no chat store open; a live chat holds ~/.cursor/chats/.../store.db.
holding_chat_store() {
  local pid=$1
  lsof -p "$pid" -Fn 2>/dev/null | grep -q '/.cursor/chats/.*/store\.db'
}

pids=""
while read -r pid; do
  [[ -n "${pid:-}" ]] || continue
  if [[ -n "$pids" ]]; then
    pids="$pids,$pid"
  else
    pids="$pid"
  fi
done <<EOF
$(pgrep -a -x agent || true)
$(pgrep -a -x cursor-agent || true)
EOF

live_keys=()

if [[ -n "$pids" ]]; then
  # One eww dump for every pid. Per-pid `ps eww` was ~50ms each and made
  # the 1s timer overlap itself once 20 agents were live.
  env_text=$(ps eww -p "$pids" -ww -o pid= -o command= 2>/dev/null || true)
  # macOS `comm` is truncated (~16 chars), so the tool name comes from args.
  while read -r pid args; do
    [[ -n "${pid:-}" ]] || continue
    [[ -n "${args:-}" ]] || continue
    bin=${args%% *}
    comm=${bin##*/}
    is_cursor_agent "$comm" || continue

    last=${args##* }
    if [[ "$last" == "ls" ]]; then
      holding_chat_store "$pid" || continue
    fi

    blob=""
    while read -r epid rest; do
      [[ "$epid" == "$pid" ]] || continue
      blob=$rest
      break
    done <<< "$env_text"
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
  done <<EOF
$(ps -p "$pids" -ww -o pid= -o args= 2>/dev/null)
EOF
fi

# A failed match (0 live keys) must not wipe hook history. That is how
# an earlier comm-truncation bug turned every row back into "found".
if [[ -d "$spool_dir" && ${#live_keys[@]} -gt 0 ]]; then
  shopt -s nullglob
  for spool in "$spool_dir"/*; do
    [[ -f "$spool" ]] || continue
    key=${spool##*/}
    keep=0
    for live in "${live_keys[@]}"; do
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
