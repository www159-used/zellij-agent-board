# Zellij fault experiments

These experiments exercise lifecycle behavior through a real PTY-attached
Zellij client. They are separate from `e2e/scenes`, which replay the Board
in-process.

## Run

```bash
make fault
make fault SCENARIO=native-cross-session-switch REPEAT=20
make fault SCENARIO=board-same-session-focus REPEAT=20
make fault SCENARIO=board-launch-focus REPEAT=5
make fault SCENARIO=board-cross-session-jump REPEAT=40
```

Set `ZAB_FAULT_ZELLIJ` to test an exact binary:

```bash
ZAB_FAULT_ZELLIJ=/path/to/zellij-0.45.0 \
  make fault SCENARIO=board-cross-session-jump REPEAT=40
```

Exit `0` means every Hold held, `1` means a lifecycle Hold broke, and `2`
means the World could not be set up. Failure artifacts are written under
`target/zab-fault/<experiment>/01/`. Start with `summary.txt`; use
`outcome.json` for tooling and the remaining files for raw evidence.

`repeat` runs inside one isolated World: the same PTY client repeatedly
opens the board, jumps, returns, and opens it again. This preserves
suppressed panes and server state so lifecycle races accumulate.

## Add Cases

Add a TOML file under `scenarios/`. Each `[[case]]` keeps its setup, target,
disturbance, and expected outcome together. An Agent is referenced as
`session.role`:

```toml
[[case]]
name = "board-cross-session-jump"
description = "Jump from the board to an Agent in another session."
repeat = 40
given = { attached = "origin", board = "origin" }
target = { ref = "dest.target", agent = "cursor", shows = "ZAB-SENTINEL-DEST" }
when = { go = "dest.target" }
then = { attached = "dest", sees = "dest.target" }
```

One document may contain multiple `[[case]]` entries. The loader derives the
internal World, Disturb, and Holds from each Case:

- `given`: initial attachment, optional board location, and optional `focus = "session.role"` before opening the board
- `target` or `targets`: coupled Agent identity and visible sentinel
- `when`: native session switch or board go-to-Agent
- `then`: destination attachment and visible target facts

The runner also captures the committed daemon snapshot and stops the isolated
daemon before removing its state directory.

The runner owns real pane ids, short socket paths, Scan isolation, plugin
permission handling, bounded polling, teardown, and artifacts. Scenarios
must not contain subprocess commands, sleeps, socket paths, or log matches.

The loader still accepts the original World/Disturb/Hold documents, so
existing external recipes do not need a flag-day migration.

To add a Disturb kind, add its value loader to `_DISTURB_LOADERS` in
`harness/scenario.py` and its runtime adapter to `_DISTURB_ADAPTERS` in
`harness/run.py`. Unknown kinds fail while loading and print the registered
choices. Holds use the corresponding `_HOLD_LOADERS` seam plus an observer
and oracle rule.

Default Holds always check that the PTY client remains attached, declared
sessions survive, the control plane responds, and the server is not
unexpectedly replaced. A CliPipe one-second timeout and
`Starting Zellij client!` after an intended switch are not failures alone.

## Phases and mock agent

Use `[[case.phase]]` when intermediate outcomes matter. Each phase waits for
its own `then` before the next disturbance runs; a failure stops the cycle and
records the phase name. Do not mix phases with case-level `when`/`then`.
Existing single-step cases keep their behavior.

```toml
[[case.phase]]
name = "switch-away"
when = { switch = "dest" }
then = { attached = "dest", sees = "dest.target" }

[[case.phase]]
name = "return-to-origin"
when = { return = true }
then = { attached = "origin" }
```

`return` uses the original PTY client to return to the initial session and
reopens its board where declared. `switch` currently uses the one native
switch destination configured for the experiment.

The standalone mock runs without network access or credentials:

```bash
python3 e2e/zellij/harness/mock_agent.py --store /tmp/mock-agent-test new
python3 e2e/zellij/harness/mock_agent.py --store /tmp/mock-agent-test resume EXACT_UUID
```

It accepts JSON lines on stdin and emits JSON lines on stdout. Operations:
`submit`/`finish` with `text`, `permission`, `approve`, `draft` with `text`,
`inspect`, and `exit`. The ready event includes session id, process id,
instance id, and restored messages. Sessions are atomically persisted before
acknowledgement; concurrent processes cannot own the same session. Unknown
resume ids fail rather than opening a new conversation. Events are also
written to `events.jsonl` as independent process evidence.

`--exit-mode ignore|crash` and `--exit-delay SECONDS` inject exit faults.
A requested normal exit requires idle state and no draft. EOF exits normally;
crashes do not emit a clean-exit event. These are explicit mock contracts,
not claims about the real Codex CLI's protocol.

The subprocess regressions run in the existing harness unittest command.
The mock is not yet launched by the Zellij scenarios: connecting it through
the production supervisor, lifecycle policy, and Codex hook adapter is the
next integration step. The current suite does **not** assert automatic sleep.
