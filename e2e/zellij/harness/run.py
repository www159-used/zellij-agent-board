"""Run an Experiment. Callers never see sockets, PTYs, or log paths."""

from __future__ import annotations

import fcntl
import json
import os
import pty
import re
import secrets
import select
import shlex
import shutil
import struct
import subprocess
import tempfile
import termios
import threading
import time
from dataclasses import dataclass
from pathlib import Path

from harness.oracle import Observation, Verdict, judge
from harness.scenario import Disturb, Experiment

REPO = Path(__file__).resolve().parents[3]
DEFAULT_WASM = REPO / "target/wasm32-wasip1/release/zellij-agent-board.wasm"
DEFAULT_TUI = REPO / "target/release/board-tui"


@dataclass(frozen=True)
class Report:
    name: str
    verdict: str
    description: str = ""
    hold: str | None = None
    expectation: str | None = None
    evidence: str | None = None
    setup: str | None = None
    artifacts: Path | None = None
    repeats_held: int = 0
    repeats_requested: int = 0


def run(
    experiment: Experiment,
    *,
    zellij: str | None = None,
    wasm: Path | None = None,
    tui: Path | None = None,
    artifacts: Path | None = None,
    repeats: int | None = None,
) -> Report:
    tries = max(1, repeats if repeats is not None else experiment.repeat)
    binary = _resolve_zellij(zellij or os.environ.get("ZAB_FAULT_ZELLIJ"))
    if binary is None:
        return Report(
            name=experiment.name,
            description=experiment.description,
            verdict=Verdict.SETUP_FAILED,
            setup="zellij_missing",
            repeats_requested=tries,
        )
    wasm = wasm or DEFAULT_WASM
    tui = tui or DEFAULT_TUI
    if _needs_board(experiment) and (not wasm.is_file() or not tui.is_file()):
        return Report(
            name=experiment.name,
            description=experiment.description,
            verdict=Verdict.SETUP_FAILED,
            setup="artifact_missing",
            repeats_requested=tries,
        )
    root = Path(artifacts) if artifacts else REPO / "target" / "zab-fault" / experiment.name
    root.mkdir(parents=True, exist_ok=True)
    trial = root / "01"
    if trial.exists():
        shutil.rmtree(trial)
    trial.mkdir(parents=True)
    return _run_once(experiment, binary, wasm, tui, trial, tries)


def _resolve_zellij(zellij: str | None) -> str | None:
    if zellij:
        path = Path(zellij)
        if path.is_file() and os.access(path, os.X_OK):
            return str(path.resolve())
        return shutil.which(zellij)
    return shutil.which("zellij")


def _needs_board(experiment: Experiment) -> bool:
    return any(session.board for session in experiment.world.sessions) or any(
        step.kind == "go" for step in experiment.disturb
    )


def _explains_reconnect(experiment: Experiment) -> bool:
    attach = experiment.world.attach
    for step in experiment.disturb:
        if step.kind == "switch":
            return True
        if step.kind == "go" and step.target and step.target.session != attach:
            return True
    return False


def _run_once(
    experiment: Experiment,
    zellij: str,
    wasm: Path,
    tui: Path,
    trial: Path,
    cycles: int,
) -> Report:
    world = LiveWorld(experiment, zellij, wasm, tui, trial)
    try:
        setup = world.materialize()
        if setup is not None:
            world.bundle(setup)
            return Report(
                name=experiment.name,
                description=experiment.description,
                verdict=Verdict.SETUP_FAILED,
                setup=setup,
                artifacts=trial,
                repeats_requested=cycles,
            )
        held = 0
        for cycle in range(1, cycles + 1):
            world.notes.append(f"cycle {cycle}/{cycles}")
            disturb = world.disturb()
            if disturb is not None:
                return _runtime_failure_report(
                    experiment, world, trial, cycles, held, disturb
                )
            judgement = world.wait_held()
            if judgement.kind != Verdict.HELD:
                world.bundle(judgement.kind, judgement.hold, judgement.evidence)
                return Report(
                    name=experiment.name,
                    description=experiment.description,
                    verdict=judgement.kind,
                    hold=judgement.hold,
                    expectation=_expectation(experiment, judgement.hold),
                    evidence=judgement.evidence,
                    artifacts=trial,
                    repeats_held=held,
                    repeats_requested=cycles,
                )
            held += 1
            if cycle < cycles:
                restored = world.restore()
                if restored is not None:
                    return _runtime_failure_report(
                        experiment, world, trial, cycles, held, restored
                    )
        world.bundle(Verdict.HELD)
        return Report(
            name=experiment.name,
            description=experiment.description,
            verdict=Verdict.HELD,
            artifacts=trial,
            repeats_held=held,
            repeats_requested=cycles,
        )
    finally:
        world.teardown()


