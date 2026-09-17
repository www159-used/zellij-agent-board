//! Daemon-side sleep/resume control, without a per-pane supervisor.
//!
//! The daemon owns the relationship table ([`SleepRecord`]) and drives the
//! pane through one seam: [`Injector`] types into the agent's pane. Sleep
//! injects the agent's exit; resume types the exact relaunch command into
//! the shell the exited agent left behind.
//!
//! The logic here is a pure unit over the database and the injector, so it is
//! tested without Zellij; only [`zellij_injector`] shells out.
use std::io;
use std::process::Command;

use crate::database::HostDatabase;
use crate::scan::zellij_bin;
use crate::AgentId;

/// One keystroke burst to inject. Characters go through `write-chars`; a lone
/// control byte goes through `write` so Enter / Ctrl+C cannot be stripped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stroke {
    Chars(String),
    Byte(u8),
}

/// Types into an agent's pane. Real callers reach Zellij; tests substitute a
/// recording closure so the control logic needs no terminal.
pub(crate) type Injector<'a> = &'a (dyn Fn(&AgentId, &Stroke) -> io::Result<()> + Send + Sync);

/// Outcome of a control request. A rejection carries the same vocabulary the
/// supervisor used (`not_sleepable`, ...) so the board and E2E read alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Accepted,
    Rejected(&'static str),
}

/// Ask the agent in `id`'s pane to exit while keeping the pane. Only a
/// running, known agent is sleepable; a known-busy status is rejected
/// untouched. Missing idle status is allowed: real Claude often has no
/// registry row on the wrapper pid the scan kept.
pub(crate) fn sleep(
    db: &HostDatabase,
    inject: Injector,
    now: u64,
    id: &AgentId,
) -> io::Result<Outcome> {
    let Some(mut record) = db.sleep_record(id)? else {
        return Ok(Outcome::Rejected("no_record"));
    };
    if record.phase != "running" || is_blocking_status(&record.status) {
        return Ok(Outcome::Rejected("not_sleepable"));
    }
    let Some(strokes) = exit_strokes(&record.kind) else {
        return Ok(Outcome::Rejected("unknown_kind"));
    };
    for stroke in &strokes {
        inject(id, stroke)?;
    }
    record.phase = "stopping".into();
    record.updated_at = now;
    db.upsert_sleep(id, &record)?;
    Ok(Outcome::Accepted)
}

/// Relaunch the exact conversation in the same pane. Valid only once the agent
/// has actually left (a scan set `sleeping`, or `failed` after a bad exit).
pub(crate) fn resume(
    db: &HostDatabase,
    inject: Injector,
    now: u64,
    id: &AgentId,
) -> io::Result<Outcome> {
    let Some(mut record) = db.sleep_record(id)? else {
        return Ok(Outcome::Rejected("no_record"));
    };
    if record.phase != "sleeping" && record.phase != "failed" {
        return Ok(Outcome::Rejected("not_sleeping"));
    }
    if record.resume_cmd.is_empty() {
        return Ok(Outcome::Rejected("missing_resume_cmd"));
    }
    inject(id, &Stroke::Chars(shell_line(&record.resume_cmd)))?;
    record.phase = "running".into();
    record.updated_at = now;
    db.upsert_sleep(id, &record)?;
    Ok(Outcome::Accepted)
}

/// Statuses that mean the agent is mid-turn. Empty / `idle` / unknown stay
/// sleepable so a missing `sessions/<pid>.json` cannot block `z`.
fn is_blocking_status(status: &str) -> bool {
    matches!(
        status,
        "busy" | "working" | "running" | "waiting" | "requires_action"
    )
}

/// How each agent family is told to exit into its shell. Extend per kind.
///
/// Claude Code is a raw-mode TUI. `write-chars "/exit\r"` is not enough:
/// Zellij's character path can drop the carriage return, and a burst that
/// includes Enter often lands `/exit` in the input box without submitting.
/// Clear any draft with Ctrl+C, type the slash command, then send Enter
/// as a raw `write 13` — the same seam the E2E already uses on a shell.
fn exit_strokes(kind: &str) -> Option<Vec<Stroke>> {
    match kind {
        "claude" => Some(vec![
            Stroke::Byte(0x03),
            Stroke::Chars("/exit".into()),
            Stroke::Byte(b'\r'),
        ]),
        _ => None,
    }
}

/// Render argv as one shell line the pane's shell will run, newline-terminated.
fn shell_line(argv: &[String]) -> String {
    let mut line = argv
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    line.push('\n');
    line
}

