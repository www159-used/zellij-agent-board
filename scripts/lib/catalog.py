"""Load the CLI catalog (builtin TOML + user drop-ins)."""

from __future__ import annotations

import os
import sys
import tomllib
from pathlib import Path


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def user_adapter_dir() -> Path:
    xdg = os.environ.get("XDG_CONFIG_HOME")
    if xdg:
        return Path(xdg) / "zellij-agent-board" / "adapters"
    return Path.home() / ".config" / "zellij-agent-board" / "adapters"


def expand_home(path: str, home: Path | None = None) -> Path:
    home = home or Path.home()
    if path == "~":
        return home
    if path.startswith("~/"):
        return home / path[2:]
    return Path(path)


def load_catalog() -> dict:
    data = tomllib.loads((repo_root() / "adapters" / "catalog.toml").read_text())
    defaults = list(data.get("defaults", {}).get("skip") or [])
    protocols = data.get("protocol") or {}
    adapters = []
    for item in data.get("adapter") or []:
        adapters.append(_normalize_adapter(item, defaults))
    user_dir = user_adapter_dir()
    if user_dir.is_dir():
        for path in sorted(user_dir.glob("*.toml")):
            text = path.read_text()
            extra = tomllib.loads(text)
            if extra.get("adapter") or extra.get("protocol"):
                for name, proto in (extra.get("protocol") or {}).items():
                    protocols[name] = proto
                for item in extra.get("adapter") or []:
                    _upsert(adapters, _normalize_adapter(item, defaults))
            elif extra.get("id"):
                _upsert(adapters, _normalize_adapter(extra, defaults))
    return {"adapters": adapters, "protocols": protocols}


def event_map_text(catalog: dict | None = None) -> str:
    catalog = catalog or load_catalog()
    lines = []
    for proto in (catalog.get("protocols") or {}).values():
        for src, dest in (proto.get("map") or {}).items():
            lines.append(f"{src}={dest}")
    return "".join(f"{line}\n" for line in lines)


def _normalize_adapter(item: dict, defaults: list[str]) -> dict:
    out = dict(item)
    if "skip" not in out:
        out["skip"] = list(defaults)
    if out.get("ls_holds_chat_store") and not out.get("chat_store_needle"):
        out["chat_store_needle"] = "/.cursor/chats/"
    return out


def _upsert(adapters: list[dict], item: dict) -> None:
    for index, existing in enumerate(adapters):
        if existing.get("id") == item.get("id"):
            adapters[index] = item
            return
    adapters.append(item)


def dump_scan_index() -> None:
    catalog = load_catalog()
    for adapter in catalog["adapters"]:
        for bin_name in adapter.get("bins") or []:
            print(f"bin {bin_name}")
        print(f"hook {expand_home(adapter['settings'])}")
        needle = adapter.get("chat_store_needle")
        if needle:
            for bin_name in adapter.get("bins") or []:
                print(f"chat_store {bin_name} {needle}")


if __name__ == "__main__":
    if sys.argv[1:] == ["dump"]:
        dump_scan_index()
    else:
        raise SystemExit("usage: catalog.py dump")
