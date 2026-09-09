"""Holds are World facts. Log noise is not a Break by itself."""

from __future__ import annotations

import unittest

from harness.oracle import Observation, Verdict, judge


class JudgeHolds(unittest.TestCase):
    def test_clipipe_timeout_alone_is_held(self) -> None:
        observation = Observation(
            client_alive=True,
            sessions=frozenset({"origin", "dest"}),
            control_alive=True,
            log_lines=("Action CliPipe did not complete within 1s timeout",),
        )
        self.assertEqual(judge(observation).kind, Verdict.HELD)

    def test_lost_connection_without_transition_is_broken(self) -> None:
        observation = Observation(
            client_alive=True,
            sessions=frozenset({"origin"}),
            control_alive=True,
            log_lines=("Lost connection to the server",),
        )
        judgement = judge(observation)
        self.assertEqual(judgement.kind, Verdict.BROKEN)
        self.assertEqual(judgement.hold, "lost_connection")

    def test_client_exit_is_broken(self) -> None:
        observation = Observation(
            client_alive=False,
            sessions=frozenset({"origin"}),
            control_alive=True,
        )
        judgement = judge(observation)
        self.assertEqual(judgement.kind, Verdict.BROKEN)
        self.assertEqual(judgement.hold, "client_alive")

    def test_explained_reconnect_is_held(self) -> None:
        observation = Observation(
            client_alive=True,
            sessions=frozenset({"origin", "dest"}),
            control_alive=True,
            attached_session="dest",
            explained_reconnect=True,
            log_lines=(
                "Starting Zellij client!",
                "Lost connection to the server",
            ),
        )
        self.assertEqual(judge(observation).kind, Verdict.HELD)

    def test_missing_session_is_broken(self) -> None:
        observation = Observation(
            client_alive=True,
            sessions=frozenset({"origin"}),
            expected_sessions=frozenset({"origin", "dest"}),
            control_alive=True,
        )
        judgement = judge(observation)
        self.assertEqual(judgement.kind, Verdict.BROKEN)
        self.assertEqual(judgement.hold, "session_exists")

    def test_control_plane_deaf_is_broken(self) -> None:
        observation = Observation(
            client_alive=True,
            sessions=frozenset({"origin"}),
            control_alive=False,
        )
        judgement = judge(observation)
        self.assertEqual(judgement.kind, Verdict.BROKEN)
        self.assertEqual(judgement.hold, "control_alive")

    def test_unexpected_restart_is_broken(self) -> None:
        observation = Observation(
            client_alive=True,
            sessions=frozenset({"origin"}),
            control_alive=True,
            server_restarted=True,
        )
        judgement = judge(observation)
        self.assertEqual(judgement.kind, Verdict.BROKEN)
        self.assertEqual(judgement.hold, "server_stable")

    def test_missing_sentinel_is_broken(self) -> None:
        observation = Observation(
            client_alive=True,
            sessions=frozenset({"dest"}),
            control_alive=True,
            attached_session="dest",
            required_sentinels=frozenset({"ZAB-SENTINEL-DEST"}),
            visible_sentinels=frozenset(),
        )
        judgement = judge(observation)
        self.assertEqual(judgement.kind, Verdict.BROKEN)
        self.assertEqual(judgement.hold, "sees")

    def test_wrong_attached_session_is_broken(self) -> None:
        observation = Observation(
            client_alive=True,
            sessions=frozenset({"origin", "dest"}),
            control_alive=True,
            attached_session="origin",
            required_session="dest",
        )
        judgement = judge(observation)
        self.assertEqual(judgement.kind, Verdict.BROKEN)
        self.assertEqual(judgement.hold, "on_session")


if __name__ == "__main__":
    unittest.main()
