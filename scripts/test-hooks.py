#!/usr/bin/env python3
"""Exercise the real hook script with isolated spool and state directories."""

import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest


HOOK = Path(__file__).resolve().parent / "zellij-agent-board-hook.sh"


class HookTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = dict(os.environ, TMPDIR=str(self.root),
                        ZAB_STATE_DIR=str(self.root / "state"),
                        ZELLIJ_SESSION_NAME="hook-test", ZELLIJ_PANE_ID="42")
        self.spool = self.root / "zellij-agent-board-spool" / "hook-test-42"
        self.started = self.root / "state" / "started" / "hook-test-42"

    def hook(self, event, payload=None):
        result = subprocess.run(
            ["bash", str(HOOK), event], input=json.dumps(payload or {}),
            text=True, capture_output=True, env=self.env, timeout=5,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_readers_keep_old_markers_during_prompt_publication(self):
        old_hook = b"HOOK hook-test 42 sessionStart @1\n"
        old_start = b"STARTED hook-test 42 1\n"
        for path, contents in ((self.spool, old_hook), (self.started, old_start)):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(contents)
        with self.spool.open("rb", buffering=0) as notice, self.started.open("rb", buffering=0) as start:
            prefix_notice, prefix_start = notice.read(5), start.read(5)
            self.hook("beforeSubmitPrompt", {"prompt": "work on the board"})
            self.assertEqual(prefix_notice + notice.read(), old_hook)
            self.assertEqual(prefix_start + start.read(), old_start)
        self.assertIn("beforeSubmitPrompt", self.spool.read_text())
        self.assertIn("work on the board", self.spool.read_text())
        self.assertGreater(int(self.started.read_text().split()[3]), 1)
        for parent in (self.spool.parent, self.started.parent):
            self.assertEqual(list((parent / ".pending").iterdir()), [])

    def test_tool_notices_preserve_start_and_completion_clears_it(self):
        self.hook("beforeSubmitPrompt")
        started = self.started.read_bytes()
        self.hook("preToolUse", {"tool_name": "Shell", "tool_input": {"command": "pwd"}})
        self.assertEqual(self.started.read_bytes(), started)
        self.assertIn("Shell pwd", self.spool.read_text())
        # Same completion cleanup as stop, without emitting a terminal notification.
        self.hook("afterAgentResponse")
        self.assertFalse(self.started.exists())
        self.assertIn("afterAgentResponse", self.spool.read_text())

    def test_codex_interrupt_clears_working_marker(self):
        self.hook("UserPromptSubmit")
        self.assertTrue(self.started.exists())
        self.hook("Interrupt")
        self.assertIn(" interrupt ", self.spool.read_text())
        self.assertFalse(self.started.exists())
        self.hook("UserPromptSubmit")
        self.assertTrue(self.started.exists())

    def test_cursor_stop_status_distinguishes_abort_and_error(self):
        self.hook("beforeSubmitPrompt")
        self.hook("stop", {"hook_event_name": "stop", "status": "aborted"})
        self.assertIn(" interrupt ", self.spool.read_text())
        self.assertFalse(self.started.exists())
        self.hook("beforeSubmitPrompt")
        self.assertTrue(self.started.exists())
        self.hook("stop", {"hook_event_name": "stop", "status": "error"})
        self.assertIn(" stopFailure ", self.spool.read_text())
        self.assertFalse(self.started.exists())
        self.hook("beforeSubmitPrompt")
        self.hook("afterAgentResponse", {
            "hook_event_name": "afterAgentResponse",
            "status": "completed",
        })
        self.assertIn(" afterAgentResponse ", self.spool.read_text())
        self.assertFalse(self.started.exists())
        self.hook("beforeSubmitPrompt")
        self.hook("stop", {"hook_event_name": "stop", "status": "completed"})
        self.assertIn(" stop ", self.spool.read_text())
        self.assertFalse(self.started.exists())

    def test_notification_types_map_or_are_ignored(self):
        self.hook("UserPromptSubmit")
        self.assertTrue(self.started.exists())
        self.hook("Notification", {
            "hook_event_name": "Notification",
            "notification_type": "permission_prompt",
            "message": "needs permission to use Bash",
        })
        self.assertIn(" permissionRequest ", self.spool.read_text())
        self.assertIn("needs permission to use Bash", self.spool.read_text())
        self.assertTrue(self.started.exists())
        self.hook("Notification", {
            "hook_event_name": "Notification",
            "notification_type": "idle_prompt",
        })
        self.assertIn(" idleWait ", self.spool.read_text())
        self.assertFalse(self.started.exists())
        self.hook("UserPromptSubmit")
        self.hook("Notification", {
            "hook_event_name": "Notification",
            "notification_type": "auth_success",
        })
        self.assertIn(" beforeSubmitPrompt ", self.spool.read_text())
        self.assertNotIn(" auth_success ", self.spool.read_text())
        self.assertNotIn(" notification ", self.spool.read_text())
        self.assertTrue(self.started.exists())

    def test_stop_failure_and_permission_request_normalize(self):
        self.hook("UserPromptSubmit")
        self.hook("PermissionRequest", {
            "hook_event_name": "PermissionRequest",
            "tool_name": "Bash",
        })
        self.assertIn(" permissionRequest ", self.spool.read_text())
        self.assertIn("Bash", self.spool.read_text())
        self.assertTrue(self.started.exists())
        self.hook("StopFailure", {
            "hook_event_name": "StopFailure",
            "error": "rate_limit",
        })
        self.assertIn(" stopFailure ", self.spool.read_text())
        self.assertIn("rate_limit", self.spool.read_text())
        self.assertFalse(self.started.exists())
        self.hook("session.compacted")
        self.assertIn(" postCompact ", self.spool.read_text())

    def test_opencode_error_and_status_payloads(self):
        self.hook("UserPromptSubmit")
        self.hook("session.error", {
            "hook_event_name": "session.error",
            "error": {"name": "MessageAbortedError", "message": "aborted"},
        })
        self.assertIn(" interrupt ", self.spool.read_text())
        self.assertFalse(self.started.exists())
        self.hook("UserPromptSubmit")
        self.hook("session.error", {
            "hook_event_name": "session.error",
            "error": {"name": "ApiError", "message": "provider 500"},
        })
        self.assertIn(" stopFailure ", self.spool.read_text())
        self.assertIn("ApiError", self.spool.read_text())
        self.assertFalse(self.started.exists())
        self.hook("UserPromptSubmit")
        self.assertTrue(self.started.exists())
        self.hook("session.status", {
            "hook_event_name": "session.status",
            "status": {"type": "retry", "message": "rate limited"},
        })
        self.assertIn(" permissionRequest ", self.spool.read_text())
        self.assertTrue(self.started.exists())
        self.hook("session.status", {
            "hook_event_name": "session.status",
            "status": {"type": "busy"},
        })
        self.assertIn(" postToolUse ", self.spool.read_text())
        self.assertTrue(self.started.exists())
        prompt = self.spool.read_text()
        self.hook("session.status", {
            "hook_event_name": "session.status",
            "status": {"type": "idle"},
        })
        self.assertEqual(self.spool.read_text(), prompt)

    def test_reasonix_native_payload_and_interrupt(self):
        self.hook("UserPromptSubmit", {
            "event": "UserPromptSubmit",
            "prompt": "fix the board",
        })
        self.assertIn(" beforeSubmitPrompt ", self.spool.read_text())
        self.assertIn("fix the board", self.spool.read_text())
        self.assertTrue(self.started.exists())
        self.hook("PreToolUse", {
            "event": "PreToolUse",
            "toolName": "bash",
            "toolArgs": {"command": "ls"},
        })
        self.assertIn(" preToolUse ", self.spool.read_text())
        self.assertIn("bash", self.spool.read_text())
        self.assertIn("ls", self.spool.read_text())
        self.assertTrue(self.started.exists())
        self.hook("Stop", {"event": "Stop", "isInterrupt": True})
        self.assertIn(" interrupt ", self.spool.read_text())
        self.assertFalse(self.started.exists())
        self.hook("UserPromptSubmit")
        self.hook("Notification", {
            "event": "Notification",
            "notificationType": "permission_prompt",
            "message": "approve bash",
        })
        self.assertIn(" permissionRequest ", self.spool.read_text())
        self.assertTrue(self.started.exists())

    def test_outside_zellij_does_not_write_state(self):
        self.env.pop("ZELLIJ_PANE_ID")
        self.env.pop("ZELLIJ_SESSION_NAME")
        self.hook("beforeSubmitPrompt")
        self.assertEqual(list(self.root.iterdir()), [])

    def _fake_ps_lsof(self, work, ps_body):
        bindir = self.root / "bin"
        bindir.mkdir()
        (bindir / "ps").write_text("#!/bin/sh\n" + ps_body)
        (bindir / "lsof").write_text(
            "#!/bin/sh\n"
            f"printf '%s\\n' \"n{work}\"\n"
        )
        for name in ("ps", "lsof"):
            (bindir / name).chmod(0o755)
        self.env["PATH"] = f"{bindir}{os.pathsep}{self.env['PATH']}"

    def test_codex_daemon_recovers_pane_from_tui_cwd(self):
        self.env.pop("ZELLIJ_PANE_ID")
        self.env.pop("ZELLIJ_SESSION_NAME")
        work = self.root / "work"
        work.mkdir()
        self._fake_ps_lsof(
            work,
            "if [ \"$1\" = \"-ax\" ]; then\n"
            "  printf '%s\\n' \"4242 codex\"\n"
            "  exit 0\n"
            "fi\n"
            "printf '%s\\n' \"codex ZELLIJ_PANE_ID=7 ZELLIJ_SESSION_NAME=leecode\"\n",
        )
        self.hook("Stop", {"cwd": str(work), "session_id": "01a0e684"})
        spool = self.root / "zellij-agent-board-spool" / "leecode-7"
        self.assertIn(" stop ", spool.read_text())

    def test_codex_daemon_wrong_env_prefers_tui_cwd(self):
        # Daemon born in daily/4 still exports those Zellij ids.
        self.env["ZELLIJ_SESSION_NAME"] = "daily"
        self.env["ZELLIJ_PANE_ID"] = "4"
        work = self.root / "work"
        work.mkdir()
        self._fake_ps_lsof(
            work,
            "if [ \"$1\" = \"-ax\" ]; then\n"
            "  printf '%s\\n' \"4242 codex\"\n"
            "  exit 0\n"
            "fi\n"
            "printf '%s\\n' \"codex ZELLIJ_PANE_ID=7 ZELLIJ_SESSION_NAME=leecode\"\n",
        )
        self.hook("PostToolUse", {
            "cwd": str(work),
            "session_id": "01a0e684",
            "tool_name": "Bash",
            "tool_input": {"command": "pwd"},
        })
        spool = self.root / "zellij-agent-board-spool" / "leecode-7"
        self.assertIn(" postToolUse ", spool.read_text())
        self.assertFalse(
            (self.root / "zellij-agent-board-spool" / "daily-4").exists()
        )

    def test_payload_without_identity_keeps_env(self):
        # Cursor/Claude hooks often omit cwd/session_id; keep inherited pane.
        self.env["ZELLIJ_SESSION_NAME"] = "daily"
        self.env["ZELLIJ_PANE_ID"] = "4"
        self.hook("PreToolUse", {
            "tool_name": "Bash",
            "tool_input": {"command": "./gradlew"},
        })
        spool = self.root / "zellij-agent-board-spool" / "daily-4"
        self.assertIn(" preToolUse ", spool.read_text())

    def test_codex_session_id_loads_cwd_from_rollout(self):
        self.env.pop("ZELLIJ_PANE_ID", None)
        self.env.pop("ZELLIJ_SESSION_NAME", None)
        work = self.root / "work"
        work.mkdir()
        sid = "01a0ead1-0a1c-7b42-b480-94e0b75e8254"
        sessions = self.root / "fake-home" / ".codex" / "sessions" / "2026" / "09" / "29"
        sessions.mkdir(parents=True)
        (sessions / f"rollout-{sid}.jsonl").write_text(
            json.dumps({
                "type": "session_meta",
                "payload": {"session_id": sid, "cwd": str(work)},
            })
            + "\n"
        )
        self.env["HOME"] = str(self.root / "fake-home")
        self._fake_ps_lsof(
            work,
            "if [ \"$1\" = \"-ax\" ]; then\n"
            "  printf '%s\\n' \"4242 codex\"\n"
            "  exit 0\n"
            "fi\n"
            "printf '%s\\n' \"codex ZELLIJ_PANE_ID=252 ZELLIJ_SESSION_NAME=lp\"\n",
        )
        self.hook("Stop", {"session_id": sid})
        spool = self.root / "zellij-agent-board-spool" / "lp-252"
        self.assertIn(" stop ", spool.read_text())

    def test_env_matching_cwd_keeps_inherited_pane(self):
        # Cursor / a real TUI already have the right Zellij ids.
        self.env["ZELLIJ_SESSION_NAME"] = "leecode"
        self.env["ZELLIJ_PANE_ID"] = "0"
        work = self.root / "work"
        work.mkdir()
        self._fake_ps_lsof(
            work,
            "if [ \"$1\" = \"-ax\" ]; then\n"
            "  printf '%s\\n' \"1111 agent\" \"4242 codex\"\n"
            "  exit 0\n"
            "fi\n"
            "case \"$3\" in\n"
            "  1111) printf '%s\\n' \"agent ZELLIJ_PANE_ID=0 ZELLIJ_SESSION_NAME=leecode\" ;;\n"
            "  *) printf '%s\\n' \"codex ZELLIJ_PANE_ID=7 ZELLIJ_SESSION_NAME=leecode\" ;;\n"
            "esac\n",
        )
        self.hook("Stop", {"cwd": str(work), "status": "completed"})
        spool = self.root / "zellij-agent-board-spool" / "leecode-0"
        self.assertIn(" stop ", spool.read_text())
        self.assertFalse(
            (self.root / "zellij-agent-board-spool" / "leecode-7").exists()
        )

    def test_codex_daemon_skips_ambiguous_cwd(self):
        self.env.pop("ZELLIJ_PANE_ID")
        self.env.pop("ZELLIJ_SESSION_NAME")
        work = self.root / "work"
        work.mkdir()
        self._fake_ps_lsof(
            work,
            "if [ \"$1\" = \"-ax\" ]; then\n"
            "  printf '%s\\n' \"4242 codex\" \"4343 codex\"\n"
            "  exit 0\n"
            "fi\n"
            "case \"$3\" in\n"
            "  4242) printf '%s\\n' \"codex ZELLIJ_PANE_ID=7 ZELLIJ_SESSION_NAME=leecode\" ;;\n"
            "  *) printf '%s\\n' \"codex ZELLIJ_PANE_ID=8 ZELLIJ_SESSION_NAME=other\" ;;\n"
            "esac\n",
        )
        self.hook("Stop", {"cwd": str(work)})
        self.assertFalse((self.root / "zellij-agent-board-spool").exists())

    def _codex_shared_cwd(self, panes, name="Daily task", custom_home=False):
        work = self.root / "work"
        work.mkdir()
        self.env["ZELLIJ_SESSION_NAME"] = "daily"
        self.env["ZELLIJ_PANE_ID"] = "4"
        self.env["HOME"] = str(self.root / "fake-home")
        codex_home = Path(self.env["HOME"]) / ".codex"
        if custom_home:
            codex_home = self.root / "custom-codex"
            self.env["CODEX_HOME"] = str(codex_home)
        else:
            self.env.pop("CODEX_HOME", None)
        codex_home.mkdir(parents=True)
        sid = "01a0d803-414c-75c3-b213-cf0fec3a4967"
        with sqlite3.connect(codex_home / "state_5.sqlite") as db:
            db.execute("CREATE TABLE threads (id TEXT, name TEXT, title TEXT, cwd TEXT)")
            db.execute("INSERT INTO threads VALUES (?, ?, ?, ?)",
                       (sid, name, "Original prompt", str(work)))
        self._fake_ps_lsof(
            work,
            'if [ "$1" = "-ax" ]; then\n'
            '  printf "%s\\n" "4242 codex" "4343 codex"\n'
            '  exit 0\n'
            'fi\n'
            'case "$3" in\n'
            '  4242) printf "%s\\n" "codex ZELLIJ_PANE_ID=0 ZELLIJ_SESSION_NAME=daily" ;;\n'
            '  *) printf "%s\\n" "codex ZELLIJ_PANE_ID=8 ZELLIJ_SESSION_NAME=daily" ;;\n'
            'esac\n',
        )
        (self.root / "panes.json").write_text(json.dumps(panes))
        zellij = self.root / "bin" / "zellij"
        zellij.write_text(f'#!/bin/sh\ncat "{self.root / "panes.json"}"\n')
        zellij.chmod(0o755)
        return {"cwd": str(work), "session_id": sid}

    def test_codex_shared_cwd_stop_resolves_thread_name(self):
        payload = self._codex_shared_cwd([
            {"id": 0, "title": "Daily task | work", "is_plugin": False},
            {"id": 8, "title": "Other task | work", "is_plugin": False},
            {"id": 0, "title": "Other plugin", "is_plugin": True},
        ])
        spool = self.root / "zellij-agent-board-spool" / "daily-0"
        started = self.root / "state" / "started" / "daily-0"
        for path, line in [(spool, "HOOK daily 0 postToolUse @1\n"),
                           (started, "STARTED daily 0 1\n")]:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(line)
        self.hook("Stop", payload)
        self.assertIn(" stop ", spool.read_text())
        self.assertFalse(started.exists())
        self.assertFalse((spool.parent / "daily-4").exists())
        self.assertFalse((spool.parent / "daily-8").exists())

    def test_codex_shared_cwd_busy_title_keeps_tool_and_stop_hooks(self):
        panes = [
            {"id": 0, "title": "Daily task | work"},
            {"id": 8, "title": "Other task | work"},
        ]
        payload = self._codex_shared_cwd(panes)
        spool = self.root / "zellij-agent-board-spool" / "daily-0"
        started = self.root / "state" / "started" / "daily-0"
        self.hook("UserPromptSubmit", payload)
        self.assertIn(" beforeSubmitPrompt ", spool.read_text())
        self.assertTrue(started.exists())
        for spinner in ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]:
            with self.subTest(spinner=spinner):
                panes[0]["title"] = f"{spinner} Daily task | work"
                (self.root / "panes.json").write_text(json.dumps(panes))
                self.hook("PostToolUse", payload)
                self.assertIn(" postToolUse ", spool.read_text())
                self.assertTrue(started.exists())
        # Stop runs before the TUI consumes completion and removes its spinner.
        self.hook("Stop", payload)
        self.assertIn(" stop ", spool.read_text())
        self.assertFalse(started.exists())
        self.assertFalse((spool.parent / "daily-4").exists())
        self.assertFalse((spool.parent / "daily-8").exists())

    def test_codex_shared_cwd_respects_custom_home_and_unnamed_thread(self):
        payload = self._codex_shared_cwd([
            {"id": 0, "title": "Other task | work"},
            {"id": 8, "title": "Original prompt | work", "is_suppressed": True},
        ], name=None, custom_home=True)
        self.hook("PostToolUse", payload)
        spool = self.root / "zellij-agent-board-spool" / "daily-8"
        self.assertIn(" postToolUse ", spool.read_text())

    def test_codex_shared_cwd_drops_duplicate_titles(self):
        payload = self._codex_shared_cwd([
            {"id": 0, "title": "Daily task | work"},
            {"id": 8, "title": "Daily task | work"},
        ])
        self.hook("Stop", payload)
        self.assertFalse((self.root / "zellij-agent-board-spool").exists())

    def test_codex_shared_cwd_busy_duplicate_titles_remain_ambiguous(self):
        payload = self._codex_shared_cwd([
            {"id": 0, "title": "⠋ Daily task | work"},
            {"id": 8, "title": "⠙ Daily task | work"},
        ])
        self.hook("PostToolUse", payload)
        self.assertFalse((self.root / "zellij-agent-board-spool").exists())

    def test_codex_shared_cwd_does_not_strip_arbitrary_title_prefixes(self):
        payload = self._codex_shared_cwd([
            {"id": 0, "title": "Review Daily task | work"},
            {"id": 8, "title": "⠋Daily task | work"},
        ])
        self.hook("Stop", payload)
        self.assertFalse((self.root / "zellij-agent-board-spool").exists())

    def test_codex_shared_cwd_ignores_non_agent_and_prefix_titles(self):
        payload = self._codex_shared_cwd([
            {"id": 0, "title": "Daily task extended | work"},
            {"id": 8, "title": "Original prompt | work"},
            {"id": 99, "title": "Daily task | work"},
            {"id": 0, "title": "Daily task | work", "is_plugin": True},
        ])
        self.hook("Stop", payload)
        self.assertFalse((self.root / "zellij-agent-board-spool").exists())

    def test_codex_shared_cwd_missing_db_does_not_create_one(self):
        payload = self._codex_shared_cwd([{"id": 0, "title": "Daily task | work"}])
        db = Path(self.env["HOME"]) / ".codex" / "state_5.sqlite"
        db.unlink()
        self.hook("Stop", payload)
        self.assertFalse(db.exists())
        self.assertFalse((self.root / "zellij-agent-board-spool").exists())

    def test_codex_shared_cwd_failed_pane_query_drops_hook(self):
        payload = self._codex_shared_cwd([{"id": 0, "title": "Daily task | work"}])
        zellij = self.root / "bin" / "zellij"
        for body in ['printf "not-json"', 'exit 1']:
            with self.subTest(body=body):
                zellij.write_text(f"#!/bin/sh\n{body}\n")
                self.hook("Stop", payload)
                self.assertFalse((self.root / "zellij-agent-board-spool").exists())

    def test_codex_shared_cwd_requires_matching_thread_cwd(self):
        payload = self._codex_shared_cwd([{"id": 0, "title": "Daily task | work"}])
        with sqlite3.connect(Path(self.env["HOME"]) / ".codex" / "state_5.sqlite") as db:
            db.execute("UPDATE threads SET cwd = ?", (str(self.root / "other-work"),))
        self.hook("Stop", payload)
        self.assertFalse((self.root / "zellij-agent-board-spool").exists())

    def test_codex_shared_cwd_supports_schema_without_name(self):
        payload = self._codex_shared_cwd([{"id": 0, "title": "Original prompt | work"}])
        with sqlite3.connect(Path(self.env["HOME"]) / ".codex" / "state_5.sqlite") as db:
            db.execute("ALTER TABLE threads RENAME TO old_threads")
            db.execute("CREATE TABLE threads AS SELECT id, title, cwd FROM old_threads")
        self.hook("PostToolUse", payload)
        spool = self.root / "zellij-agent-board-spool" / "daily-0"
        self.assertIn(" postToolUse ", spool.read_text())


class InstallTests(unittest.TestCase):
    def test_reasonix_writes_flat_hook_commands(self):
        import importlib.util

        scripts = Path(__file__).resolve().parent
        if str(scripts) not in sys.path:
            sys.path.insert(0, str(scripts))
        spec = importlib.util.spec_from_file_location(
            "install_hooks", scripts / "install-hooks.py"
        )
        installer = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(installer)

        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        settings = root / "settings.json"
        hook = root / "zellij-agent-board-hook.sh"
        adapter = {"settings": str(settings), "protocol": "reasonix"}
        protocols = {
            "reasonix": {
                "events": ["UserPromptSubmit", "Stop", "PermissionRequest"],
            }
        }
        installer.install_reasonix(adapter, protocols, str(hook))
        data = json.loads(settings.read_text())
        self.assertEqual(
            data["hooks"]["UserPromptSubmit"],
            [{"command": f"{hook} UserPromptSubmit"}],
        )
        self.assertEqual(data["hooks"]["Stop"], [{"command": f"{hook} Stop"}])
        installer.install_reasonix(adapter, protocols, str(hook))
        again = json.loads(settings.read_text())
        self.assertEqual(len(again["hooks"]["Stop"]), 1)


if __name__ == "__main__":
    unittest.main()
