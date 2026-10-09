#!/usr/bin/env python3
"""Launch the actual WASM bridge from a pane whose working directory was removed.

python3 -B scripts/test-deleted-cwd.py WASM BOARD_TUI
Uses a throwaway Zellij session and PTY client; no user tabs or config are changed.
The plugin path must already have its ordinary Zellij permissions granted.
"""
import json
import os
from pathlib import Path
import pty
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time
import fcntl
import termios

if os.name != "posix" or not shutil.which("zellij"):
    print("deleted-cwd test requires Zellij and a Unix PTY; skipped")
    raise SystemExit(0)

wasm, tui = map(lambda path: Path(path).resolve(), sys.argv[1:3])
with tempfile.TemporaryDirectory(prefix="zab-cwd-", dir="/tmp") as directory:
    root = Path(directory).resolve()
    session = f"zab-cwd-{os.getpid()}"
    env = os.environ.copy()
    for key in ["ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_PANE_ID"]:
        env.pop(key, None)
    env.update(TMPDIR=directory, ZELLIJ_SOCKET_DIR=directory,
               ZAB_STATE_DIR=str(root / "state"), ZAB_SCAN_SESSION_PREFIX=session,
               ZELLIJ_AGENT_BOARD_NO_STATS="1", TERM="xterm-256color")
    config = root / "config.kdl"
    config.write_text(f'''keybinds clear-defaults=true {{
    normal {{
        bind "Alt q" {{
            LaunchPlugin "file:{wasm}" {{
                floating true
                tui "{tui}"
            }}
        }}
    }}
}}
session_serialization false
show_startup_tips false
show_release_notes false
''')
    worktree = root / "removed-worktree"
    worktree.mkdir()
    master = None
    client = None
    reader = None

    def zellij(*args):
        result = subprocess.run(["zellij", "--session", session, *args],
                                env=env, capture_output=True, text=True, timeout=10)
        if result.returncode:
            raise RuntimeError(result.stderr)
        return result.stdout

    def panes():
        return json.loads(zellij("action", "list-panes", "--all", "--json"))

    def wait(check, message, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = check()
            if result:
                return result
            time.sleep(0.1)
        raise AssertionError(message)

    try:
        created = subprocess.run(["zellij", "--config", str(config), "attach", "--create-background", session],
                                 env=env, capture_output=True, text=True, timeout=10)
        if created.returncode:
            raise RuntimeError(created.stderr)
        client, master = pty.fork()
        if client == 0:
            os.execvpe("zellij", ["zellij", "--config", str(config), "attach", session], env)
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))

        def drain():
            while True:
                try:
                    if not os.read(master, 65536):
                        return
                except OSError:
                    return

        reader = threading.Thread(target=drain, daemon=True)
        reader.start()
        wait(lambda: len(zellij("action", "list-clients").splitlines()) > 1,
             "test client did not attach")
        pane_id = zellij("action", "new-pane", "--cwd", str(worktree), "--name", "stale-cwd",
                         "--close-on-exit", "--", "/bin/sleep", "600").strip()
        numeric_id = int(pane_id.removeprefix("terminal_"))
        stale = wait(lambda: next((p for p in panes()
                                 if not p["is_plugin"] and p["id"] == numeric_id
                                 and p.get("pane_cwd") == str(worktree)), None),
                     "test pane did not start in the worktree")
        worktree.rmdir()
        assert not worktree.exists()
        os.write(master, b"\x1bq")
        board = wait(lambda: next((p for p in panes() if not p["is_plugin"]
                                 and p["title"] == "board-tui" and not p["exited"]), None),
                     "board did not open from the removed worktree")
        board_id = f'terminal_{board["id"]}'
        wait(lambda: "q Quit" in zellij("action", "dump-screen", "--pane-id", board_id),
             "board did not render its navigation")
        assert board.get("pane_cwd") == "/", board.get("pane_cwd")
        zellij("action", "write-chars", "--pane-id", board_id, "q")
        wait(lambda: not any(not p["is_plugin"] and p["id"] == board["id"] for p in panes()),
             "q did not close the board")
        assert any(not p["is_plugin"] and p["id"] == stale["id"] for p in panes())
        print("PASS: bridge and native board open from removed cwd; q closes only the board.")
    finally:
        subprocess.run(["zellij", "delete-session", "--force", "--", session], env=env,
                       capture_output=True, timeout=10)
        if client:
            # Closing the session normally exits its client; kill only our PTY client if needed.
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                if os.waitpid(client, os.WNOHANG)[0]:
                    client = None
                    break
                time.sleep(0.05)
            if client:
                try:
                    os.kill(client, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                os.waitpid(client, 0)
        if master is not None:
            os.close(master)
        if reader:
            reader.join(timeout=1)
