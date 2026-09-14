# Host state ownership

The local daemon (`board-tui --daemon`) owns `state.redb`. Board clients
request a snapshot or a refresh over HTTP/1 on a private Unix socket.
Hyper owns HTTP framing and parsing; `/v1/` versions the routes. Hooks do not open redb. The WASM build has no redb dependency.

## Lifetime and concurrency

A process lock in the data directory elects one owner before database opening
or endpoint publication. Simultaneous TUI launches can start candidates; only
one becomes the owner. Clients have bounded I/O and startup waits. Socket
files live in a private short temporary directory, with the endpoint published
atomically in the data directory, so long worktree paths work on macOS too.

One HTTP I/O thread forwards validated requests through a bounded queue to the
database owner. Both queue capacity and active connections are limited to 32.
Request bodies are limited to 64 KiB, responses to 16 MiB, and each connection
has a two-second overall deadline. Each connection serves one request; keepalive
is disabled. The synchronous client reuses a current-thread Tokio runtime and
has a 500 ms request deadline. Incomplete HTTP bodies cannot block database
work. Only the Unix socket is bound; there is no TCP port, DNS or proxy path.

One worker scans processes and Zellij panes. The owner continues serving
committed snapshots while it runs, then commits the scan and title changes
in one transaction. Refresh requests coalesce while work is pending and for
two seconds after its start. There is no per-TUI reconciler process anymore.
The daemon remains available after a board closes; scans currently require
client refresh requests. Automatic sleep scheduling is future work.

## Persistence and migration

Data directory precedence: `ZAB_STATE_DIR`, `XDG_DATA_HOME/zellij-agent-board`,
then `~/.local/share/zellij-agent-board`. Tests override `ZAB_STATE_DIR`.
The database cache is capped at 4 MiB; this is not a whole-process memory cap.

The `state` table holds schema version, revision, scan protocol text, and place
protocol text. Every publish uses `Durability::Immediate`. The revision and
both snapshots become visible together. This initial schema preserves existing
Board protocol semantics; it does not yet contain resumable chat records or a
sleep state machine.

Initialization imports legacy `scan`, `places.host`, and `places` from the
runtime cache directory. Current `places` takes precedence over legacy names.
The import and schema marker commit together. Missing files are allowed;
other file I/O failures abort initialization. Existing source files are kept
for rollback, but never read again as authoritative state after initialization.
Database corruption and unsupported schemas fail instead of resetting state.

Focus, hook spool, and seen/started files remain input channels. Their writers
are unchanged. A database snapshot includes the notices observed by its scan;
the TUI also consumes live hook notices between scans. This is not a claim of
atomicity between hook files and the database.

## HTTP interface

| Request | Body | Successful response |
| --- | --- | --- |
| `GET /v1/snapshot` | None | `200` with `{revision, scan, places}` |
| `POST /v1/refresh` | `application/json`: `{"home":"session-name"}` | `202` with `{"accepted":true}` |
| `POST /v1/shutdown` | None | `202` with `{"accepted":true}` |

Refresh and shutdown acknowledge acceptance, not completion. Refreshes may be
coalesced; observe snapshot revisions to check for a later committed result.
Shutdown waits for pending scan work before removing the endpoint.

Errors have an `{"error":"message"}` JSON body: unknown routes/versions return
404, wrong methods 405 (with `Allow`), invalid JSON or unexpected bodies 400,
unsupported refresh media types 415, oversized bodies 413, and an unavailable
or saturated owner queue 503. Hyper handles malformed HTTP syntax separately.

For example, using the same socket as the TUI:

```bash
state_dir="${ZAB_STATE_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/zellij-agent-board}"
daemon_socket="$(cat "$state_dir/daemon.endpoint")"
curl --noproxy '*' --unix-socket "$daemon_socket" http://localhost/v1/snapshot
curl --noproxy '*' --unix-socket "$daemon_socket" \
  -H 'Content-Type: application/json' -d '{"home":""}' \
  http://localhost/v1/refresh
```

## Recovery and diagnostics

After an unclean exit, redb recovers committed state; a new lock owner publishes
a new socket endpoint. An open TUI retains its last model and attempts to
restart the daemon when its next refresh fails. Ordinary TUI clients never
fall back to reading legacy snapshots over a newer database.

`--snapshot` reads the committed snapshot without starting a daemon.
`--reconcile` starts/connects and requests a refresh (asynchronous acknowledgement).
`--daemon-stop` asks the owner to finish its pending scan, commit it and exit.
Close open boards first, as they reconnect automatically. Binary upgrades
require this restart; the daemon is not hot-reloaded. An unclean exit can leave
a small stale socket directory under `/tmp/zabd-*`; the endpoint is replaced
on restart and no database state is stored there.

## Regression checks

`tests/daemon.rs` runs actual daemon processes, real Unix sockets and redb:
committed state survives SIGKILL/restart without reimporting stale cache files;
a competing owner cannot disrupt the active one; malformed/disconnected
clients do not take down concurrent readers. HTTP regressions verify curl
interoperability, chunked bodies, rejected mutations, body limits, and progress
while other clients leave headers or bodies unfinished. Storage tests cover interrupted
migration and preservation of other sessions' titles.

Real Zellij regression scenarios exercise the production board/daemon path.
Their teardown stops the isolated daemon; failure bundles include a committed
JSON snapshot rather than copying an open database file.
