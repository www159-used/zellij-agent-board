# Working in this repository

Read `CONTEXT.md` and `CONTEXT-MAP.md` before changing product behavior.

## OpenSpec

Use OpenSpec for new features and behavior changes:

1. Inspect `openspec/specs/` and `openspec list --json`.
2. Explore the user problem and current behavior. Capture a change proposal,
   scenarios, alternatives, and open decisions under `openspec/changes/`.
3. Distinguish confirmed requirements from proposed defaults. A prototype,
   passing tests, generated artifacts, or successful CLI validation does not
   establish product agreement.
4. Resolve critical product choices with the user before drafting implementation
   tasks and starting apply. Respect authorization already given in the session;
   routine implementation choices do not require repeated confirmation.
5. Validate changed specs with `openspec validate --all --strict --no-interactive`.
   Verify implementation against accepted scenarios before syncing or archiving.

The `notification-queue` change is currently exploration only. Its handling
policy and queue granularity are unresolved. PR #11 is a prototype, not the
accepted specification. Do not infer approval from that implementation.

OpenSpec skills are installed under `.codex/skills/` and `.cursor/skills/`;
Cursor commands are under `.cursor/commands/`.

## Verification

For product changes, use `make check` and the relevant scene or Zellij tests.
For OpenSpec-only changes, validate the specs and inspect the diff; product
checks are needed only if runtime code or behavior changes.
Use Python 3.11+ for the hook installer tests (`tomllib` is required).
