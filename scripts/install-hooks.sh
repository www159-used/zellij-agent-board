#!/usr/bin/env bash
# Merge zellij-agent-board-hook.sh into ~/.cursor/hooks.json (user-level, not per-repo).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/scripts/zellij-agent-board-hook.sh"
dest_dir="${HOME}/.cursor/hooks"
dest="$dest_dir/zellij-agent-board-hook.sh"
hooks_json="${HOME}/.cursor/hooks.json"

mkdir -p "$dest_dir"
cp "$src" "$dest"
chmod +x "$dest"
rm -f "$dest_dir/agent-board-hook.sh"

command="${dest}"
python3 - "$hooks_json" "$command" <<'PY'
import json, sys
from pathlib import Path

path = Path(sys.argv[1])
command = sys.argv[2]
events = [
    "sessionStart",
    "sessionEnd",
    "beforeSubmitPrompt",
    "preToolUse",
    "postToolUse",
    "postToolUseFailure",
    "beforeShellExecution",
    "afterShellExecution",
    "preCompact",
    "stop",
    "afterAgentResponse",
]

if path.exists():
    data = json.loads(path.read_text())
else:
    data = {"version": 1, "hooks": {}}

if not isinstance(data, dict):
    raise SystemExit("hooks.json is not an object")
data.setdefault("version", 1)
hooks = data.setdefault("hooks", {})
if not isinstance(hooks, dict):
    raise SystemExit("hooks.json hooks is not an object")

changed = False
for event in events:
    entries = hooks.get(event) or []
    if not isinstance(entries, list):
        raise SystemExit(f"hooks.json {event} is not a list")
    wanted = f"{command} {event}"
    # Drop legacy agent-board-hook entries for this event (not zellij-agent-board-hook).
    filtered = []
    for item in entries:
        if not isinstance(item, dict):
            filtered.append(item)
            continue
        cmd = item.get("command")
        if (
            isinstance(cmd, str)
            and "agent-board-hook.sh" in cmd
            and "zellij-agent-board-hook" not in cmd
        ):
            changed = True
            continue
        filtered.append(item)
    entries = filtered
    if any(isinstance(item, dict) and item.get("command") == wanted for item in entries):
        hooks[event] = entries
        continue
    entries.append({"command": wanted})
    hooks[event] = entries
    changed = True

if changed or not path.exists():
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(data, indent=2) + "\n")
    tmp.replace(path)

print(f"installed {command}")
print(f"hooks: {path}")
PY
