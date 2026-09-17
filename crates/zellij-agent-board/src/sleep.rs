//! Daemon-side sleep/resume control, without a per-pane supervisor.
//!
//! The daemon owns the relationship table ([`SleepRecord`]) and drives the
//! pane through one seam: [`Injector`] types characters into the agent's pane
//! (`zellij action write-chars`). Sleep injects the agent's exit; resume types
//! the exact relaunch command into the shell the exited agent left behind.
//!
//! The logic here is a pure unit over the database and the injector, so it is
//! tested without Zellij; only [`zellij_injector`] shells out.
use std::io;
use std::process::Command;

use crate::database::HostDatabase;
use crate::scan::zellij_bin;
use crate::AgentId;

/// Types characters into an agent's pane. Real callers reach Zellij; tests
/// substitute a recording closure so the control logic needs no terminal.
pub(crate) type Injector<'a> = &'a (dyn Fn(&AgentId, &str) -> io::Result<()> + Send + Sync);

/// Outcome of a control request. A rejection carries the same vocabulary the
/// supervisor used (`not_sleepable`, ...) so the board and E2E read alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Accepted,
    Rejected(&'static str),
}

/// Ask the agent in `id`'s pane to exit while keeping the pane. Only an idle,
/// running, known agent is sleepable; anything else is rejected untouched.
pub(crate) fn sleep(
    db: &HostDatabase,
    inject: Injector,
    now: u64,
    id: &AgentId,
) -> io::Result<Outcome> {
    let Some(mut record) = db.sleep_record(id)? else {
        return Ok(Outcome::Rejected("no_record"));
    };
    if record.phase != "running" || record.status != "idle" {
        return Ok(Outcome::Rejected("not_sleepable"));
    }
    let Some(exit) = exit_text(&record.kind) else {
        return Ok(Outcome::Rejected("unknown_kind"));
    };
    inject(id, exit)?;
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
    inject(id, &shell_line(&record.resume_cmd))?;
    record.phase = "running".into();
    record.updated_at = now;
    db.upsert_sleep(id, &record)?;
    Ok(Outcome::Accepted)
}

/// How each agent family is told to exit into its shell. Extend per kind.
fn exit_text(kind: &str) -> Option<&'static str> {
    match kind {
        // Claude Code runs a raw-mode TUI: submit needs a carriage return
        // (`\r`, the byte a real Enter sends), not a line feed. `\n` only lands
        // `/exit` in the input box without submitting, so the REPL never exits.
        "claude" => Some("/exit\r"),
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

/// The real injector: `zellij --session <s> action write-chars` into the pane.
/// Same channel the board and E2E already use.
pub(crate) fn zellij_injector(id: &AgentId, text: &str) -> io::Result<()> {
    let status = Command::new(zellij_bin())
        .args(["--session", &id.session])
        .args([
            "action",
            "write-chars",
            "--pane-id",
            &format!("terminal_{}", id.pane_id),
            "--",
            text,
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "write-chars failed for {}:{} ({})",
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

    type CallLog = Mutex<Vec<(AgentId, String)>>;

    /// A fresh capture buffer for injected characters.
    fn recorder() -> &'static CallLog {
        Box::leak(Box::new(Mutex::new(Vec::new())))
    }

    /// A recording injector over `log`, so the control logic can be asserted
    /// without a terminal.
    fn record_into(log: &CallLog) -> impl Fn(&AgentId, &str) -> io::Result<()> + '_ {
        move |id: &AgentId, text: &str| {
            log.lock().unwrap().push((id.clone(), text.to_owned()));
            Ok(())
        }
    }

    #[test]
    fn idle_running_claude_sleeps_by_injecting_exit_and_marks_stopping() {
        let dir = tempfile::tempdir().unwrap();
        let db = open_db(dir.path());
        db.upsert_sleep(&id(), &running_claude()).unwrap();
        let log = recorder();
        let inject = record_into(log);

        assert_eq!(sleep(&db, &inject, 42, &id()).unwrap(), Outcome::Accepted);

        let calls = log.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], (id(), "/exit\r".to_owned()));
        let record = db.sleep_record(&id()).unwrap().unwrap();
        assert_eq!(record.phase, "stopping");
        assert_eq!(record.updated_at, 42);
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
        assert_eq!(calls[0], (id(), "claude -r sid-1\n".to_owned()));
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
