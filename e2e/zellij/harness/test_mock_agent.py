"""Exercise the mock as a real process, including durability and exit faults."""
import json
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import unittest
import uuid


CLI = Path(__file__).with_name("mock_agent.py")


class MockAgentProcess(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.store = Path(self.temporary.name)

    def start(self, session=None, mode="normal"):
        process = subprocess.Popen(
            [sys.executable, str(CLI), "--store", str(self.store),
             "--exit-mode", mode, *( ["resume", session] if session else ["new"])],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, bufsize=1,
        )
        self.addCleanup(self.close, process)
        return process

    def close(self, process):
        if process.poll() is None:
            process.kill()
        process.wait(timeout=5)
        for stream in (process.stdin, process.stdout, process.stderr):
            stream.close()

    def read(self, process):
        self.assertTrue(select.select([process.stdout], [], [], 5)[0], "mock response timed out")
        line = process.stdout.readline()
        self.assertTrue(line, "mock exited before replying")
        return json.loads(line)

    def send(self, process, op, **values):
        process.stdin.write(json.dumps(dict(op=op, **values)) + "\n")
        process.stdin.flush()
        return self.read(process)

    def test_exact_resume_changes_process_but_preserves_conversation(self):
        first = self.start()
        ready = self.read(first)
        self.send(first, "submit", text="remember this marker")
        self.send(first, "finish", text="saved marker")
        self.assertEqual(self.send(first, "exit")["event"], "exited")
        self.assertEqual(first.wait(timeout=5), 0)
        # A newer conversation must not redirect an exact resume.
        other = self.start()
        other_ready = self.read(other)
        resumed = self.start(ready["session_id"])
        restored = self.read(resumed)
        self.assertEqual(restored["session_id"], ready["session_id"])
        self.assertNotEqual(restored["session_id"], other_ready["session_id"])
        self.assertNotEqual(restored["instance_id"], ready["instance_id"])
        self.assertNotEqual(restored["pid"], ready["pid"])
        self.assertTrue(restored["resumed"])
        self.assertEqual([m["text"] for m in restored["messages"]],
                         ["remember this marker", "saved marker"])

    def test_unknown_and_already_running_sessions_are_rejected(self):
        running = self.start()
        ready = self.read(running)
        for session in (ready["session_id"], str(uuid.uuid4())):
            rejected = self.start(session)
            self.assertEqual(rejected.wait(timeout=5), 2)
            self.assertEqual(rejected.stdout.read(), "")
        self.assertEqual(len(list(self.store.glob("*.json"))), 1)

    def test_busy_permission_and_draft_prevent_normal_exit(self):
        process = self.start()
        self.read(process)
        self.send(process, "draft", text="unsent")
        self.assertEqual(self.send(process, "exit")["event"], "error")
        self.send(process, "submit", text="work")
        self.assertEqual(self.send(process, "exit")["event"], "error")
        self.assertEqual(self.send(process, "permission")["status"], "waiting")
        self.assertEqual(self.send(process, "exit")["event"], "error")
        self.send(process, "approve")
        self.send(process, "finish", text="done")
        self.assertEqual(self.send(process, "exit")["event"], "exited")
        self.assertEqual(process.wait(timeout=5), 0)

    def test_ignored_exit_remains_alive_and_crash_has_no_clean_exit_event(self):
        process = self.start(mode="ignore")
        self.read(process)
        self.assertEqual(self.send(process, "exit")["event"], "exit_ignored")
        self.assertIsNone(process.poll())
        self.assertEqual(self.send(process, "inspect")["status"], "idle")
        crashed = self.start(mode="crash")
        ready = self.read(crashed)
        self.send(crashed, "submit", text="durable before crash")
        self.send(crashed, "finish", text="done")
        crashed.stdin.write('{"op":"exit"}\n')
        crashed.stdin.flush()
        self.assertEqual(crashed.wait(timeout=5), 17)
        events = [json.loads(line) for line in (self.store / "events.jsonl").read_text().splitlines()]
        self.assertFalse(any(e["event"] == "exited" and e["instance_id"] == ready["instance_id"] for e in events))
        resumed = self.start(ready["session_id"])
        self.assertEqual(self.read(resumed)["messages"][0]["text"], "durable before crash")