def _runtime_failure_report(
    experiment: Experiment,
    world: LiveWorld,
    trial: Path,
    cycles: int,
    held: int,
    failure: str,
) -> Report:
    if failure == "client_lost":
        hold = "client_alive"
        evidence = "PTY disconnected during experiment"
        world.bundle(Verdict.BROKEN, hold, evidence)
        return Report(
            name=experiment.name,
            description=experiment.description,
            verdict=Verdict.BROKEN,
            hold=hold,
            expectation=_expectation(experiment, hold),
            evidence=evidence,
            artifacts=trial,
            repeats_held=held,
            repeats_requested=cycles,
        )
    world.bundle(failure)
    return Report(
        name=experiment.name,
        description=experiment.description,
        verdict=Verdict.SETUP_FAILED,
        setup=failure,
        artifacts=trial,
        repeats_held=held,
        repeats_requested=cycles,
    )


def _expectation(experiment: Experiment, hold: str | None) -> str | None:
    fixed = {
        "client_alive": "PTY client remains alive",
        "control_alive": "Zellij control plane responds",
        "server_stable": "session servers are not replaced",
        "session_exists": "all declared sessions still exist",
        "lost_connection": "no unexplained client disconnect",
    }
    if hold in fixed:
        return fixed[hold]
    if hold == "on_session":
        expected = next(
            (item.session for item in experiment.also if item.kind == hold),
            None,
        )
        return f'client attached to session "{expected}"' if expected else None
    if hold == "sees":
        target = next(
            (item.target for item in experiment.also if item.kind == hold),
            None,
        )
        if target is not None:
            pane = experiment.world.pane(target.session, target.role)
            return f'focused pane shows "{pane.sentinel}"'
    return None


class PtyClient:
    def __init__(self, argv: list[str], env: dict[str, str]) -> None:
        master, slave = pty.openpty()
        _set_winsize(master, 32, 120)
        self.proc = subprocess.Popen(
            argv,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            start_new_session=True,
        )
        os.close(slave)
        self.master = master
        self.buf = bytearray()
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._read, daemon=True)
        self._thread.start()

    def _read(self) -> None:
        while not self._stop.is_set():
            try:
                ready, _, _ = select.select([self.master], [], [], 0.2)
            except (OSError, ValueError):
                break
            if not ready:
                if self.proc.poll() is not None:
                    break
                continue
            try:
                data = os.read(self.master, 4096)
            except OSError:
                break
            if not data:
                break
            with self._lock:
                self.buf.extend(data)

    def write(self, data: bytes) -> bool:
        try:
            os.write(self.master, data)
        except OSError:
            return False
        return True

    def alive(self) -> bool:
        return self.proc.poll() is None

    def transcript(self) -> str:
        with self._lock:
            return self.buf.decode("utf-8", "replace")

    def close(self) -> None:
        self._stop.set()
        if self.alive():
            self.proc.terminate()
            try:
                self.proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        try:
            os.close(self.master)
        except OSError:
            pass


def _set_winsize(fd: int, rows: int, cols: int) -> None:
    packed = struct.pack("HHHH", rows, cols, 0, 0)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, packed)


