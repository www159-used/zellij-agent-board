#!/usr/bin/env python3
"""Exercise the real hook script with isolated spool and state directories."""

import json
import os
from pathlib import Path
import subprocess
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

    def test_outside_zellij_does_not_write_state(self):
        self.env.pop("ZELLIJ_PANE_ID")
        self.env.pop("ZELLIJ_SESSION_NAME")
        self.hook("beforeSubmitPrompt")
        self.assertEqual(list(self.root.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
