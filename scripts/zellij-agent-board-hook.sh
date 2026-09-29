#!/usr/bin/env bash
# Catalog adapter hook → zellij-agent-board. Always exit 0. Only report inside a Zellij pane.
set -u
trap 'rm -f "${spool_tmp:-}" "${started_tmp:-}"; exit 0' EXIT

event="${1:-}"
payload=$(cat || true)

# Codex 0.157+ runs hooks from the app-server daemon. That process keeps the
# Zellij ids from wherever it was born (often daily/4), not the TUI pane that
# owns the turn. Never trust ZELLIJ_* that only a daemon holds. Resolve the
# pane from payload cwd / session_id→rollout / codex TUI match. Leave
# hooks.json alone — editing the command string invalidates trusted_hash.
if command -v python3 >/dev/null 2>&1; then
  resolved=$(
    ENV_SESSION="${ZELLIJ_SESSION_NAME:-}" \
    ENV_PANE="${ZELLIJ_PANE_ID:-}" \
    python3 -c '
import glob, json, os, shutil, subprocess, sys

def emit_nothing():
    raise SystemExit(0)

def run(argv):
    return subprocess.run(argv, capture_output=True, text=True, errors="replace")

def real(path):
    try:
        return os.path.realpath(path) if path else ""
    except Exception:
        return ""

def is_daemon(args):
    return (
        "app-server" in args
        or "code-mode-host" in args
        or "pid-update-loop" in args
    )

SHELLS = {"zsh", "bash", "sh", "fish", "nu", "dash", "login"}
AGENT_BINS = {
    "codex", "agent", "claude", "codebuddy", "opencode", "pi", "reasonix", "node",
}

def is_agentish(args):
    if not args or is_daemon(args):
        return False
    base = os.path.basename(args.split()[0])
    if base in SHELLS:
        return False
    return base in AGENT_BINS

def proc_cwds(pid, args):
    out = []
    found = run([lsof, "-a", "-p", pid, "-d", "cwd", "-Fn"])
    for item in found.stdout.splitlines():
        if item.startswith("n"):
            path = real(item[1:])
            if path:
                out.append(path)
    parts = args.split()
    for index, part in enumerate(parts):
        if part in ("-C", "--cd") and index + 1 < len(parts):
            path = real(parts[index + 1])
            if path:
                out.append(path)
    return out

def zellij_ids(pid):
    env = run([ps, "eww", "-p", pid])
    if env.returncode != 0:
        return None
    session = pane = ""
    for token in env.stdout.split():
        if token.startswith("ZELLIJ_SESSION_NAME="):
            session = token.split("=", 1)[1]
        elif token.startswith("ZELLIJ_PANE_ID="):
            pane = token.split("=", 1)[1]
    if not session or not pane:
        return None
    if "/" in session or "/" in pane or not pane.isdigit():
        return None
    return session, pane

def cwd_from_session(session_id):
    if not session_id or "/" in session_id or ".." in session_id:
        return ""
    pattern = os.path.join(
        os.path.expanduser("~"), ".codex", "sessions", "**", f"*{session_id}*.jsonl"
    )
    for path in glob.glob(pattern, recursive=True):
        try:
            with open(path, errors="replace") as handle:
                for line in handle:
                    try:
                        obj = json.loads(line)
                    except Exception:
                        continue
                    if obj.get("type") != "session_meta":
                        continue
                    payload = obj.get("payload")
                    if isinstance(payload, dict):
                        return str(payload.get("cwd") or "")
                    break
        except Exception:
            continue
    return ""

def emit(session, pane):
    print(session)
    print(pane)
    raise SystemExit(0)

raw = sys.stdin.read()
env_session = os.environ.get("ENV_SESSION", "")
env_pane = os.environ.get("ENV_PANE", "")
try:
    data = json.loads(raw) if raw.strip() else {}
except Exception:
    data = {}
if not isinstance(data, dict):
    data = {}
cwd = str(
    data.get("cwd")
    or data.get("CWD")
    or data.get("working_directory")
    or data.get("workdir")
    or ""
)
session_id = str(data.get("session_id") or data.get("sessionId") or "")
# No Codex identity in the payload: keep inherited Zellij ids (Cursor/Claude
# / tests). Codex daemon hooks carry cwd or session_id and must not.
if not cwd and not session_id:
    if env_session and env_pane:
        emit(env_session, env_pane)
    emit_nothing()

wanted = real(cwd)
if not wanted and session_id:
    wanted = real(cwd_from_session(session_id))

ps = shutil.which("ps") or "/bin/ps"
lsof = shutil.which("lsof") or "/usr/sbin/lsof"
listed = run([ps, "-ax", "-o", "pid=,args="])
if listed.returncode != 0:
    emit_nothing()

codex_rows = []
live_env = False
env_matches_cwd = False
for line in listed.stdout.splitlines():
    line = line.strip()
    if not line:
        continue
    pid, _, args = line.partition(" ")
    args = args.strip()
    if not pid.isdigit() or not is_agentish(args):
        continue
    ids = zellij_ids(pid)
    if not ids:
        continue
    cwds = proc_cwds(pid, args)
    if ids == (env_session, env_pane):
        live_env = True
        if wanted and wanted in cwds:
            env_matches_cwd = True
    if os.path.basename(args.split()[0]) == "codex":
        codex_rows.append((args, ids, cwds))

# Cursor / Claude / a real Codex TUI already carry the right pane ids.
# A shell left in the daemon birth pane must not count.
if env_session and env_pane and live_env and (not wanted or env_matches_cwd):
    emit(env_session, env_pane)

if session_id:
    hits = [ids for args, ids, _cwds in codex_rows if session_id in args]
    pairs = set(hits)
    if len(pairs) == 1:
        session, pane = next(iter(pairs))
        emit(session, pane)

if wanted:
    hits = []
    for args, ids, cwds in codex_rows:
        if wanted not in cwds:
            continue
        hits.append((session_id and session_id in args, ids))
    if hits:
        tagged = [ids for tag, ids in hits if tag]
        chosen = tagged if tagged else [ids for _tag, ids in hits]
        pairs = set(chosen)
        if len(pairs) == 1:
            session, pane = next(iter(pairs))
            emit(session, pane)

emit_nothing()
' <<<"$payload"
  ) || true
  if [ -n "$resolved" ]; then
    ZELLIJ_SESSION_NAME=$(printf '%s\n' "$resolved" | sed -n '1p')
    ZELLIJ_PANE_ID=$(printf '%s\n' "$resolved" | sed -n '2p')
  else
    case "$payload" in
      *\"cwd\"* | *\"session_id\"* | *\"sessionId\"*)
        # Codex identity present but no pane matched — drop daemon ids.
        ZELLIJ_SESSION_NAME=""
        ZELLIJ_PANE_ID=""
        ;;
    esac
  fi