/// POSIX single-quote quoting; safe for arbitrary argv typed into a shell.
fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b':' | b'=')
        })
    {
        return arg.to_owned();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// The real injector: `write-chars` for text, `write` for a control byte.
/// Same channel the board and E2E already use.
pub(crate) fn zellij_injector(id: &AgentId, stroke: &Stroke) -> io::Result<()> {
    match stroke {
        Stroke::Chars(text) => zellij_action(
            id,
            &[
                "action",
                "write-chars",
                "--pane-id",
                &format!("terminal_{}", id.pane_id),
                "--",
                text,
            ],
        ),
        Stroke::Byte(byte) => zellij_action(
            id,
            &[
                "action",
                "write",
                "--pane-id",
                &format!("terminal_{}", id.pane_id),
                "--",
                &byte.to_string(),
            ],
        ),
    }
}

fn zellij_action(id: &AgentId, args: &[&str]) -> io::Result<()> {
    let status = Command::new(zellij_bin())
        .args(["--session", &id.session])
        .args(args)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "pane inject failed for {}:{} ({})",
            id.session,
            id.pane_id,
            status.code().unwrap_or(-1)
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::SleepRecord;
    use std::sync::Mutex;

    fn open_db(dir: &std::path::Path) -> HostDatabase {
        std::fs::write(dir.join("scan"), "META hooks=1\n").unwrap();
        HostDatabase::open(&dir.join("state.redb"), dir).unwrap()
    }

    fn id() -> AgentId {
        AgentId {
            session: "work".into(),
            pane_id: 3,
        }
    }

    fn running_claude() -> SleepRecord {
        SleepRecord {
            kind: "claude".into(),
            cwd: "/tmp/ww".into(),
            session_id: "sid-1".into(),
            resume_cmd: vec!["claude".into(), "-r".into(), "sid-1".into()],
            phase: "running".into(),
            status: "idle".into(),
            last_pid: Some(1234),
            missing_since: None,
            updated_at: 1,
        }
    }

    type CallLog = Mutex<Vec<(AgentId, Stroke)>>;

    /// A fresh capture buffer for injected characters.
    fn recorder() -> &'static CallLog {
        Box::leak(Box::new(Mutex::new(Vec::new())))
    }

    /// A recording injector over `log`, so the control logic can be asserted
    /// without a terminal.
    fn record_into(log: &CallLog) -> impl Fn(&AgentId, &Stroke) -> io::Result<()> + '_ {
        move |id: &AgentId, stroke: &Stroke| {
            log.lock().unwrap().push((id.clone(), stroke.clone()));
            Ok(())
        }
    }

    fn claude_exit() -> [Stroke; 3] {
        [
            Stroke::Byte(0x03),
            Stroke::Chars("/exit".into()),
            Stroke::Byte(b'\r'),
        ]
    }

    #[test]
    fn idle_running_claude_sleeps_by_submitting_exit_and_marks_stopping() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_db(dir.path());
        db.upsert_sleep(&id(), &running_claude()).unwrap();
        let log = recorder();
        let inject = record_into(log);

        assert_eq!(sleep(&db, &inject, 42, &id()).unwrap(), Outcome::Accepted);

        let calls = log.lock().unwrap();
        assert_eq!(calls.len(), 3);
        for (index, stroke) in claude_exit().iter().enumerate() {
            assert_eq!(calls[index], (id(), stroke.clone()));
        }
        let record = db.sleep_record(&id()).unwrap().unwrap();
        assert_eq!(record.phase, "stopping");
        assert_eq!(record.updated_at, 42);
    }

    #[test]
    fn empty_status_is_sleepable_so_a_missing_registry_row_cannot_block_z() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_db(dir.path());
        let mut unknown = running_claude();
        unknown.status.clear();
        db.upsert_sleep(&id(), &unknown).unwrap();
        let log = recorder();
        let inject = record_into(log);

        assert_eq!(sleep(&db, &inject, 1, &id()).unwrap(), Outcome::Accepted);
        assert_eq!(log.lock().unwrap().len(), 3);
    }

    #[test]
    fn busy_or_unknown_agent_is_not_sleepable_and_pane_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_db(dir.path());
        let log = recorder();
        let inject = record_into(log);

        // No record at all.
        assert_eq!(
            sleep(&db, &inject, 1, &id()).unwrap(),
            Outcome::Rejected("no_record")
        );
        // Busy agent.
        let mut busy = running_claude();
        busy.status = "busy".into();
        db.upsert_sleep(&id(), &busy).unwrap();
        assert_eq!(
            sleep(&db, &inject, 1, &id()).unwrap(),
            Outcome::Rejected("not_sleepable")
        );
        // Unknown kind.
        let mut other = running_claude();
        other.kind = "mystery".into();
        db.upsert_sleep(&id(), &other).unwrap();
        assert_eq!(
            sleep(&db, &inject, 1, &id()).unwrap(),
            Outcome::Rejected("unknown_kind")
        );
        assert!(log.lock().unwrap().is_empty(), "no injection on rejection");
        // Phase never advanced.
        assert_eq!(db.sleep_record(&id()).unwrap().unwrap().phase, "running");
    }

    #[test]
    fn resume_types_exact_relaunch_and_marks_running() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_db(dir.path());
        let mut sleeping = running_claude();
        sleeping.phase = "sleeping".into();
        sleeping.status = String::new();
        sleeping.last_pid = None;
        db.upsert_sleep(&id(), &sleeping).unwrap();
        let log = recorder();
        let inject = record_into(log);

        assert_eq!(resume(&db, &inject, 7, &id()).unwrap(), Outcome::Accepted);

        let calls = log.lock().unwrap();
        assert_eq!(
            calls[0],
            (id(), Stroke::Chars("claude -r sid-1\n".to_owned()))
        );
        assert_eq!(db.sleep_record(&id()).unwrap().unwrap().phase, "running");
    }

    #[test]
    fn resume_rejects_when_not_sleeping_or_no_command() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_db(dir.path());
        let log = recorder();
        let inject = record_into(log);

        assert_eq!(
            resume(&db, &inject, 1, &id()).unwrap(),
            Outcome::Rejected("no_record")
        );
        db.upsert_sleep(&id(), &running_claude()).unwrap();
        assert_eq!(
            resume(&db, &inject, 1, &id()).unwrap(),
            Outcome::Rejected("not_sleeping")
        );
        let mut empty = running_claude();
        empty.phase = "sleeping".into();
        empty.resume_cmd.clear();
        db.upsert_sleep(&id(), &empty).unwrap();
        assert_eq!(
            resume(&db, &inject, 1, &id()).unwrap(),
            Outcome::Rejected("missing_resume_cmd")
        );
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn shell_line_quotes_arguments_with_spaces_and_quotes() {
        assert_eq!(shell_line(&["claude".into(), "-r".into()]), "claude -r\n");
        assert_eq!(
            shell_line(&["a b".into(), "it's".into()]),
            "'a b' 'it'\\''s'\n"
        );
    }
}
