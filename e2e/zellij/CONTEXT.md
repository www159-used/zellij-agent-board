# Fault experiment

Repository-local Zellij lifecycle experiments. A scenario declares a
World, a Disturb, and Holds; the harness materializes them on a real
attached client.

## Language

**Experiment**:
One named trial: a World, an ordered Disturb, and the Holds that must
stay true for the whole run.
_Avoid_: test, case, script, scene (scenes are in-process Board replays)

**World**:
The isolated topology an Experiment starts from: sessions, panes,
whether a session has the board, and which pane the client attaches to.
_Avoid_: fixture, env, cluster

**Disturb**:
An intended change to the World — native switch or go-to-Agent.
_Avoid_: action, step, command (those leak CLI/PTY verbs)

**Hold**:
A World fact that must remain true from quiet to teardown. Default
Holds are always on; a scenario may only add facts.
_Avoid_: assert, oracle, invariant (Hold is the declared fact)

**AgentRef**:
How a scenario names an Agent: session + role. The harness binds the
real pane id.
_Avoid_: pane id, AgentId (AgentId is the product identity after Scan)

**Verdict**:
The closed result of one Experiment: Held, Broken, or SetupFailed.
_Avoid_: pass, fail, error (those collapse setup and product faults)
