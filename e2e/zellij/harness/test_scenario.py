"""Scenarios declare World, Disturb, and extra Holds — never pane ids."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from harness.scenario import AgentRef, Disturb, Hold, load_experiment, load_experiments


CROSS_SESSION_JUMP = """
name = "board-cross-session-jump"
repeat = 3

[world]
attach = "origin"

[[world.sessions]]
name = "origin"
board = true

[[world.sessions]]
name = "dest"

[[world.panes]]
session = "dest"
role = "target"
agent = "cursor"
sentinel = "ZAB-SENTINEL-DEST"

[[disturb]]
go = { session = "dest", role = "target" }

[[hold]]
on_session = "dest"

[[hold]]
sees = { session = "dest", role = "target" }
"""

READABLE_CROSS_SESSION_JUMP = """
name = "board-cross-session-jump"
description = "Jump from the board to an Agent in another session."
repeat = 3

[world]
attach = "origin"

[world.sessions.origin]
board = true

[[world.sessions.dest.panes]]
role = "target"
agent = "cursor"
sentinel = "ZAB-SENTINEL-DEST"

[[disturb]]
go = "dest.target"

[[hold]]
on_session = "dest"

[[hold]]
sees = "dest.target"
"""

CASE_DOCUMENT = """
[[case]]
name = "board-cross-session-jump"
description = "Jump from the board to an Agent in another session."
repeat = 100
given = { attached = "origin", board = "origin" }
target = { ref = "dest.target", agent = "cursor", shows = "ZAB-SENTINEL-DEST" }
when = { go = "dest.target" }
then = { attached = "dest", sees = "dest.target" }

[[case]]
name = "native-cross-session-switch"
description = "Switch sessions without involving the board."
repeat = 5
given = { attached = "origin" }
target = { ref = "dest.target", shows = "ZAB-SENTINEL-DEST" }
when = { switch = "dest" }
then = { attached = "dest", sees = "dest.target" }
"""


class LoadExperiment(unittest.TestCase):
    def test_loads_cross_session_jump(self) -> None:
        experiment = load_experiment(CROSS_SESSION_JUMP)
        self.assertEqual(experiment.name, "board-cross-session-jump")
        self.assertEqual(experiment.repeat, 3)
        self.assertEqual(experiment.world.attach, "origin")
        self.assertEqual([session.name for session in experiment.world.sessions], ["origin", "dest"])
        self.assertTrue(experiment.world.session("origin").board)
        self.assertFalse(experiment.world.session("dest").board)
        pane = experiment.world.pane("dest", "target")
        self.assertEqual(pane.agent, "cursor")
        self.assertEqual(pane.sentinel, "ZAB-SENTINEL-DEST")
        self.assertEqual(
            experiment.disturb,
            (Disturb.go(AgentRef("dest", "target")),),
        )
        self.assertEqual(
            experiment.also,
            (
                Hold.on_session("dest"),
                Hold.sees(AgentRef("dest", "target")),
            ),
        )

    def test_loads_readable_nested_world_and_refs(self) -> None:
        experiment = load_experiment(READABLE_CROSS_SESSION_JUMP)

        self.assertEqual(
            experiment.description,
            "Jump from the board to an Agent in another session.",
        )
        self.assertEqual(
            [session.name for session in experiment.world.sessions],
            ["origin", "dest"],
        )
        self.assertTrue(experiment.world.session("origin").board)
        self.assertEqual(
            experiment.disturb,
            (Disturb.go(AgentRef("dest", "target")),),
        )
        self.assertEqual(
            experiment.also,
            (
                Hold.on_session("dest"),
                Hold.sees(AgentRef("dest", "target")),
            ),
        )

    def test_case_keeps_coupled_setup_action_and_holds_together(self) -> None:
        experiment = load_experiments(CASE_DOCUMENT)[0]

        self.assertEqual(experiment.name, "board-cross-session-jump")
        self.assertEqual(experiment.repeat, 100)
        self.assertEqual(experiment.world.attach, "origin")
        self.assertTrue(experiment.world.session("origin").board)
        pane = experiment.world.pane("dest", "target")
        self.assertEqual(pane.agent, "cursor")
        self.assertEqual(pane.sentinel, "ZAB-SENTINEL-DEST")
        self.assertEqual(
            experiment.disturb,
            (Disturb.go(AgentRef("dest", "target")),),
        )
        self.assertEqual(
            experiment.also,
            (
                Hold.on_session("dest"),
                Hold.sees(AgentRef("dest", "target")),
            ),
        )

    def test_one_document_can_contain_multiple_cases(self) -> None:
        experiments = load_experiments(CASE_DOCUMENT)

        self.assertEqual(
            [experiment.name for experiment in experiments],
            [
                "board-cross-session-jump",
                "native-cross-session-switch",
            ],
        )

    def test_load_experiment_rejects_ambiguous_multi_case_document(self) -> None:
        with self.assertRaisesRegex(ValueError, "contains 2 cases"):
            load_experiment(CASE_DOCUMENT)

    def test_unknown_disturb_names_the_extension_point(self) -> None:
        text = READABLE_CROSS_SESSION_JUMP.replace(
            'go = "dest.target"',
            'teleport = "dest.target"',
        )

        with self.assertRaisesRegex(ValueError, "unknown disturb 'teleport'"):
            load_experiment(text)

    def test_agent_ref_explains_expected_shape(self) -> None:
        text = READABLE_CROSS_SESSION_JUMP.replace(
            'go = "dest.target"',
            'go = "dest"',
        )

        with self.assertRaisesRegex(ValueError, "session.role"):
            load_experiment(text)

    def test_sees_hold_must_name_a_declared_pane(self) -> None:
        text = READABLE_CROSS_SESSION_JUMP.replace(
            'sees = "dest.target"',
            'sees = "dest.typo"',
        )

        with self.assertRaisesRegex(ValueError, r"Hold::Sees.*dest\.typo"):
            load_experiment(text)

    def test_on_session_hold_must_name_a_declared_session(self) -> None:
        text = READABLE_CROSS_SESSION_JUMP.replace(
            'on_session = "dest"',
            'on_session = "typo"',
        )

        with self.assertRaisesRegex(ValueError, r"Hold::OnSession.*typo"):
            load_experiment(text)

    def test_go_without_board_is_impossible(self) -> None:
        text = """
name = "bad"
[world]
attach = "home"
[[world.sessions]]
name = "home"
[[world.panes]]
session = "home"
role = "target"
agent = "cursor"
sentinel = "X"
[[disturb]]
go = { session = "home", role = "target" }
"""
        with self.assertRaisesRegex(ValueError, "board"):
            load_experiment(text)

    def test_checked_in_recipes_load(self) -> None:
        from harness.scenario import load_recipe, recipe_names

        self.assertEqual(
            set(recipe_names()),
            {
                "native-cross-session-switch",
                "board-same-session-focus",
                "board-cross-session-jump",
            },
        )
        jump = load_recipe("board-cross-session-jump")
        self.assertEqual(jump.world.attach, "origin")
        self.assertTrue(jump.world.session("origin").board)

    def test_load_path_reads_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "one.toml"
            path.write_text(CROSS_SESSION_JUMP)
            self.assertEqual(load_experiment(path).name, "board-cross-session-jump")


if __name__ == "__main__":
    unittest.main()
