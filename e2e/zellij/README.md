# Zellij fault experiments

These experiments exercise lifecycle behavior through a real PTY-attached
Zellij client. They are separate from `e2e/scenes`, which replay the Board
in-process.

## Run

```bash
./scripts/zab-fault.sh
./scripts/zab-fault.sh native-cross-session-switch 20
./scripts/zab-fault.sh board-same-session-focus 20
./scripts/zab-fault.sh board-cross-session-jump 100
```

Set `ZAB_FAULT_ZELLIJ` to test an exact binary:

```bash
ZAB_FAULT_ZELLIJ=/path/to/zellij-0.45.0 \
  ./scripts/zab-fault.sh board-cross-session-jump 100
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
repeat = 100
given = { attached = "origin", board = "origin" }
target = { ref = "dest.target", agent = "cursor", shows = "ZAB-SENTINEL-DEST" }
when = { go = "dest.target" }
then = { attached = "dest", sees = "dest.target" }
```

One document may contain multiple `[[case]]` entries. The loader derives the
internal World, Disturb, and Holds from each Case:

- `given`: initial attachment and optional board location
- `target` or `targets`: coupled Agent identity and visible sentinel
- `when`: native session switch or board go-to-Agent
- `then`: destination attachment and visible target facts

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
