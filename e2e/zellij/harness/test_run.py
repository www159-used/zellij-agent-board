"""run() reports SetupFailed before any World fact is judged."""

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock
from unittest.mock import patch

from harness.__main__ import _format
from harness.oracle import Verdict
from harness.run import (
    _pane_id_or,
    LiveWorld,
    PtyClient,
    Report,
    client_terminal_panes,
    focused_sentinels,
    list_clients_present,
    run,
)
from harness.scenario import load_recipe


class RunSetup(unittest.TestCase):
    def test_failed_intermediate_phase_prevents_later_actions(self):
        from harness.run import _run_once
        experiment = load_recipe("native-cross-session-switch")
        world = Mock()
        world.materialize.return_value = None
        world.disturb.return_value = None
        world.wait_held.return_value = SimpleNamespace(
            kind=Verdict.BROKEN, hold="on_session", evidence="origin")
        with patch("harness.run.LiveWorld", return_value=world):
            report = _run_once(experiment, "zellij", Path("x"), Path("x"), Path("x"), 1)
        world.disturb.assert_called_once_with(experiment.checkpoints[0].disturb)
        self.assertEqual(report.repeats_held, 0)
        self.assertIn("switch-away", report.evidence)
        self.assertIn('"dest"', report.expectation)
        world.teardown.assert_called_once()

    def test_missing_zellij_is_setup_failed(self) -> None:
        experiment = load_recipe("native-cross-session-switch")
        report = run(experiment, zellij="/no/such/zellij")
        self.assertEqual(report.verdict, Verdict.SETUP_FAILED)
        self.assertEqual(report.setup, "zellij_missing")

    def test_held_report_reads_as_an_experiment_result(self) -> None:
        report = Report(
            name="board-cross-session-jump",
            description="Jump from the board to an Agent in another session.",
            verdict=Verdict.HELD,
            artifacts=Path("/tmp/artifacts"),
            repeats_held=100,
            repeats_requested=100,
        )

        self.assertEqual(
            _format(report),
            "✓ board-cross-session-jump\n"
            "  Jump from the board to an Agent in another session.\n"
            "  held: 100/100 cycles\n"
            "  artifacts: /tmp/artifacts",
        )

    def test_broken_report_explains_expected_and_observed(self) -> None:
        report = Report(
            name="board-cross-session-jump",
            verdict=Verdict.BROKEN,
            hold="on_session",
            expectation='client attached to session "dest"',
            evidence="origin",
            repeats_held=7,
            repeats_requested=100,
        )

        self.assertEqual(
            _format(report),
            "✗ board-cross-session-jump\n"
            "  broken: cycle 8/100\n"
            '  expected: client attached to session "dest"\n'
            "  observed: origin",
        )

    def test_teardown_timeout_does_not_skip_isolate_cleanup(self) -> None:
        world = LiveWorld.__new__(LiveWorld)
        world.pty = None
        world.real_names = {"origin": "ztest0"}
        world.zellij = "zellij"
        world.isolate = Path("/tmp/zabf-test")
        world.sock = world.isolate / "s"
        world.state = world.isolate / "z"
        world.tui = Path("/tmp/board-tui")
        world.session_prefix = "ztest"

        with (
            patch(
                "harness.run.subprocess.run",
                side_effect=subprocess.TimeoutExpired("delete-session", 8),
            ),
            patch("harness.run.shutil.rmtree") as rmtree,
        ):
            world.teardown()

        rmtree.assert_called_once_with(world.isolate, ignore_errors=True)

    def test_pty_write_reports_a_disconnected_terminal(self) -> None:
        client = PtyClient.__new__(PtyClient)
        client.master = 7

        with patch("harness.run.os.write", side_effect=OSError(5, "I/O error")):
            self.assertFalse(client.write(b"\x1bq"))

    def test_go_sends_enter_through_the_attached_client(self) -> None:
        world = LiveWorld.__new__(LiveWorld)
        world.pty = Mock()
        world.pty.write.return_value = True
        world.notes = []
        world._find_tui = Mock(return_value=40)
        step = SimpleNamespace(
            target=SimpleNamespace(session="dest", role="target")
        )

        self.assertIsNone(world._disturb_go(step))

        world.pty.write.assert_called_once_with(b"\r")
        self.assertEqual(world.notes, ["go to Agent: dest.target"])

    def test_zero_is_a_valid_pane_id_without_falling_back(self) -> None:
        fallback_called = False

        def fallback() -> int:
            nonlocal fallback_called
            fallback_called = True
            return 9

        self.assertEqual(_pane_id_or("terminal_0\n", fallback), 0)
        self.assertFalse(fallback_called)

    def test_decoy_marker_is_not_a_coreutils_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            world = LiveWorld.__new__(LiveWorld)
            world.decoy = Path(tmp) / "agent"

            world._write_decoy()

            self.assertTrue(world.decoy.is_file())
            self.assertFalse(world.decoy.is_symlink())

    def test_only_focused_pane_sentinel_is_visible(self) -> None:
        panes = [
            {"id": 3, "is_plugin": False, "is_focused": True},
            {"id": 8, "is_plugin": False, "is_focused": False},
        ]
        screens = {3: "shell", 8: "ZAB-SENTINEL-TARGET"}
        self.assertEqual(
            focused_sentinels(
                panes,
                screens.get,
                frozenset({"ZAB-SENTINEL-TARGET"}),
            ),
            frozenset(),
        )

    def test_checks_every_focused_pane_reported_by_zellij(self) -> None:
        panes = [
            {"id": 3, "is_plugin": False, "is_focused": True},
            {"id": 8, "is_plugin": False, "is_focused": True},
        ]
        screens = {3: "shell", 8: "ZAB-SENTINEL-TARGET"}
        self.assertEqual(
            focused_sentinels(
                panes,
                screens.get,
                frozenset({"ZAB-SENTINEL-TARGET"}),
            ),
            frozenset({"ZAB-SENTINEL-TARGET"}),
        )

    def test_list_clients_identifies_attached_session(self) -> None:
        self.assertTrue(
            list_clients_present(
                "CLIENT_ID ZELLIJ_PANE_ID RUNNING_COMMAND\n"
                "1         terminal_8     /bin/zsh\n"
            )
        )
        self.assertFalse(
            list_clients_present("CLIENT_ID ZELLIJ_PANE_ID RUNNING_COMMAND\n")
        )

    def test_client_focus_comes_from_connected_clients(self) -> None:
        self.assertEqual(
            client_terminal_panes(
                "CLIENT_ID ZELLIJ_PANE_ID RUNNING_COMMAND\n"
                "1         terminal_0     /bin/zsh\n"
                "3         terminal_8     agent\n"
                "4         plugin_5       board\n"
            ),
            frozenset({0, 8}),
        )

    def test_stale_pane_focus_is_not_an_attached_clients_focus(self) -> None:
        world = LiveWorld.__new__(LiveWorld)
        world.real_names = {"home": "test-home"}
        world.sentinels = frozenset({"FIRST", "CURRENT"})
        world._zj = Mock(return_value=SimpleNamespace(stdout="1 terminal_1 agent\n"))
        world._panes = Mock(return_value=[
            {"id": 1, "is_plugin": False, "is_focused": True},
            {"id": 2, "is_plugin": False, "is_focused": True},
        ])
        world._dump = lambda session, pane: {1: "FIRST", 2: "CURRENT"}[pane]
        self.assertEqual(world._visible_sentinels("home"), frozenset({"FIRST"}))


if __name__ == "__main__":
    unittest.main()
