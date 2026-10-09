#!/usr/bin/env python3
"""Exercise embedded bottom, navigation and terminal cleanup in isolated PTYs.

Usage: python3 -B scripts/test-monitor.py target/debug/board-tui
Each board uses an empty PATH and an isolated bottom config.
"""

import json
import os
from pathlib import Path
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import time
import unittest

if os.name != "posix":
    print("monitor tests require a Unix PTY; skipped")
    raise SystemExit(0)

import fcntl
import pty
import termios

BINARY = Path(sys.argv.pop(1)).resolve()

class BoardTTY:
    def __init__(self, binary=BINARY, bad_config=False, preferences=None, settings_path=None):
        self.temp = tempfile.TemporaryDirectory(prefix="zab-monitor-")
        self.root = Path(self.temp.name)
        state = self.root / "state"
        state.mkdir()
        (state / "scan").write_text("META hooks=1\n" + "".join(
            f"SCAN test {p} agent agent --workspace /tmp/task-{p}\n"
            for p in range(40)
        ))
        (state / "places").write_text("".join(
            f"PLACE test {p} 0\tmain\ttask-{p:02}\n" for p in range(40)
        ))
        # Freeze the fixture snapshot without querying real Zellij sessions.
        self.lock = (state / "reconcile.lock").open("w")
        fcntl.flock(self.lock, fcntl.LOCK_EX)
        bindir = self.root / "bin"
        bindir.mkdir()
        config = self.root / "bottom.toml"
        config.write_text("invalid = [" if bad_config else "[flags]\nrate = 250\n")
        self.settings_path = Path(settings_path) if settings_path else self.root / "settings.json"
        if preferences is not None:
            self.settings_path.write_text(json.dumps(preferences))
        env = os.environ.copy()
        env.update(
            ZAB_STATE_DIR=str(state), ZELLIJ_SESSION_NAME="test",
            TMPDIR=str(self.root), TERM="xterm-256color",
            ZELLIJ_AGENT_BOARD_STATS=str(state / "usage.jsonl"),
            PATH=str(bindir), ZAB_BOTTOM_CONFIG=str(config),
            ZAB_SETTINGS_PATH=str(self.settings_path),
        )
        env.pop("ZELLIJ_AGENT_BOARD_NO_STATS", None)
        ready_r, ready_w = os.pipe()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.close(ready_w)
            os.read(ready_r, 1)
            os.close(ready_r)
            os.execve(str(binary), [str(binary)], env)
        os.close(ready_r)
        self.original = termios.tcgetattr(self.fd)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 120, 0, 0))
        os.write(ready_w, b"1")
        os.close(ready_w)
        self.exited = False

    def read_until(self, needle, timeout=5):
        output = b""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if select.select([self.fd], [], [], 0.1)[0]:
                try:
                    chunk = os.read(self.fd, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                output += chunk
                # Ratatui can position the cursor across spaces instead of
                # writing them. Compare visible text as well as raw escapes.
                visible = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", output)
                if needle in output or (b"\x1b" not in needle and
                        re.sub(rb"\s+", b"", needle) in re.sub(rb"\s+", b"", visible)):
                    return output
        raise AssertionError(f"Missing {needle!r}; last output: {output[-1000:]!r}")

    def send(self, text):
        os.write(self.fd, text)

    def wait_setting(self, key, value):
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            # Keep draining the PTY while waiting on the settings file: a full
            # Settings frame can otherwise block the child before it reads the click.
            if select.select([self.fd], [], [], 0.02)[0]:
                os.read(self.fd, 65536)
            if self.settings_path.exists() and json.loads(self.settings_path.read_text())[key] == value:
                return
        raise AssertionError(f"Setting {key!r} was not saved as {value!r}")

    def close_board(self):
        self.send(b"q")
        self.read_until(b"\x1b[?1049l")
        self.wait_exit()

    def wait_exit(self):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.exited = True
                assert os.waitstatus_to_exitcode(status) == 0, status
                assert termios.tcgetattr(self.fd) == self.original
                return
            if select.select([self.fd], [], [], 0.02)[0]:
                try:
                    os.read(self.fd, 65536)
                except OSError:
                    pass
        raise AssertionError("board did not exit")

    def cleanup(self):
        if not self.exited:
            pid, _ = os.waitpid(self.pid, os.WNOHANG)
            if not pid:
                try:
                    os.killpg(self.pid, signal.SIGKILL)
                except (ProcessLookupError, PermissionError):
                    try:
                        os.kill(self.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline:
                    if os.waitpid(self.pid, os.WNOHANG)[0]:
                        break
                    if select.select([self.fd], [], [], 0)[0]:
                        try:
                            os.read(self.fd, 65536)
                        except OSError:
                            pass
                    time.sleep(0.02)
                else:
                    raise AssertionError("test child did not exit after SIGKILL")
        os.close(self.fd)
        self.lock.close()
        self.temp.cleanup()


class MonitorTests(unittest.TestCase):
    def board(self, **kwargs):
        board = BoardTTY(**kwargs)
        self.addCleanup(board.cleanup)
        board.read_until(b"task-00")
        return board

    def click(self, board, x, y):
        board.send(f"\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m".encode())

    def test_embedded_bottom_without_path_and_clickable_navigation(self):
        board = self.board()
        board.send(b"G")
        board.read_until(b"task-39")
        board.send(b"t")
        board.read_until(b"CPU", timeout=10)
        # The monitor uses its own input handling while navigation stays live.
        board.send(b"?")
        board.read_until(b"General")
        self.click(board, 3, 1)
        board.read_until(b"task-39")
        self.click(board, 17, 1)
        board.read_until(b"General")
        board.send(b"\x1b")
        # Wait past crossterm's Escape/Alt disambiguation window.
        time.sleep(0.1)
        board.read_until(b"CPU")
        fcntl.ioctl(board.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 90, 0, 0))
        os.kill(board.pid, signal.SIGWINCH)
        board.read_until(b"CPU")
        board.close_board()

    def test_q_exits_home_picker_search_and_overlays(self):
        for mode, needle in [(b"p", b"PICK"), (b"/", b"find"),
                             (b"?", b"help"), (b"S", b"settings")]:
            with self.subTest(mode=mode):
                board = self.board()
                board.send(mode)
                board.read_until(needle)
                board.close_board()

    def test_monitor_error_recovery_and_retry(self):
        board = self.board(bad_config=True)
        board.send(b"t")
        board.read_until(b"Press t to restart", timeout=10)
        board.send(b"h")
        board.read_until(b"task-00")
        (board.root / "bottom.toml").write_text("[flags]\nrate = 250\n")
        board.send(b"t")
        board.read_until(b"CPU", timeout=10)
        board.close_board()

    def test_q_exits_monitor_help_directly(self):
        board = self.board()
        board.send(b"t")
        board.read_until(b"CPU", timeout=10)
        board.send(b"?")
        board.read_until(b"General")
        processes = subprocess.check_output(["/bin/ps", "-axo", "pid,ppid,command"], text=True)
        children = [int(fields[0]) for line in processes.splitlines()
                    if len(fields := line.split(None, 2)) == 3
                    and fields[1] == str(board.pid) and str(BINARY) in fields[2]
                    and "--reconcile" not in fields[2]]
        self.assertEqual(len(children), 1)
        board.close_board()
        for pid in children:
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)

    def test_click_process_widget_and_q_exits_monitor_search(self):
        board = self.board()
        board.send(b"t")
        board.read_until(b"CPU", timeout=10)
        self.click(board, 90, 25)
        board.send(b"/")
        board.read_until(b"Case(")
        board.close_board()

    def test_click_quit_from_home(self):
        board = self.board()
        # Test the same close path without adding a second keyboard event.
        self.click(board, 116, 1)
        board.read_until(b"\x1b[?1049l")
        board.wait_exit()

    def test_settings_mouse_save_reopen_and_preview_consumes_space(self):
        board = self.board()
        self.click(board, 32, 1)
        board.read_until(b"Privacy / idle protection")
        # Native form content starts under the two-row navigation.
        self.click(board, 29, 9)  # 1min option
        board.wait_setting("idle_seconds", 60)
        self.assertEqual(json.loads(board.settings_path.read_text())["idle_seconds"], 60)
        self.click(board, 33, 12)  # Clock style
        board.wait_setting("saver_style", "clock")
        self.assertEqual(json.loads(board.settings_path.read_text())["saver_style"], "clock")
        self.click(board, 9, 15)  # Preview
        board.read_until(b"Space / click to resume")
        board.send(b" ")
        board.read_until(b"Privacy / idle protection")
        board.send(b"\x1b")
        board.read_until(b"task-00")
        board.send(b"\x0c")  # Ctrl+l enters the curtain from Home.
        board.read_until(b"Space / click to resume")
        board.send(b" ")
        board.read_until(b"task-00")
        board.close_board()
        reopened = self.board(settings_path=board.settings_path)
        reopened.send(b"S")
        reopened.read_until(b"1min")
        self.assertEqual(json.loads(reopened.settings_path.read_text())["saver_style"], "clock")
        reopened.close_board()

    def test_idle_screensaver_restores_picker_and_q_exits(self):
        board = self.board(preferences={"idle_seconds": 1})
        board.send(b"p")
        board.read_until(b"PICK")
        board.read_until(b"Space / click to resume")
        # Pointer motion and Enter do not dismiss the curtain.
        board.send(b"\x1b[<35;3;4M\r")
        time.sleep(0.15)
        board.send(b" ")
        board.read_until(b"PICK")
        board.read_until(b"Space / click to resume")
        board.close_board()

    def test_idle_monitor_and_close_actions(self):
        board = self.board(preferences={"idle_seconds": 1, "idle_action": "monitor"})
        board.read_until(b"CPU", timeout=10)
        board.close_board()
        board = self.board(preferences={"idle_seconds": 1, "idle_action": "close"})
        board.read_until(b"\x1b[?1049l")
        board.wait_exit()

    def test_monitor_screensaver_and_click_resume(self):
        board = self.board(preferences={"idle_seconds": 1})
        board.send(b"t")
        board.read_until(b"CPU", timeout=10)
        board.read_until(b"Space / click to resume")
        self.click(board, 60, 17)
        board.read_until(b"CPU", timeout=10)
        board.close_board()

    def test_screensaver_click_on_blank_area_restores_picker_without_passing_through(self):
        board = self.board()
        board.send(b"p")
        board.read_until(b"PICK")
        board.send(b"\x0c")
        board.read_until(b"Space / click to resume")
        self.click(board, 8, 8)
        board.read_until(b"PICK", timeout=2)
        board.close_board()


if __name__ == "__main__":
    unittest.main()
