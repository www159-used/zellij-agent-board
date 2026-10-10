#!/usr/bin/env python3
"""Exercise the embedded read-only recovery with isolated Codex state."""
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest

READER = Path(__file__).with_name('codex-terminal-state.py')


class TerminalStateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.rollout = self.home / 'sessions/rollout.jsonl'
        self.rollout.parent.mkdir()
        with sqlite3.connect(self.home / 'state_5.sqlite') as db:
            db.execute('CREATE TABLE threads (id TEXT, name TEXT, rollout_path TEXT)')
            db.execute('INSERT INTO threads VALUES (?,?,?)', ('thread-a', 'Current task', str(self.rollout)))
        self.pane = dict(session='test', pane=42, title='Current task | project', resume_thread='thread-a')

    def write(self, *events):
        self.rollout.write_text(''.join(json.dumps(dict(timestamp=f'2026-10-10T06:00:0{index}.123Z',
                               type='event_msg', payload=event)) + '\n' for index, event in enumerate(events)))

    def read(self, panes=None):
        request = dict(home=str(self.home), panes=panes or [self.pane])
        result = subprocess.run([sys.executable, str(READER), json.dumps(request)],
                                capture_output=True, text=True, check=True, timeout=2)
        return json.loads(result.stdout)

    def test_aborted_turn_is_recovered_and_a_new_turn_cancels_old_evidence(self):
        self.write(dict(type='task_started', turn_id='one'), dict(type='turn_aborted', turn_id='one'))
        rows = self.read()
        self.assertEqual(rows, [dict(session='test', pane=42, event='interrupt', at_ms=1791612001123)])
        self.write(dict(type='turn_aborted'), dict(type='task_started', turn_id='two'))
        self.assertEqual(self.read(), [])

    def test_failed_compaction_is_failed_and_successful_old_turns_are_not_done(self):
        self.write(dict(type='task_complete', error={'message': 'private prompt and directory', 'codex_error_info': 'other'}))
        rows = self.read()
        self.assertEqual(rows[0]['event'], 'stopFailure')
        self.assertNotIn('private', json.dumps(rows))
        self.write(dict(type='task_complete', error=None))
        self.assertEqual(self.read(), [])

    def test_resumed_pane_title_wins_over_old_resume_argument(self):
        self.write(dict(type='turn_aborted'))
        self.pane['title'] = 'A different task | project'
        self.assertEqual(self.read(), [])
        self.pane['title'] = '⠙ Current task | project'
        self.assertEqual(self.read()[0]['event'], 'interrupt')
        self.pane['title'] = '[ ! ] Action Required | Current task | project'
        self.assertEqual(self.read()[0]['event'], 'interrupt')

    def test_duplicate_titles_do_not_guess_a_thread(self):
        self.write(dict(type='turn_aborted'))
        with sqlite3.connect(self.home / 'state_5.sqlite') as db:
            db.execute('INSERT INTO threads VALUES (?,?,?)', ('thread-b', 'Current task', str(self.rollout)))
        self.assertEqual(self.read(), [])

    def test_partial_rollout_and_unknown_schema_fail_open_without_creating_database(self):
        self.write(dict(type='turn_aborted'))
        with self.rollout.open('a') as handle:
            handle.write('{incomplete')
        self.assertEqual(self.read()[0]['event'], 'interrupt')
        with sqlite3.connect(self.home / 'state_5.sqlite') as db:
            db.execute('DROP TABLE threads')
        self.assertEqual(self.read(), [])
        (self.home / 'state_5.sqlite').unlink()
        self.assertEqual(self.read(), [])
        self.assertFalse((self.home / 'state_5.sqlite').exists())

    def test_unknown_records_and_missing_timestamp_do_not_break_recovery(self):
        self.write(dict(type='turn_aborted'))
        with self.rollout.open('a') as handle:
            handle.write('null\n[]\n42\n{"type":"new_format"}\n')
        self.assertEqual(self.read()[0]['event'], 'interrupt')
        self.rollout.write_text('{"type":"event_msg","payload":{"type":"turn_aborted"}}\n')
        self.assertEqual(self.read(), [])

    def test_rollout_outside_codex_sessions_is_rejected(self):
        self.write(dict(type='turn_aborted'))
        outside = self.home / 'unrelated.jsonl'
        self.rollout.rename(outside)
        self.rollout.symlink_to(outside)
        self.assertEqual(self.read(), [])


if __name__ == '__main__':
    unittest.main()
