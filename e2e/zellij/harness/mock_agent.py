"""Deterministic agent process for lifecycle tests; JSON lines in/out.

Session files belong to this mock CLI, never to the board or its database.
Resume requires an exact id. EOF is a graceful exit; SIGKILL cannot emit exit.
"""

from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import sys
import time
import uuid


def persist(path: Path, value: dict) -> None:
    temporary = path.with_suffix(".tmp")
    with temporary.open("w") as stream:
        json.dump(value, stream)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    descriptor = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--store", type=Path, required=True)
    parser.add_argument("--exit-mode", choices=("normal", "ignore", "crash"), default="normal")
    parser.add_argument("--exit-delay", type=float, default=0)
    parser.add_argument("action", choices=("new", "resume"))
    parser.add_argument("session_id", nargs="?")
    args = parser.parse_args()
    if args.exit_delay < 0 or (args.action == "resume") != (args.session_id is not None):
        parser.error("resume requires an exact session id; new takes no id; delay must be nonnegative")
    try:
        session_id = str(uuid.UUID(args.session_id)) if args.session_id else str(uuid.uuid4())
    except ValueError:
        parser.error("session id must be a UUID")
    args.store.mkdir(parents=True, exist_ok=True)
    path = args.store / f"{session_id}.json"
    instance = str(uuid.uuid4())
    with (args.store / f"{session_id}.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print("session already running", file=sys.stderr)
            return 2
        if args.action == "resume":
            try:
                session = json.loads(path.read_text())
                if session["session_id"] != session_id or not isinstance(session["messages"], list):
                    raise ValueError("invalid session")
            except (OSError, ValueError, KeyError, TypeError) as error:
                print(f"cannot resume: {error}", file=sys.stderr)
                return 2
        else:
            session = {"session_id": session_id, "messages": []}
        persist(path, session)
        status = "idle"
        draft = ""

        def emit(event: str, **extra) -> None:
            value = dict(event=event, session_id=session_id, instance_id=instance,
                         pid=os.getpid(), status=status, draft=draft, **extra)
            with (args.store / "events.jsonl").open("a") as stream:
                stream.write(json.dumps(value) + "\n")
                stream.flush()
                os.fsync(stream.fileno())
            print(json.dumps(value), flush=True)

        emit("ready", resumed=args.action == "resume", messages=session["messages"])
        for line in sys.stdin:
            try:
                command = json.loads(line)
                op = command["op"]
                if op == "submit":
                    if status != "idle":
                        raise ValueError("agent is not idle")
                    message = command["text"]
                    if not isinstance(message, str):
                        raise ValueError("text must be a string")
                    session["messages"].append({"role": "user", "text": message})
                    persist(path, session)
                    draft, status = "", "working"
                elif op == "finish":
                    if status != "working":
                        raise ValueError("agent is not working")
                    message = command["text"]
                    if not isinstance(message, str):
                        raise ValueError("text must be a string")
                    session["messages"].append({"role": "assistant", "text": message})
                    persist(path, session)
                    status = "idle"
                elif op == "permission":
                    if status != "working":
                        raise ValueError("agent is not working")
                    status = "waiting"
                elif op == "approve":
                    if status != "waiting":
                        raise ValueError("no pending permission")
                    status = "working"
                elif op == "draft":
                    if status != "idle" or not isinstance(command["text"], str):
                        raise ValueError("draft requires idle agent and string text")
                    draft = command["text"]
                elif op == "exit":
                    if status != "idle" or draft:
                        raise ValueError("exit requires idle agent without a draft")
                    if args.exit_mode == "ignore":
                        emit("exit_ignored")
                        continue
                    if args.exit_mode == "crash":
                        os._exit(17)
                    time.sleep(args.exit_delay)
                    break
                elif op != "inspect":
                    raise ValueError(f"unknown operation: {op}")
                emit(op)
            except (ValueError, KeyError, TypeError) as error:
                emit("error", error=str(error))
        persist(path, session)
        emit("exited")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
