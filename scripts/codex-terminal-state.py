"""Read terminal turn evidence for live, title-identified Codex panes.

Embedded by the native scanner. This reads the local DB/rollout, never sends
input to Codex and never prints transcript contents. Unknown formats fail open.
"""
import datetime
import json
from pathlib import Path
import re
import sqlite3
import sys

TAIL_BYTES = 512 * 1024
SPINNER = re.compile(r"^[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏] ")


def terminal_event(path):
    latest = None
    with path.open("rb") as handle:
        handle.seek(0, 2)
        length = handle.tell()
        handle.seek(max(0, length - TAIL_BYTES))
        for line in handle.read().decode(errors="replace").splitlines():
            try:
                row = json.loads(line)
            except ValueError:
                continue
            if not isinstance(row, dict):
                continue
            payload = row.get("payload")
            if row.get("type") != "event_msg" or not isinstance(payload, dict):
                continue
            kind = payload.get("type")
            if kind in ("task_started", "turn_started", "task_complete", "turn_completed", "turn_aborted"):
                latest = row
    if not latest:
        return None
    payload = latest["payload"]
    kind = payload["type"]
    if kind == "turn_aborted":
        event = "interrupt"
    elif kind in ("task_complete", "turn_completed") and payload.get("error"):
        event = "stopFailure"
    else:
        return None  # No guesses for active turns or successful historic turns.
    timestamp = latest.get("timestamp")
    if not isinstance(timestamp, str):
        return None
    stamp = datetime.datetime.fromisoformat(timestamp.replace("Z", "+00:00"))
    if stamp.tzinfo is None:
        return None
    return event, int(stamp.timestamp() * 1000)


def notices(home, panes):
    paths = [p for p in home.glob("state_*.sqlite") if p.stem[6:].isdigit()]
    if not paths or not panes:
        return []
    database = max(paths, key=lambda p: int(p.stem[6:]))
    # Never create a DB or run a migration; release schemas may change.
    with sqlite3.connect(database.resolve().as_uri() + "?mode=ro", uri=True, timeout=0.1) as db:
        threads = list(db.execute("SELECT id, name, rollout_path FROM threads"))
    result = []
    for pane in panes:
        title = SPINNER.sub("", pane["title"])
        title = title.removeprefix("[ ! ] Action Required | ")
        matched = [row for row in threads if row[1] and
                   (title == row[1] or title.startswith(row[1] + " | "))]
        if not matched and (title.startswith("codex ") or title.startswith("Pane #")):
            matched = [row for row in threads if row[0] == pane.get("resume_thread")]
        if len(matched) != 1:
            continue  # Same cwd or duplicate names do not establish ownership.
        try:
            path = Path(matched[0][2])
            path.resolve().relative_to((home / "sessions").resolve())
            terminal = terminal_event(path)
        except (OSError, ValueError, KeyError, TypeError, OverflowError):
            continue
        if terminal:
            event, at_ms = terminal
            if at_ms > 0:
                result.append(dict(session=pane["session"], pane=pane["pane"],
                                   event=event, at_ms=at_ms))
    return result


if __name__ == "__main__":
    try:
        request = json.loads(sys.argv[1])
        result = notices(Path(request["home"]), request["panes"])
    except (OSError, ValueError, KeyError, TypeError, sqlite3.Error):
        result = []
    print(json.dumps(result, separators=(",", ":")))
