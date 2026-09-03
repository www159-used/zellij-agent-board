#!/usr/bin/env python3
"""Install zellij-agent-board-hook into each catalog adapter."""

from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path

from lib.catalog import event_map_text, expand_home, load_catalog, repo_root

HOOK_SRC = repo_root() / "scripts" / "zellij-agent-board-hook.sh"
MAP_SRC = repo_root() / "scripts" / "event-map.txt"


def copy_hook(dest_dir: Path, map_text: str) -> Path:
    dest_dir.mkdir(parents=True, exist_ok=True)
    dest = dest_dir / "zellij-agent-board-hook.sh"
    shutil.copy(HOOK_SRC, dest)
    dest.chmod(0o755)
    (dest_dir / "event-map.txt").write_text(map_text)
    legacy = dest_dir / "agent-board-hook.sh"
    if legacy.exists():
        legacy.unlink()
    return dest


def write_json(path: Path, data) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2) + "\n")
    tmp.replace(path)


def install_cursor(adapter: dict, protocols: dict, command: str) -> None:
    path = expand_home(adapter["settings"])
    events = protocols[adapter["protocol"]]["events"]
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
        wanted_cmd = f"{command} {event}"
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
        if any(isinstance(item, dict) and item.get("command") == wanted_cmd for item in entries):
            hooks[event] = entries
            continue
        entries.append({"command": wanted_cmd})
        hooks[event] = entries
        changed = True
    if changed or not path.exists():
        write_json(path, data)
    print(f"installed {command}")
    print(f"hooks: {path}")


def install_cc(adapter: dict, protocols: dict, command: str) -> None:
    path = expand_home(adapter["settings"])
    events = protocols[adapter["protocol"]]["events"]
    if path.exists():
        data = json.loads(path.read_text())
    else:
        data = {}
    if not isinstance(data, dict):
        raise SystemExit("settings.json is not an object")
    hooks = data.setdefault("hooks", {})
    if not isinstance(hooks, dict):
        raise SystemExit("settings.json hooks is not an object")

    def entry_commands(entries):
        cmds = []
        for item in entries:
            if not isinstance(item, dict):
                continue
            for hook in item.get("hooks") or []:
                if isinstance(hook, dict) and isinstance(hook.get("command"), str):
                    cmds.append(hook["command"])
        return cmds

    changed = False
    for event in events:
        entries = hooks.get(event) or []
        if not isinstance(entries, list):
            raise SystemExit(f"settings.json hooks.{event} is not a list")
        wanted_cmd = f"{command} {event}"
        if wanted_cmd in entry_commands(entries):
            continue
        entries.append({"matcher": "*", "hooks": [{"type": "command", "command": wanted_cmd}]})
        hooks[event] = entries
        changed = True
    if changed or not path.exists():
        write_json(path, data)
    print(f"installed {command}")
    print(f"hooks: {path}")


def install_opencode(adapter: dict, protocols: dict, dest_dir: Path) -> None:
    proto = protocols[adapter["protocol"]]
    plugin_rel = proto.get("plugin")
    if not plugin_rel:
        raise SystemExit("opencode protocol is missing plugin")
    dest = dest_dir / "zellij-agent-board.js"
    shutil.copy(repo_root() / plugin_rel, dest)
    print(f"installed {dest}")
    print(f"plugin: {dest}")


def main(argv: list[str]) -> None:
    target = argv[1] if len(argv) > 1 else "all"
    catalog = load_catalog()
    adapters = catalog["adapters"]
    protocols = catalog["protocols"]
    ids = [item["id"] for item in adapters]
    if target not in ("all", *ids):
        names = "|".join(["all", *ids])
        raise SystemExit(f"usage: install-hooks.py [{names}]")

    wanted = adapters if target == "all" else [item for item in adapters if item["id"] == target]
    map_text = event_map_text(catalog) or MAP_SRC.read_text()

    for adapter in wanted:
        proto = protocols.get(adapter["protocol"])
        if not proto:
            raise SystemExit(f"unknown protocol {adapter['protocol']} for {adapter['id']}")
        dest_dir = expand_home(adapter["hook_dir"])
        command = str(copy_hook(dest_dir, map_text))
        kind = proto.get("install")
        if kind == "cursor":
            install_cursor(adapter, protocols, command)
        elif kind == "cc":
            install_cc(adapter, protocols, command)
        elif kind == "opencode":
            install_opencode(adapter, protocols, dest_dir)
        else:
            raise SystemExit(f"unknown install {kind} for {adapter['id']}")


if __name__ == "__main__":
    main(sys.argv)