fi

[ -n "${ZELLIJ_PANE_ID:-}" ] || exit 0
[ -n "${ZELLIJ_SESSION_NAME:-}" ] || exit 0

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
    event = str(data.get("hook_event_name") or data.get("event") or "")
tool = str(data.get("tool_name") or data.get("toolName") or "")
inp = data.get("tool_input") if isinstance(data.get("tool_input"), dict) else {}
args = data.get("toolArgs")
if isinstance(args, str):
    try:
        args = json.loads(args)
    except Exception:
        args = {}
if not inp and isinstance(args, dict):
    inp = args
cmd = str(inp.get("command") or data.get("command") or "")
path = str(inp.get("file_path") or inp.get("path") or "")
msg = str(data.get("agent_message") or data.get("prompt") or data.get("message") or "")
err = data.get("error")
if isinstance(err, dict):
    err_label = " ".join(
        str(err.get(key) or "") for key in ("name", "type", "message")
    )
    err = err_label.strip() or str(err.get("name") or "")
err = str(err or data.get("error_details") or "")
extra = cmd or path or msg or err
bits = [bit for bit in (tool, extra) if bit]
detail = " ".join(" ".join(bits).split())[:160]
st = data.get("status")
if isinstance(st, dict):
    status = str(st.get("type") or "")
else:
    status = str(st or "")
if not status and "abort" in err.lower():
    status = "aborted"
if not status and (data.get("isInterrupt") or data.get("is_interrupt")):
    status = "aborted"
note = str(data.get("notification_type") or data.get("notificationType") or "")
print(event.replace("\n", " "))
print(detail.replace("\n", " "))
print(status.replace("\n", " "))
print(note.replace("\n", " "))
' <<<"$payload" || true)
  event=$(printf '%s\n' "$parsed" | sed -n '1p')
  detail=$(printf '%s\n' "$parsed" | sed -n '2p')
  status=$(printf '%s\n' "$parsed" | sed -n '3p')
  note=$(printf '%s\n' "$parsed" | sed -n '4p')
