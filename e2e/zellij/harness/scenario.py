"""Experiment values. Scenarios name roles, never pane ids or log paths."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any

import tomllib


@dataclass(frozen=True)
class AgentRef:
    session: str
    role: str


@dataclass(frozen=True)
class Pane:
    role: str
    sentinel: str
    agent: str | None = None


@dataclass(frozen=True)
class Session:
    name: str
    board: bool = False
    panes: tuple[Pane, ...] = ()


@dataclass(frozen=True)
class World:
    attach: str
    sessions: tuple[Session, ...]

    def session(self, name: str) -> Session:
        for session in self.sessions:
            if session.name == name:
                return session
        raise KeyError(name)

    def pane(self, session: str, role: str) -> Pane:
        for pane in self.session(session).panes:
            if pane.role == role:
                return pane
        raise KeyError((session, role))


@dataclass(frozen=True)
class Disturb:
    kind: str
    session: str | None = None
    target: AgentRef | None = None

    @classmethod
    def switch(cls, session: str) -> Disturb:
        return cls(kind="switch", session=session)

    @classmethod
    def go(cls, target: AgentRef) -> Disturb:
        return cls(kind="go", target=target)


@dataclass(frozen=True)
class Hold:
    kind: str
    session: str | None = None
    target: AgentRef | None = None

    @classmethod
    def on_session(cls, session: str) -> Hold:
        return cls(kind="on_session", session=session)

    @classmethod
    def sees(cls, target: AgentRef) -> Hold:
        return cls(kind="sees", target=target)


@dataclass(frozen=True)
class Experiment:
    name: str
    world: World
    disturb: tuple[Disturb, ...]
    description: str = ""
    also: tuple[Hold, ...] = ()
    repeat: int = 1


SCENARIO_DIR = Path(__file__).resolve().parent.parent / "scenarios"


def recipe_names() -> tuple[str, ...]:
    return tuple(sorted(path.stem for path in SCENARIO_DIR.glob("*.toml")))


def load_recipe(name: str) -> Experiment:
    path = SCENARIO_DIR / f"{name}.toml"
    if not path.is_file():
        known = ", ".join(recipe_names()) or "(none)"
        raise FileNotFoundError(f"unknown experiment {name!r}; known: {known}")
    return load_experiment(path)


def load_recipe_cases(name: str) -> tuple[Experiment, ...]:
    path = SCENARIO_DIR / f"{name}.toml"
    if not path.is_file():
        known = ", ".join(recipe_names()) or "(none)"
        raise FileNotFoundError(f"unknown experiment {name!r}; known: {known}")
    return load_experiments(path)


def load_experiment(source: str | Path) -> Experiment:
    experiments = load_experiments(source)
    if len(experiments) != 1:
        raise ValueError(f"document contains {len(experiments)} cases; expected exactly one")
    return experiments[0]


def load_experiments(source: str | Path) -> tuple[Experiment, ...]:
    if isinstance(source, Path):
        text = source.read_text()
    else:
        text = source
    data = tomllib.loads(text)
    rows = data.get("case")
    if rows is None:
        experiments = (_from_table(data),)
    else:
        if not isinstance(rows, list) or not rows:
            raise ValueError("case document must contain at least one [[case]]")
        experiments = tuple(_from_case(row) for row in rows)
    names = [experiment.name for experiment in experiments]
    if len(set(names)) != len(names):
        raise ValueError("case names must be unique within a document")
    for experiment in experiments:
        _check_world(experiment)
    return experiments


def _from_case(row: dict[str, Any]) -> Experiment:
    _known_keys(
        row,
        {"name", "description", "repeat", "given", "target", "targets", "when", "then"},
        "case",
    )
    given = row.get("given") or {}
    when = row.get("when") or {}
    then = row.get("then") or {}
    _known_keys(given, {"attached", "board"}, "case.given")
    _known_keys(then, {"attached", "sees"}, "case.then")

    attached = str(given.get("attached") or "")
    board = str(given.get("board") or "")
    target_rows = []
    if row.get("target") is not None:
        target_rows.append(row["target"])
    target_rows.extend(row.get("targets") or [])

    session_order: list[str] = []
    panes: dict[str, list[Pane]] = {}

    def add_session(name: str) -> None:
        if name and name not in session_order:
            session_order.append(name)

    add_session(attached)
    add_session(board)
    for target_row in target_rows:
        _known_keys(target_row, {"ref", "agent", "shows"}, "case.target")
        target = _agent_ref(target_row.get("ref"))
        add_session(target.session)
        panes.setdefault(target.session, []).append(
            Pane(
                role=target.role,
                sentinel=str(target_row.get("shows") or ""),
                agent=target_row.get("agent"),
            )
        )

    disturb = _disturb(when)
    if disturb.kind == "switch":
        add_session(str(disturb.session or ""))
    elif disturb.target is not None:
        add_session(disturb.target.session)

    holds: list[Hold] = []
    if then.get("attached") is not None:
        holds.append(Hold.on_session(str(then["attached"])))
    sees = then.get("sees")
    for value in sees if isinstance(sees, list) else ([sees] if sees is not None else []):
        holds.append(Hold.sees(_agent_ref(value)))

    sessions = tuple(
        Session(
            name=name,
            board=name == board,
            panes=tuple(panes.get(name, [])),
        )
        for name in session_order
    )
    return Experiment(
        name=str(row.get("name") or ""),
        description=str(row.get("description") or ""),
        world=World(attach=attached, sessions=sessions),
        disturb=(disturb,),
        also=tuple(holds),
        repeat=int(row.get("repeat") or 1),
    )


def _known_keys(table: Any, known: set[str], location: str) -> None:
    if not isinstance(table, dict):
        raise ValueError(f"{location} must be a table")
    unknown = set(table) - known
    if unknown:
        names = ", ".join(sorted(unknown))
        raise ValueError(f"unknown {location} field(s): {names}")


def _from_table(data: dict[str, Any]) -> Experiment:
    world_raw = data.get("world") or {}
    attach = str(world_raw.get("attach") or "")
    legacy_panes = list(world_raw.get("panes") or [])
    session_rows = world_raw.get("sessions") or []
    if isinstance(session_rows, dict):
        named_rows = session_rows.items()
    else:
        named_rows = (
            (str(row.get("name") or ""), row)
            for row in session_rows
            if isinstance(row, dict)
        )
    sessions = []
    for name, row in named_rows:
        if not isinstance(row, dict):
            raise ValueError(f"session {name!r} must be a table")
        pane_rows = list(row.get("panes") or [])
        pane_rows.extend(pane for pane in legacy_panes if pane.get("session") == name)
        panes = tuple(
            Pane(
                role=str(pane.get("role") or ""),
                sentinel=str(pane.get("sentinel") or ""),
                agent=pane.get("agent"),
            )
            for pane in pane_rows
        )
        sessions.append(Session(name=name, board=bool(row.get("board")), panes=panes))
    disturb = tuple(_disturb(item) for item in data.get("disturb") or [])
    also = tuple(_hold(item) for item in data.get("hold") or [])
    return Experiment(
        name=str(data.get("name") or ""),
        description=str(data.get("description") or ""),
        world=World(attach=attach, sessions=tuple(sessions)),
        disturb=disturb,
        also=also,
        repeat=int(data.get("repeat") or 1),
    )


def _disturb(item: dict[str, Any]) -> Disturb:
    return _variant(item, "disturb", _DISTURB_LOADERS)


def _hold(item: dict[str, Any]) -> Hold:
    return _variant(item, "hold", _HOLD_LOADERS)


def _variant(
    item: dict[str, Any],
    family: str,
    loaders: dict[str, Any],
) -> Any:
    if not isinstance(item, dict) or len(item) != 1:
        raise ValueError(f"each {family} must contain exactly one kind")
    kind, value = next(iter(item.items()))
    loader = loaders.get(kind)
    if loader is None:
        choices = ", ".join(sorted(loaders))
        raise ValueError(f"unknown {family} {kind!r}; known: {choices}")
    return loader(value)


def _agent_ref(value: Any) -> AgentRef:
    if isinstance(value, str):
        session, separator, role = value.partition(".")
        if not separator or not session or not role:
            raise ValueError(f"Agent reference {value!r} must be session.role")
        return AgentRef(session, role)
    if isinstance(value, dict):
        return AgentRef(str(value.get("session") or ""), str(value.get("role") or ""))
    raise ValueError("Agent reference must be session.role or a table")


# A new declarative kind is added in one parser registry and one runner adapter.
_DISTURB_LOADERS = {
    "switch": lambda value: Disturb.switch(str(value)),
    "go": lambda value: Disturb.go(_agent_ref(value)),
}

_HOLD_LOADERS = {
    "on_session": lambda value: Hold.on_session(str(value)),
    "sees": lambda value: Hold.sees(_agent_ref(value)),
}


def _check_world(experiment: Experiment) -> None:
    names = [session.name for session in experiment.world.sessions]
    if not names:
        raise ValueError("world must declare sessions")
    if len(set(names)) != len(names):
        raise ValueError("session names must be unique")
    if experiment.world.attach not in names:
        raise ValueError("attach must name a declared session")
    if not experiment.disturb:
        raise ValueError("disturb must not be empty")
    attach = experiment.world.session(experiment.world.attach)
    for step in experiment.disturb:
        if step.kind == "go" and not attach.board:
            raise ValueError("Disturb::Go requires the attached session to have a board")
        if step.kind == "go" and step.target is not None:
            pane = experiment.world.pane(step.target.session, step.target.role)
            if not pane.agent:
                raise ValueError("Go target must name an Agent pane")
        if step.kind == "switch" and step.session not in names:
            raise ValueError("Switch target must be a declared session")
    for hold in experiment.also:
        if hold.kind == "on_session" and hold.session not in names:
            raise ValueError(
                f"Hold::OnSession must name a declared session: {hold.session}"
            )
        if hold.kind == "sees" and hold.target is not None:
            try:
                experiment.world.pane(hold.target.session, hold.target.role)
            except KeyError:
                target = f"{hold.target.session}.{hold.target.role}"
                raise ValueError(
                    f"Hold::Sees must name a declared pane: {target}"
                ) from None
