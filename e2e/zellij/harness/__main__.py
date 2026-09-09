"""Thin CLI: one Experiment name (or all recipes), optional repeat count."""

from __future__ import annotations

import sys

from harness.oracle import Verdict
from harness.run import run
from harness.scenario import load_recipe_cases, recipe_names


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    repeats = None
    if args and args[-1].isdigit():
        repeats = int(args.pop())
        if repeats < 1:
            print("zab-fault: repeats must be >= 1", file=sys.stderr)
            return 2
    if args:
        names = args
    else:
        names = list(recipe_names())
    any_broken = False
    any_setup = False
    for name in names:
        try:
            experiments = load_recipe_cases(name)
        except FileNotFoundError as exc:
            print(f"zab-fault: {exc}", file=sys.stderr)
            return 2
        for experiment in experiments:
            report = run(experiment, repeats=repeats)
            print(_format(report))
            if report.verdict == Verdict.BROKEN:
                any_broken = True
            elif report.verdict == Verdict.SETUP_FAILED:
                any_setup = True
    if any_broken:
        return 1
    if any_setup:
        return 2
    return 0


def _format(report) -> str:
    symbol = {
        Verdict.HELD: "✓",
        Verdict.BROKEN: "✗",
        Verdict.SETUP_FAILED: "!",
    }[report.verdict]
    lines = [f"{symbol} {report.name}"]
    if report.description:
        lines.append(f"  {report.description}")
    if report.verdict == Verdict.HELD:
        lines.append(
            f"  held: {report.repeats_held}/{report.repeats_requested} cycles"
        )
    elif report.verdict == Verdict.BROKEN:
        lines.append(
            f"  broken: cycle {report.repeats_held + 1}/{report.repeats_requested}"
        )
        if report.expectation:
            lines.append(f"  expected: {report.expectation}")
        lines.append(f"  observed: {report.evidence or report.hold or 'unknown'}")
    else:
        lines.append(
            f"  setup failed before cycle {report.repeats_held + 1}: "
            f"{report.setup or 'unknown'}"
        )
    if report.artifacts:
        lines.append(f"  artifacts: {report.artifacts}")
    return "\n".join(lines)


if __name__ == "__main__":
    raise SystemExit(main())
