#!/usr/bin/env bash
# Host scan for zellij-agent-board. Prints SCAN / HOOK / META lines on stdout.
# Existence comes from live processes with Zellij pane + session env.
set -u
trap 'exit 0' EXIT

spool_dir="${TMPDIR:-/tmp}/zellij-agent-board-spool"

epoch=$(date +%s)
root="$(cd "$(dirname "$0")/.." && pwd)"
catalog_dump=$(python3 -B "$root/scripts/lib/catalog.py" dump)
bins=""
hook_files=""
while read -r kind rest; do
  case "$kind" in
    bin) bins="$bins $rest" ;;
    hook) hook_files="$hook_files $rest" ;;
  esac
done <<<"$catalog_dump"

chat_store_needle() {
  local comm=$1
  printf '%s\n' "$catalog_dump" | awk -v c="$comm" '$1=="chat_store" && $2==c {print $3; exit}'
}

hooks=0
for hooks_json in $hook_files; do
  if [[ -f "$hooks_json" ]] && grep -q 'zellij-agent-board-hook' "$hooks_json"; then
    hooks=1
    break
  fi
done
printf 'META hooks=%s epoch=%s\n' "$hooks" "$epoch"

is_catalog_bin() {
  local comm=$1
  local bin
  for bin in $bins; do
    [[ "$bin" == "$comm" ]] && return 0
  done
  return 1
}

# Cursor `agent ls` keeps argv=`ls` after you pick a session. A pure picker
# has no chat store open; a live chat holds the adapter's store needle.
holding_chat_store() {
  local pid=$1 needle=$2
  lsof -p "$pid" -Fn 2>/dev/null | grep -q "${needle}.*/store\\.db"
}

# Enumerate via ps, not pgrep: macOS pgrep silently skips some live
# processes (observed with a Zellij-launched `codebuddy`).
pids=""
while read -r pid comm; do
  [[ -n "${pid:-}" ]] || continue
  is_catalog_bin "${comm##*/}" || continue
  if [[ -n "$pids" ]]; then
    pids="$pids,$pid"
  else
    pids="$pid"
  fi
done < <(ps ax -ww -o pid= -o comm=)

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
    is_catalog_bin "$comm" || continue

    last=${args##* }
    needle=$(chat_store_needle "$comm")
    if [[ "$last" == "ls" && -n "$needle" ]]; then
      holding_chat_store "$pid" "$needle" || continue
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