else
  [ -n "$event" ] || event=$(printf '%s' "$payload" | sed -n 's/.*"hook_event_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
  status=$(printf '%s' "$payload" | sed -n 's/.*"status"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
  note=$(printf '%s' "$payload" | sed -n 's/.*"notification_type"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
  detail=""
fi

# Protocol-family event names → board events. Table is shipped next
# to this script (copied from adapters/catalog.toml at install).
map_file="$(dirname "$0")/event-map.txt"
if [ ! -f "$map_file" ]; then
  map_file="$(cd "$(dirname "$0")" && pwd)/event-map.txt"
fi
if [ -f "$map_file" ]; then
  mapped=$(awk -F= -v e="$event" '$1==e {print $2; exit}' "$map_file")
  [ -n "$mapped" ] && event=$mapped
fi

# Shared payload refine in this one hook — not a per-CLI script.
# Flat event-map.txt cannot see status / notification_type.
case "$event" in
  stop|afterAgentResponse|stopFailure)
    case "${status:-}" in
      aborted) event=interrupt ;;
      error) event=stopFailure ;;
    esac
    ;;
  Notification|notification)
    case "${note:-}" in
      permission_prompt) event=permissionRequest ;;
      idle_prompt) event=idleWait ;;
      *) exit 0 ;;
    esac
    ;;
  session.status)
    case "${status:-}" in
      busy) event=postToolUse ;;
      retry) event=permissionRequest ;;
      *) exit 0 ;;
    esac
    ;;
esac

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
mkdir -p "$spool_dir/.pending" || exit 0
spool_tmp=$(mktemp "$spool_dir/.pending/hook.XXXXXX") || exit 0
printf '%s\n' "$line" >"$spool_tmp" &&
  mv -f "$spool_tmp" "${spool_dir}/${ZELLIJ_SESSION_NAME}-${ZELLIJ_PANE_ID}" || exit 0
spool_tmp=""

# Live hook mail stays in TMPDIR. Titles / seen / started live in the
# host cache so they survive reboot and q.
if [ -n "${ZAB_STATE_DIR:-}" ]; then
  state_dir="$ZAB_STATE_DIR"
elif [ -n "${XDG_CACHE_HOME:-}" ]; then
  state_dir="$XDG_CACHE_HOME/zellij-agent-board"
elif [ -n "${HOME:-}" ]; then
  state_dir="$HOME/.cache/zellij-agent-board"
else
  state_dir="${TMPDIR:-/tmp}/zellij-agent-board"
fi

# Last spool line is often a tool hook. Persist the turn start separately
# so working elapsed survives q / Alt+q.
started="${state_dir}/started/${ZELLIJ_SESSION_NAME}-${ZELLIJ_PANE_ID}"
case "$event" in
  beforeSubmitPrompt)
    mkdir -p "${state_dir}/started/.pending" || exit 0
    started_tmp=$(mktemp "${state_dir}/started/.pending/start.XXXXXX") || exit 0
    printf 'STARTED %s %s %s\n' "${ZELLIJ_SESSION_NAME}" "${ZELLIJ_PANE_ID}" "${epoch}" >"$started_tmp" &&
      mv -f "$started_tmp" "$started" || exit 0
    started_tmp=""
    ;;
  stop|afterAgentResponse|sessionEnd|interrupt|stopFailure|idleWait)
    rm -f "$started"
    ;;
esac

# Unread done → OSC 9 on this pane's tty. Zellij forwards it to the host
# terminal. Only `stop` (not afterAgentResponse) so one done cycle rings once.
# Skip if this finished_at is already SEEN. Never `zellij pipe --plugin`.
if [ "$event" = "stop" ]; then
  seen="${state_dir}/seen/${ZELLIJ_SESSION_NAME}-${ZELLIJ_PANE_ID}"
  seen_at=""
  if [ -f "$seen" ]; then
    seen_at=$(awk '{print $4}' "$seen")
  fi
  if [ "$seen_at" != "$epoch" ]; then
    place=$(basename "${PWD:-}")
    [ -n "$place" ] || place="pane ${ZELLIJ_PANE_ID}"
    msg="${ZELLIJ_SESSION_NAME} ${place} done"
    if [ -e /dev/tty ]; then
      printf '\033]9;%s\007' "$msg" >/dev/tty 2>/dev/null || true
    else
      parent_tty=$(ps -p "${PPID:-0}" -o tty= 2>/dev/null | tr -d ' ')
      if [ -n "$parent_tty" ] && [ "$parent_tty" != "??" ] && [ -e "/dev/${parent_tty}" ]; then
        printf '\033]9;%s\007' "$msg" >"/dev/${parent_tty}" 2>/dev/null || true
      fi
    fi
  fi
fi