class LiveWorld:
    def __init__(
        self,
        experiment: Experiment,
        zellij: str,
        wasm: Path,
        tui: Path,
        trial: Path,
    ) -> None:
        self.experiment = experiment
        self.zellij = zellij
        self.wasm = wasm
        self.tui = tui
        self.trial = trial
        # Zellij IPC sockets cap at ~103 bytes. Keep the isolate under /tmp.
        self.isolate = Path(tempfile.mkdtemp(prefix="zabf-", dir="/tmp"))
        self.sock = self.isolate / "s"
        self.state = self.isolate / "z"
        self.sock.mkdir()
        self.state.mkdir()
        self.config = self.isolate / "config.kdl"
        self.decoy = self.isolate / "agent"
        self.real_names: dict[str, str] = {}
        self.pane_ids: dict[tuple[str, str], int] = {}
        self.pty: PtyClient | None = None
        self.server_cookie: dict[str, int] | None = None
        self.notes: list[str] = []
        token = secrets.token_hex(2)
        self.session_prefix = f"z{token}"
        for index, session in enumerate(experiment.world.sessions):
            self.real_names[session.name] = f"{self.session_prefix}{index}"

    @property
    def env(self) -> dict[str, str]:
        env = os.environ.copy()
        env["TMPDIR"] = str(self.isolate)
        env["ZELLIJ_SOCKET_DIR"] = str(self.sock)
        env["ZAB_STATE_DIR"] = str(self.state)
        env["ZAB_ZELLIJ"] = self.zellij
        env["ZAB_SCAN_SESSION_PREFIX"] = self.session_prefix
        env["ZELLIJ_AGENT_BOARD_TUI"] = str(self.tui)
        env.pop("ZELLIJ", None)
        env.pop("ZELLIJ_SESSION_NAME", None)
        return env

    def materialize(self) -> str | None:
        self._write_config()
        self._write_decoy()
        for session in self.experiment.world.sessions:
            real = self.real_names[session.name]
            created = self._zj(
                "--config",
                str(self.config),
                "attach",
                "--create-background",
                real,
            )
            if created.returncode != 0:
                self.notes.append(
                    f"create {real} rc={created.returncode}\n{created.stdout}\n{created.stderr}"
                )
                return "world_never_quiet"
            if not self._wait_session(real):
                return "world_never_quiet"
            for pane in session.panes:
                planted = self._plant(session.name, pane.role, pane.sentinel)
                if planted is None:
                    return "world_never_quiet"
                self.pane_ids[(session.name, pane.role)] = planted
                self._zj(
                    "action",
                    "write",
                    "--pane-id",
                    f"terminal_{planted}",
                    "--",
                    "13",
                    session=real,
                )
        self._seed_scan()
        attach = self.real_names[self.experiment.world.attach]
        self.pty = PtyClient(
            [self.zellij, "--config", str(self.config), "attach", attach],
            self.env,
        )
        if not self._wait(lambda: self.pty is not None and self.pty.alive(), 5):
            return "world_never_quiet"
        if not self._wait(lambda: self._attached_logical() == self.experiment.world.attach, 8):
            return "world_never_quiet"
        self.server_cookie = self._server_cookie()
        if _needs_board(self.experiment):
            opened = self._open_board()
            if opened is not None:
                return opened
        if not self._wait(self._agents_ready, 8):
            return "world_never_quiet"
        return None

    def disturb(self) -> str | None:
        for step in self.experiment.disturb:
            adapter_name = _DISTURB_ADAPTERS.get(step.kind)
            if adapter_name is None:
                return f"unsupported_disturb:{step.kind}"
            failed = getattr(self, adapter_name)(step)
            if failed is not None:
                return failed
        return None

    def _disturb_switch(self, step: Disturb) -> str | None:
        if self.pty is None or not self.pty.alive() or step.session is None:
            return "world_never_quiet"
        target = self.real_names[step.session]
        pane = self._first_pane(step.session)
        if pane is not None:
            focused = self._zj(
                "action",
                "focus-pane-id",
                f"terminal_{pane}",
                session=target,
            )
            if focused.returncode != 0:
                return "world_never_quiet"
        if not self.pty.write(b"\x1bs"):
            return "client_lost"
        self.notes.append(f"switch session: {step.session} (pane role id {pane})")
        return None

    def _disturb_go(self, step: Disturb) -> str | None:
        if step.target is None:
            return "world_never_quiet"
        tui = self._find_tui()
        if tui is None:
            return "permission_denied"
        enter = self._zj(
            "action",
            "write",
            "--pane-id",
            f"terminal_{tui}",
            "--",
            "13",
            session=self.real_names[self.experiment.world.attach],
        )
        if enter.returncode != 0:
            return "world_never_quiet"
        self.notes.append(
            f"go to Agent: {step.target.session}.{step.target.role}"
        )
        return None

    def restore(self) -> str | None:
        if self.pty is None or not self.pty.alive():
            return "world_never_quiet"
        origin = self.experiment.world.attach
        if self._attached_logical() != origin:
            if not self.pty.write(b"\x1br"):
                return "client_lost"
            if not self._wait(lambda: self._attached_logical() == origin, 8):
                return "world_never_quiet"
            # The title changes before the reconnected client is ready to
            # consume another key. Do not let restore lose the next Alt+q.
            time.sleep(0.25)
        if _needs_board(self.experiment):
            return self._open_board()
        return None

    def wait_held(self, timeout: float = 10.0):
        deadline = time.monotonic() + timeout
        last = judge(self.observe())
        while time.monotonic() < deadline:
            last = judge(self.observe())
            if last.kind == Verdict.HELD:
                return last
            if last.hold in {"client_alive", "control_alive", "server_stable"}:
                return last
            time.sleep(0.2)
        return last

    def observe(self) -> Observation:
        required_session = None
        required_sentinels: set[str] = set()
        for hold in self.experiment.also:
            if hold.kind == "on_session":
                required_session = hold.session
            if hold.kind == "sees" and hold.target:
                pane = self.experiment.world.pane(hold.target.session, hold.target.role)
                required_sentinels.add(pane.sentinel)
        return Observation(
            client_alive=self.pty is not None and self.pty.alive(),
            sessions=self._live_logical(),
            expected_sessions=frozenset(self.real_names),
            control_alive=self._control_alive(),
            attached_session=self._attached_logical(),
            required_session=required_session,
            visible_sentinels=self._visible_sentinels(),
            required_sentinels=frozenset(required_sentinels),
            server_restarted=self._server_restarted(),
            explained_reconnect=_explains_reconnect(self.experiment),
            log_lines=self._log_lines(),
        )

    def bundle(self, verdict: str, hold: str | None = None, evidence: str | None = None) -> None:
        expectation = _expectation(self.experiment, hold)
        (self.trial / "outcome.txt").write_text(
            f"verdict={verdict}\nhold={hold or ''}\nevidence={evidence or ''}\n"
        )
        (self.trial / "outcome.json").write_text(
            json.dumps(
                {
                    "experiment": self.experiment.name,
                    "description": self.experiment.description,
                    "verdict": verdict,
                    "broken_hold": hold,
                    "expected": expectation,
                    "observed": evidence,
                    "timeline": self.notes,
                },
                indent=2,
            )
            + "\n"
        )
        summary = [
            f"Experiment: {self.experiment.name}",
            f"Purpose: {self.experiment.description or '(not documented)'}",
            f"Verdict: {verdict}",
        ]
        if expectation:
            summary.append(f"Expected: {expectation}")
        if hold:
            summary.append(f"Broken Hold: {hold}")
        if evidence:
            summary.append(f"Observed: {evidence}")
        if self.notes:
            summary.extend(["", "Timeline:", *(f"- {note}" for note in self.notes)])
        (self.trial / "summary.txt").write_text("\n".join(summary) + "\n")
        if self.notes:
            (self.trial / "notes.txt").write_text("\n\n".join(self.notes) + "\n")
        (self.trial / "isolate.env").write_text(
            f"TMPDIR={self.isolate}\nZELLIJ_SOCKET_DIR={self.sock}\nZAB_STATE_DIR={self.state}\n"
        )
        if self.pty is not None:
            (self.trial / "pty.txt").write_text(self.pty.transcript())
        scan = self.state / "scan"
        if scan.is_file():
            shutil.copy2(scan, self.trial / "scan")
        copied_logs = []
        for index, path in enumerate(sorted(self.isolate.rglob("*.log"))):
            destination = self.trial / f"zellij-{index:02d}-{path.name}"
            try:
                shutil.copy2(path, destination)
                copied_logs.append(f"{destination.name} <- {path.relative_to(self.isolate)}")
            except OSError:
                continue
        if copied_logs:
            (self.trial / "zellij-logs.txt").write_text("\n".join(copied_logs) + "\n")
        sessions = self._zj("list-sessions", "-n")
        (self.trial / "sessions.txt").write_text(sessions.stdout + sessions.stderr)
        processes = subprocess.run(
            ["/bin/ps", "ax", "-ww", "-o", "pid=", "-o", "comm=", "-o", "args="],
            capture_output=True,
            text=True,
            errors="replace",
            check=False,
        ).stdout
        relevant = [
            line
            for line in processes.splitlines()
            if str(self.isolate) in line
            or any(real in line for real in self.real_names.values())
        ]
        (self.trial / "processes.txt").write_text("\n".join(relevant) + "\n")
        agent_env = []
        for line in relevant:
            if str(self.decoy) not in line:
                continue
            fields = line.split()
            if not fields or not fields[0].isdigit():
                continue
            env_dump = subprocess.run(
                [
                    "/bin/ps",
                    "eww",
                    "-p",
                    fields[0],
                    "-ww",
                    "-o",
                    "pid=",
                    "-o",
                    "command=",
                ],
                capture_output=True,
                text=True,
                errors="replace",
                check=False,
            ).stdout
            agent_env.append(env_dump)
        (self.trial / "agent-env.txt").write_text("\n".join(agent_env))
        for logical, real in self.real_names.items():
            layout = self._zj("action", "dump-layout", session=real)
            (self.trial / f"layout-{logical}.kdl").write_text(layout.stdout)
            panes = self._zj("action", "list-panes", "--json", "--command", "--all", session=real)
            (self.trial / f"panes-{logical}.json").write_text(panes.stdout)

    def teardown(self) -> None:
        try:
            if self.pty is not None:
                self.pty.close()
                self.pty = None
            for real in self.real_names.values():
                try:
                    subprocess.run(
                        [self.zellij, "delete-session", "--force", "--", real],
                        env=self.env,
                        capture_output=True,
                        timeout=8,
                        check=False,
                    )
                except subprocess.TimeoutExpired:
                    # Teardown is best-effort and must not replace the
                    # experiment's Broken verdict with a cleanup traceback.
                    continue
        finally:
            shutil.rmtree(self.isolate, ignore_errors=True)

    def _write_config(self) -> None:
        wasm = str(self.wasm)
        tui = str(self.tui)
        lines = [
            "keybinds clear-defaults=true {",
            "    shared {",
            '        bind "Alt q" {',
            f'            LaunchPlugin "file:{wasm}" {{',
            "                floating true",
            f'                tui "{tui}"',
            "            }",
            "        }",
        ]
        switch = next(
            (
                step
                for step in self.experiment.disturb
                if step.kind == "switch" and step.session
            ),
            None,
        )
        if switch is not None and switch.session is not None:
            target = self.real_names[switch.session]
            lines.append(f'        bind "Alt s" {{ SwitchSession name="{target}"; }}')
        origin = self.real_names[self.experiment.world.attach]
        lines.append(f'        bind "Alt r" {{ SwitchSession name="{origin}"; }}')
        lines.extend(
            [
                "    }",
                "}",
                "session_serialization false",
                "show_startup_tips false",
                "show_release_notes false",
                "",
            ]
        )
        self.config.write_text("\n".join(lines))

    def _write_decoy(self) -> None:
        tail = shutil.which("tail") or "/usr/bin/tail"
        self.decoy.symlink_to(tail)

    def _plant(self, session: str, role: str, sentinel: str) -> int | None:
        real = self.real_names[session]
        identity = self.isolate / f"identity-{len(self.pane_ids)}"
        # The pane id only exists after new-pane returns. Keep the launcher
        # alive until it can exec a Catalog process whose argv also carries
        # the identity that Scan normally reads from `ps eww`.
        command = (
            f"printf '%s\\n' {shlex.quote(sentinel)}; "
            f"while [ ! -s {shlex.quote(str(identity))} ]; do sleep 0.01; done; "
            f"set -- $(cat {shlex.quote(str(identity))}); "
            f"exec {shlex.quote(str(self.decoy))} -f /dev/null "
            '"ZELLIJ_PANE_ID=$1" "ZELLIJ_SESSION_NAME=$2"'
        )
        result = self._zj(
            "action",
            "new-pane",
            "--name",
            role,
            "--close-on-exit",
            "--",
            "bash",
            "-c",
            command,
            session=real,
        )
        pane_id = _parse_pane_id(result.stdout) or self._find_named_pane(real, role)
        if pane_id is not None:
            identity.write_text(f"{pane_id} {real}\n")
        return pane_id

    def _seed_scan(self) -> None:
        lines = ["META hooks=1"]
        for session in self.experiment.world.sessions:
            for pane in session.panes:
                if not pane.agent:
                    continue
                pane_id = self.pane_ids.get((session.name, pane.role))
                if pane_id is None:
                    continue
                real = self.real_names[session.name]
                lines.append(
                    f"SCAN {real} {pane_id} agent {self.decoy} --workspace {self.isolate} {pane.sentinel}"
                )
        if len(lines) > 1:
            self.state.joinpath("scan").write_text("\n".join(lines) + "\n")

    def _open_board(self) -> str | None:
        if self.pty is None:
            return "permission_denied"
        if not self.pty.write(b"\x1bq"):
            return "client_lost"
        time.sleep(0.4)
        if not self.pty.write(b"y"):
            return "client_lost"
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if "allow" in self.pty.transcript().lower():
                if not self.pty.write(b"y"):
                    return "client_lost"
            tui = self._find_tui()
            if tui is not None and self._board_chrome(tui):
                return None
            time.sleep(0.3)
        return "permission_denied"

    def _agents_ready(self) -> bool:
        for session in self.experiment.world.sessions:
            for pane in session.panes:
                pane_id = self.pane_ids.get((session.name, pane.role))
                if pane_id is None:
                    return False
                screen = self._dump(session.name, pane_id)
                if pane.sentinel not in screen:
                    return False
        return True

    def _wait_session(self, real: str) -> bool:
        return self._wait(
            lambda: self._zj("action", "list-tabs", "--json", session=real).returncode == 0,
            6,
        )

    def _wait(self, pred, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                if pred():
                    return True
            except Exception:
                pass
            time.sleep(0.15)
        return False

    def _live_logical(self) -> frozenset[str]:
        text = self._zj("list-sessions", "-n").stdout
        live = set()
        for logical, real in self.real_names.items():
            if any(line.split()[:1] == [real] for line in text.splitlines()):
                live.add(logical)
        return frozenset(live)

    def _attached_logical(self) -> str | None:
        for logical, real in self.real_names.items():
            clients = self._zj("action", "list-clients", session=real)
            if clients.returncode == 0 and list_clients_present(clients.stdout):
                return logical
        if self.pty is not None:
            text = _strip_ansi(self.pty.transcript())
            matches = re.findall(r"Zellij \(([^)]+)\)", text)
            if matches:
                current = matches[-1]
                for logical, real in self.real_names.items():
                    if current == real:
                        return logical
        for flags in (["-n"], []):
            text = self._zj("list-sessions", *flags).stdout
            for line in text.splitlines():
                if "attached" not in line.lower():
                    continue
                head = line.split()[0] if line.split() else ""
                for logical, real in self.real_names.items():
                    if head == real or real in line:
                        return logical
        return None

    def _control_alive(self) -> bool:
        attach = self.real_names[self.experiment.world.attach]
        # After a switch the origin session may still answer; any mapped session is enough.
        for real in self.real_names.values():
            if self._zj("action", "list-tabs", "--json", session=real, timeout=3).returncode == 0:
                return True
        return self._zj("action", "list-tabs", "--json", session=attach, timeout=3).returncode == 0

    def _visible_sentinels(self) -> frozenset[str]:
        logical = self._attached_logical()
        if logical is None:
            return frozenset()
        candidates = frozenset(
            pane.sentinel
            for session in self.experiment.world.sessions
            for pane in session.panes
        )
        return focused_sentinels(
            self._panes(logical),
            lambda pane_id: self._dump(logical, pane_id),
            candidates,
        )

    def _panes(self, logical: str) -> list[dict]:
        real = self.real_names[logical]
        result = self._zj(
            "action",
            "list-panes",
            "--json",
            "--state",
            "--all",
            session=real,
        )
        try:
            panes = json.loads(result.stdout or "[]")
        except json.JSONDecodeError:
            return []
        return panes if isinstance(panes, list) else []

    def _log_lines(self) -> tuple[str, ...]:
        lines: list[str] = []
        if self.pty is not None:
            lines.extend(self.pty.transcript().splitlines())
        for path in self.isolate.rglob("*.log"):
            try:
                lines.extend(path.read_text(errors="replace").splitlines())
            except OSError:
                pass
        return tuple(lines)

    def _server_cookie(self) -> dict[str, int]:
        cookie: dict[str, int] = {}
        if self.sock.is_dir():
            for path in self.sock.iterdir():
                try:
                    cookie[path.name] = path.stat().st_ino
                except OSError:
                    continue
        return cookie

    def _server_restarted(self) -> bool:
        if not self.server_cookie:
            return False
        now = self._server_cookie()
        return any(name in now and now[name] != ino for name, ino in self.server_cookie.items())

    def _dump(self, logical: str, pane_id: int) -> str:
        real = self.real_names[logical]
        result = self._zj(
            "action",
            "dump-screen",
            "--pane-id",
            f"terminal_{pane_id}",
            session=real,
        )
        return result.stdout

    def _first_pane(self, logical: str) -> int | None:
        for (session, _), pane_id in self.pane_ids.items():
            if session == logical:
                return pane_id
        return None

    def _find_named_pane(self, real: str, role: str) -> int | None:
        result = self._zj("action", "list-panes", "--json", "--command", "--all", session=real)
        try:
            panes = json.loads(result.stdout or "[]")
        except json.JSONDecodeError:
            return None
        for pane in panes:
            blob = " ".join(
                str(pane.get(key) or "")
                for key in ("title", "pane_command", "terminal_command", "name")
            )
            if role in blob or str(self.decoy) in blob:
                return int(pane["id"])
        return None

    def _find_tui(self) -> int | None:
        attach = self.real_names[self.experiment.world.attach]
        # After a jump the TUI may already be gone; look at the attach session first.
        for real in (attach, *self.real_names.values()):
            result = self._zj("action", "list-panes", "--json", "--command", "--all", session=real)
            try:
                panes = json.loads(result.stdout or "[]")
            except json.JSONDecodeError:
                continue
            for pane in panes:
                if pane.get("is_plugin"):
                    continue
                blob = " ".join(
                    str(pane.get(key) or "")
                    for key in ("title", "pane_command", "terminal_command")
                )
                if "board-tui" in blob:
                    return int(pane["id"])
        return None

    def _board_chrome(self, tui_id: int) -> bool:
        attach = self.experiment.world.attach
        screen = self._dump(attach, tui_id)
        agent_sessions = {
            self.real_names[session.name]
            for session in self.experiment.world.sessions
            if any(pane.agent for pane in session.panes)
        }
        return "j/k" in screen and all(name in screen for name in agent_sessions)

    def _zj(
        self,
        *args: str,
        session: str | None = None,
        timeout: float = 8,
    ) -> subprocess.CompletedProcess[str]:
        cmd = [self.zellij]
        if session:
            cmd += ["--session", session]
        cmd += list(args)
        try:
            return subprocess.run(
                cmd,
                env=self.env,
                capture_output=True,
                text=True,
                timeout=timeout,
                check=False,
            )
        except subprocess.TimeoutExpired as exc:
            return subprocess.CompletedProcess(cmd, 1, exc.stdout or "", exc.stderr or "timeout")


def _strip_ansi(text: str) -> str:
    return re.sub(r"\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~]|\].*?(?:\x07|\x1b\\))", "", text)


def focused_sentinels(
    panes: list[dict],
    screen_for,
    candidates: frozenset[str],
) -> frozenset[str]:
    seen: set[str] = set()
    for pane in panes:
        if pane.get("is_plugin") or not pane.get("is_focused"):
            continue
        try:
            screen = screen_for(int(pane["id"])) or ""
        except (KeyError, TypeError, ValueError):
            continue
        seen.update(token for token in candidates if token in screen)
    return frozenset(seen)


def list_clients_present(text: str) -> bool:
    return any(re.match(r"^\s*\d+\s+", line) for line in text.splitlines())


def _parse_pane_id(text: str) -> int | None:
    for token in text.split():
        if token.startswith("terminal_"):
            try:
                return int(token.split("_", 1)[1])
            except ValueError:
                return None
        if token.isdigit():
            return int(token)
    return None


_DISTURB_ADAPTERS = {
    "switch": "_disturb_switch",
    "go": "_disturb_go",
}
