"""Judge Holds from World facts. Zellij log noise is classified, not trusted."""

from __future__ import annotations

from dataclasses import dataclass, field


class Verdict:
    HELD = "held"
    BROKEN = "broken"
    SETUP_FAILED = "setup_failed"


@dataclass(frozen=True)
class Observation:
    client_alive: bool
    sessions: frozenset[str]
    control_alive: bool
    log_lines: tuple[str, ...] = ()
    attached_session: str | None = None
    visible_sentinels: frozenset[str] = field(default_factory=frozenset)
    required_sentinels: frozenset[str] = field(default_factory=frozenset)
    expected_sessions: frozenset[str] = field(default_factory=frozenset)
    required_session: str | None = None
    server_restarted: bool = False
    explained_reconnect: bool = False


@dataclass(frozen=True)
class Judgement:
    kind: str
    hold: str | None = None
    evidence: str | None = None


_CLIPIPE_TIMEOUT = "Action CliPipe did not complete within 1s timeout"
_STARTING_CLIENT = "Starting Zellij client!"
_LOST_CONNECTION = "Lost connection"


def judge(observation: Observation) -> Judgement:
    if not observation.client_alive:
        return Judgement(kind=Verdict.BROKEN, hold="client_alive")
    if not observation.control_alive:
        return Judgement(kind=Verdict.BROKEN, hold="control_alive")
    if observation.server_restarted and not observation.explained_reconnect:
        return Judgement(kind=Verdict.BROKEN, hold="server_stable")
    missing = observation.expected_sessions - observation.sessions
    if missing:
        return Judgement(
            kind=Verdict.BROKEN,
            hold="session_exists",
            evidence=",".join(sorted(missing)),
        )
    if (
        observation.required_session is not None
        and observation.attached_session != observation.required_session
    ):
        return Judgement(
            kind=Verdict.BROKEN,
            hold="on_session",
            evidence=observation.attached_session,
        )
    unseen = observation.required_sentinels - observation.visible_sentinels
    if unseen:
        return Judgement(
            kind=Verdict.BROKEN,
            hold="sees",
            evidence=",".join(sorted(unseen)),
        )
    if _unexplained_lost_connection(observation):
        return Judgement(
            kind=Verdict.BROKEN,
            hold="lost_connection",
            evidence=_LOST_CONNECTION,
        )
    return Judgement(kind=Verdict.HELD)


def _unexplained_lost_connection(observation: Observation) -> bool:
    if not any(_LOST_CONNECTION in line for line in observation.log_lines):
        return False
    return not observation.explained_reconnect
