# zellij-agent-board

Floating Zellij dashboard of live coding-agent panes. Existence comes from the process scan; hooks only carry status.

## Language

**Agent**:
A live coding-agent pane, identified by (session, pane).
_Avoid_: process, CLI, row (the board row is a view of an Agent)

**Catalog**:
The loaded set of adapters and protocols that decide which processes exist, how hooks install, and how events are named.
_Avoid_: plugin registry, config, allowlist

**Adapter**:
One CLI family's declaration: bins, badge, skip list, and which protocol it speaks.
_Avoid_: plugin, integration, tool (tool is the scanned comm name)

**Protocol**:
A hook-install and event-name family shared by many adapters (`cursor`, `cc`, `opencode`).
_Avoid_: schema, backend, hook format

**Scan**:
The host process enumeration that creates and drops Agents. The only source of existence.
_Avoid_: discover, pgrep (pgrep silently misses some live processes)

**Hook**:
A status notice for an Agent that already exists. It never creates a row.
_Avoid_: event (the raw CLI name; the board consumes a normalized hook)
