"""run() reports SetupFailed before any World fact is judged."""

from __future__ import annotations

import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

from harness.__main__ import _format
from harness.oracle import Verdict
from harness.run import (
    LiveWorld,
    PtyClient,
    Report,
    focused_sentinels,
    list_clients_present,
    run,
)
from harness.scenario import load_recipe


class RunSetup(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
