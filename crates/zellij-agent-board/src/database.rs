//! Host-only durable snapshots. Only the daemon opens this database.
use std::io;
use std::path::Path;

use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::protocol::{format_places, merge_places, parse_places, replace_session_places};
use crate::{AgentId, PanePlace};

const STATE: TableDefinition<&str, &str> = TableDefinition::new("state");
/// Sleep/resume relationship table: one row per supervised-less agent pane,
/// keyed by `AgentId`. Added additively; readers on the old schema ignore it,
/// so it does not bump `SCHEMA`.
const SLEEP: TableDefinition<&str, &str> = TableDefinition::new("sleep_state");
const SCHEMA: &str = "1";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub revision: u64,
    pub scan: Option<String>,
    pub places: String,
    /// Sleep/resume relationship rows. Observability only: it does not affect
    /// `revision`, which still tracks scan/place changes.
    #[serde(default)]
    pub sleep: Vec<SleepRow>,
}

/// A relationship-table row flattened for `--snapshot` / clients. Mirrors the
/// durable [`SleepRecord`] with its `AgentId` inlined.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SleepRow {
    pub session: String,
    pub pane_id: u32,
    pub kind: String,
    pub session_id: String,
    pub phase: String,
    pub status: String,
    pub cwd: String,
}

/// One agent pane's resume relationship. The daemon is the only writer; the
/// exact `resume_cmd` (e.g. `["claude","-r",<id>]`) lets resume re-run the
/// same conversation in the same pane without a per-pane supervisor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SleepRecord {
    /// Agent family, decides the resume command shape (`claude`, `cursor`, ...).
    pub kind: String,
    pub cwd: String,
    /// Claude `sessionId` for exact `-r`; empty until a scan observes it.
    pub session_id: String,
    /// Exact argv to relaunch the conversation in the pane's shell.
    pub resume_cmd: Vec<String>,
    /// `running` | `stopping` | `sleeping` | `failed`.
    pub phase: String,
    /// Live agent activity as last observed by a scan (`idle` gates sleep).
    pub status: String,
    pub last_pid: Option<u32>,
    /// First time the pane was probed as gone. A single glitchy `list-panes`
    /// (e.g. during a pane's command transition) must not drop a resumable row,
    /// so pruning waits until the pane stays gone past [`PANE_GONE_GRACE_SECS`].
    #[serde(default)]
    pub missing_since: Option<u64>,
    pub updated_at: u64,
}

/// How long a pane must stay gone across scans before its row is pruned. Long
/// enough to ride out a transient probe during a pane's command transition.
const PANE_GONE_GRACE_SECS: u64 = 10;

/// One live agent a scan saw this pass, enough to (re)build its relationship.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SleepObservation {
    pub kind: String,
    pub cwd: String,
    pub session_id: String,
    pub status: String,
    pub pid: u32,
    pub resume_cmd: Vec<String>,
}

/// Structured scan input for [`HostDatabase::reconcile_sleep`]: the agents seen
/// live, plus each probed session's live terminal pane ids (`None` = the probe
/// failed, so treat panes as unknown and fail open).
#[derive(Clone, Debug, Default)]
pub(crate) struct SleepScan {
    pub live: Vec<(AgentId, SleepObservation)>,
    pub panes: std::collections::BTreeMap<String, Option<std::collections::HashSet<u32>>>,
}

/// Composite key: `AgentId` is (session, pane); the tab is not identity.
fn sleep_key(id: &AgentId) -> String {
    format!("{}\t{}", id.session, id.pane_id)
}

fn parse_sleep_key(key: &str) -> Option<AgentId> {
    let (session, pane) = key.split_once('\t')?;
    Some(AgentId {
        session: session.to_owned(),
        pane_id: pane.parse().ok()?,
    })
}

pub(crate) struct HostDatabase(Database);

fn db_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

impl HostDatabase {
    /// Import once, in the same durable transaction as the schema marker.
    /// Original files remain available for rollback; never re-import them.
    pub(crate) fn open(path: &Path, legacy: &Path) -> io::Result<Self> {
        let db = Database::builder()
            .set_cache_size(4 * 1024 * 1024)
            .create(path)
            .map_err(db_error)?;
        let mut tx = db.begin_write().map_err(db_error)?;
        tx.set_durability(Durability::Immediate).map_err(db_error)?;
        {
            let mut table = tx.open_table(STATE).map_err(db_error)?;
            let schema = table
                .get("schema")
                .map_err(db_error)?
                .map(|v| v.value().to_owned());
            match schema.as_deref() {
                Some(SCHEMA) => {}
                Some(other) => {
                    return Err(io::Error::other(format!(
                        "unsupported database schema {other}"
                    )))
                }
                None => {
                    let scan = read_legacy(&legacy.join("scan"))?;
                    let places = format_places(merge_places(
                        parse_places(
                            &read_legacy(&legacy.join("places.host"))?.unwrap_or_default(),
                        ),
                        parse_places(&read_legacy(&legacy.join("places"))?.unwrap_or_default()),
                    ));
                    table.insert("schema", SCHEMA).map_err(db_error)?;
                    table.insert("revision", "1").map_err(db_error)?;
                    if let Some(scan) = scan.filter(|text| !text.trim().is_empty()) {
                        table.insert("scan", scan.as_str()).map_err(db_error)?;
                    }
                    table.insert("places", places.as_str()).map_err(db_error)?;
                }
            }
        }
        tx.commit().map_err(db_error)?;
        Ok(Self(db))
    }

    pub(crate) fn snapshot(&self) -> io::Result<Snapshot> {
        let tx = self.0.begin_read().map_err(db_error)?;
        let table = tx.open_table(STATE).map_err(db_error)?;
        let get = |key| -> io::Result<Option<String>> {
            Ok(table
                .get(key)
                .map_err(db_error)?
                .map(|v| v.value().to_owned()))
        };
        let revision = get("revision")?
            .unwrap_or_default()
            .parse()
            .map_err(db_error)?;
        let scan = get("scan")?;
        let places = get("places")?.unwrap_or_default();
        drop(table);
        drop(tx);
        Ok(Snapshot {
            revision,
            scan,
            places,
            sleep: self.sleep_rows()?,
        })
    }

    /// Relationship rows flattened for clients (observability only).
    fn sleep_rows(&self) -> io::Result<Vec<SleepRow>> {
        Ok(self
            .sleep_records()?
            .into_iter()
            .map(|(id, record)| SleepRow {
                session: id.session,
                pane_id: id.pane_id,
                kind: record.kind,
                session_id: record.session_id,
                phase: record.phase,
                status: record.status,
                cwd: record.cwd,
            })
            .collect())
    }

    /// A scan and its title changes become visible together, after fsync.
    pub(crate) fn publish(&self, scan: &str, places: Vec<(AgentId, PanePlace)>) -> io::Result<()> {
        if scan.trim().is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty scan"));
        }
        let previous = self.snapshot()?;
        let places = format_places(replace_session_places(
            parse_places(&previous.places),
            places,
        ));
        if previous.scan.as_deref() == Some(scan) && previous.places == places {
            return Ok(());
        }
        let revision = previous
            .revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("revision overflow"))?
            .to_string();
        let mut tx = self.0.begin_write().map_err(db_error)?;
        tx.set_durability(Durability::Immediate).map_err(db_error)?;
        {
            let mut table = tx.open_table(STATE).map_err(db_error)?;
            table.insert("scan", scan).map_err(db_error)?;
            table.insert("places", places.as_str()).map_err(db_error)?;
            table
                .insert("revision", revision.as_str())
                .map_err(db_error)?;
        }
        tx.commit().map_err(db_error)
    }

    /// Insert or replace one agent's resume relationship, durably.
    pub(crate) fn upsert_sleep(&self, id: &AgentId, record: &SleepRecord) -> io::Result<()> {
        let value = serde_json::to_string(record)?;
        let mut tx = self.0.begin_write().map_err(db_error)?;
        tx.set_durability(Durability::Immediate).map_err(db_error)?;
        {
            let mut table = tx.open_table(SLEEP).map_err(db_error)?;
            table
                .insert(sleep_key(id).as_str(), value.as_str())
                .map_err(db_error)?;
        }
        tx.commit().map_err(db_error)
    }

    /// One agent's relationship, or `None` if never recorded.
    pub(crate) fn sleep_record(&self, id: &AgentId) -> io::Result<Option<SleepRecord>> {
        let tx = self.0.begin_read().map_err(db_error)?;
        let table = match tx.open_table(SLEEP) {
            Ok(table) => table,
            // The table only exists once something has been recorded.
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(db_error(error)),
        };
        match table.get(sleep_key(id).as_str()).map_err(db_error)? {
            Some(value) => Ok(Some(serde_json::from_str(value.value())?)),
            None => Ok(None),
        }
    }

    /// Every recorded relationship, for reconciliation and diagnostics.
    pub(crate) fn sleep_records(&self) -> io::Result<Vec<(AgentId, SleepRecord)>> {
        let tx = self.0.begin_read().map_err(db_error)?;
        let table = match tx.open_table(SLEEP) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(error) => return Err(db_error(error)),
        };
        let mut out = Vec::new();
        for entry in table.iter().map_err(db_error)? {
            let (key, value) = entry.map_err(db_error)?;
            let Some(id) = parse_sleep_key(key.value()) else {
                continue;
            };
            out.push((id, serde_json::from_str(value.value())?));
        }
        Ok(out)
    }

    /// Fold one scan into the relationship table, driving the lifecycle a
    /// supervisor used to observe directly:
    /// - a live agent (re)writes its row as `running` (or stays `stopping`
    ///   while the exact pid we asked to exit is still alive);
    /// - a row whose agent is gone but whose pane survives becomes `sleeping`
    ///   if we asked it to stop, else `failed` (an unexpected disappearance);
    /// - a row whose pane is gone is forgotten.
    pub(crate) fn reconcile_sleep(&self, scan: &SleepScan, now: u64) -> io::Result<()> {
        use std::collections::HashSet;
        let mut live_ids: HashSet<AgentId> = HashSet::new();
        for (id, obs) in &scan.live {
            live_ids.insert(id.clone());
            let mut record = self.sleep_record(id)?.unwrap_or_default();
            record.kind = obs.kind.clone();
            if !obs.cwd.is_empty() {
                record.cwd = obs.cwd.clone();
            }
            // Session id and status come together from the registry file; a
            // live pid whose file is not written yet must not clobber either.
            if !obs.session_id.is_empty() {
                record.session_id = obs.session_id.clone();
                record.status = obs.status.clone();
            }
            if !obs.resume_cmd.is_empty() {
                record.resume_cmd = obs.resume_cmd.clone();
            }
            // Keep `stopping` only while the very process we asked to exit is
            // still alive; any other live pid means it is up and running.
            let still_exiting = record.phase == "stopping" && record.last_pid == Some(obs.pid);
            record.phase = if still_exiting { "stopping" } else { "running" }.into();
            record.last_pid = Some(obs.pid);
            record.missing_since = None;
            record.updated_at = now;
            self.upsert_sleep(id, &record)?;
        }
        for (id, mut record) in self.sleep_records()? {
            if live_ids.contains(&id) {
                continue;
            }
            // Only a definitely-closed pane prunes the row; an unprobed or
            // failed-probe session keeps it (fail open).
            let pane_gone = matches!(scan.panes.get(&id.session), Some(Some(ids)) if !ids.contains(&id.pane_id));
            if pane_gone {
                // Debounce: hold the row until the pane stays gone past the
                // grace window, so one racy probe cannot drop a resumable agent.
                let since = record.missing_since.unwrap_or(now);
                if now.saturating_sub(since) >= PANE_GONE_GRACE_SECS {
                    self.remove_sleep(&id)?;
                } else if record.missing_since.is_none() {
                    record.missing_since = Some(now);
                    record.updated_at = now;
                    self.upsert_sleep(&id, &record)?;
                }
                continue;
            }
            // Pane present: forget any transient miss and advance the phase.
            let next = match record.phase.as_str() {
                "stopping" => "sleeping",
                "running" => "failed",
                other => other,
            };
            let phase_changed = next != record.phase;
            if phase_changed || record.missing_since.is_some() {
                if phase_changed {
                    record.phase = next.into();
                    record.status.clear();
                    record.last_pid = None;
                }
                record.missing_since = None;
                record.updated_at = now;
                self.upsert_sleep(&id, &record)?;
            }
        }
        Ok(())
    }

    /// Forget one agent's relationship (its pane is gone).
    pub(crate) fn remove_sleep(&self, id: &AgentId) -> io::Result<()> {
        let mut tx = self.0.begin_write().map_err(db_error)?;
        tx.set_durability(Durability::Immediate).map_err(db_error)?;
        {
            let mut table = tx.open_table(SLEEP).map_err(db_error)?;
            table.remove(sleep_key(id).as_str()).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)
    }
}

fn read_legacy(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_and_commits_survive_reopen_without_reimporting_stale_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("scan"), "META hooks=1\n").unwrap();
        std::fs::write(
            dir.path().join("places.host"),
            "PLACE remote 1 0\tmain\tremote\n",
        )
        .unwrap();
        let path = dir.path().join("state.redb");
        let db = HostDatabase::open(&path, dir.path()).unwrap();
        assert_eq!(
            db.snapshot().unwrap().scan.as_deref(),
            Some("META hooks=1\n")
        );
        db.publish(
            "META hooks=0\n",
            parse_places("PLACE home 2 0\tmain\tnew\n"),
        )
        .unwrap();
        let committed = db.snapshot().unwrap();
        assert!(committed.places.contains("remote"));
        drop(db);
        assert_eq!(
            HostDatabase::open(&path, dir.path())
                .unwrap()
                .snapshot()
                .unwrap(),
            committed
        );
    }

    fn sample_record() -> SleepRecord {
        SleepRecord {
            kind: "claude".into(),
            cwd: "/tmp/ww".into(),
            session_id: "7c61f52f-5b66-46e7-a5dc-e2283947dfd1".into(),
            resume_cmd: vec![
                "claude".into(),
                "-r".into(),
                "7c61f52f-5b66-46e7-a5dc-e2283947dfd1".into(),
            ],
            phase: "sleeping".into(),
            status: "idle".into(),
            last_pid: Some(4242),
            missing_since: None,
            updated_at: 1_700_000_000,
        }
    }

    #[test]
    fn sleep_table_absent_reads_as_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("scan"), "META hooks=1\n").unwrap();
        let db = HostDatabase::open(&dir.path().join("state.redb"), dir.path()).unwrap();
        // Never written: neither a lookup nor a listing may fail.
        let id = AgentId {
            session: "work".into(),
            pane_id: 1,
        };
        assert_eq!(db.sleep_record(&id).unwrap(), None);
        assert!(db.sleep_records().unwrap().is_empty());
    }

    #[test]
    fn sleep_relationship_upserts_lists_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("scan"), "META hooks=1\n").unwrap();
        let path = dir.path().join("state.redb");
        let db = HostDatabase::open(&path, dir.path()).unwrap();
        let id = AgentId {
            session: "work".into(),
            pane_id: 7,
        };
        let mut record = sample_record();
        db.upsert_sleep(&id, &record).unwrap();
        assert_eq!(db.sleep_record(&id).unwrap().as_ref(), Some(&record));

        // Upsert replaces in place, keyed by (session, pane).
        record.phase = "running".into();
        record.last_pid = Some(5555);
        db.upsert_sleep(&id, &record).unwrap();
        let listed = db.sleep_records().unwrap();
        assert_eq!(listed, vec![(id.clone(), record.clone())]);

        // A durable row is readable by a fresh owner after reopen.
        drop(db);
        let reopened = HostDatabase::open(&path, dir.path()).unwrap();
        assert_eq!(reopened.sleep_record(&id).unwrap(), Some(record));
        reopened.remove_sleep(&id).unwrap();
        assert_eq!(reopened.sleep_record(&id).unwrap(), None);
        assert!(reopened.sleep_records().unwrap().is_empty());
    }

    fn obs(pid: u32) -> SleepObservation {
        SleepObservation {
            kind: "claude".into(),
            cwd: "/tmp/ww".into(),
            session_id: "sid-1".into(),
            status: "idle".into(),
            pid,
            resume_cmd: vec!["claude".into(), "-r".into(), "sid-1".into()],
        }
    }

    fn scan_with(
        live: Vec<(AgentId, SleepObservation)>,
        panes: &[(&str, Option<&[u32]>)],
    ) -> SleepScan {
        SleepScan {
            live,
            panes: panes
                .iter()
                .map(|(session, ids)| {
                    (
                        (*session).to_owned(),
                        ids.map(|ids| ids.iter().copied().collect()),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn reconcile_records_live_agent_then_sleeps_and_resumes_over_scans() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("scan"), "META hooks=1\n").unwrap();
        let db = HostDatabase::open(&dir.path().join("state.redb"), dir.path()).unwrap();
        let id = AgentId {
            session: "work".into(),
            pane_id: 1,
        };

        // First scan: a live claude becomes a running row with its resume cmd.
        db.reconcile_sleep(
            &scan_with(vec![(id.clone(), obs(100))], &[("work", Some(&[1]))]),
            1,
        )
        .unwrap();
        let record = db.sleep_record(&id).unwrap().unwrap();
        assert_eq!(record.phase, "running");
        assert_eq!(record.session_id, "sid-1");
        assert_eq!(record.resume_cmd, vec!["claude", "-r", "sid-1"]);
        assert_eq!(record.last_pid, Some(100));

        // Board asked it to sleep: phase becomes stopping while pid 100 lives.
        let mut stopping = record;
        stopping.phase = "stopping".into();
        db.upsert_sleep(&id, &stopping).unwrap();
        db.reconcile_sleep(
            &scan_with(vec![(id.clone(), obs(100))], &[("work", Some(&[1]))]),
            2,
        )
        .unwrap();
        assert_eq!(db.sleep_record(&id).unwrap().unwrap().phase, "stopping");

        // Next scan: pid gone but pane 1 survives → sleeping, session kept.
        db.reconcile_sleep(&scan_with(vec![], &[]), 3).unwrap();
        let asleep = db.sleep_record(&id).unwrap().unwrap();
        assert_eq!(asleep.phase, "sleeping");
        assert_eq!(asleep.session_id, "sid-1");
        assert_eq!(asleep.last_pid, None);

        // Resume brings a new pid: reconcile flips it back to running.
        db.reconcile_sleep(
            &scan_with(vec![(id.clone(), obs(200))], &[("work", Some(&[1]))]),
            4,
        )
        .unwrap();
        let awake = db.sleep_record(&id).unwrap().unwrap();
        assert_eq!(awake.phase, "running");
        assert_eq!(awake.last_pid, Some(200));
    }

    #[test]
    fn reconcile_marks_unexpected_disappearance_failed_and_prunes_closed_pane() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("scan"), "META hooks=1\n").unwrap();
        let db = HostDatabase::open(&dir.path().join("state.redb"), dir.path()).unwrap();
        let id = AgentId {
            session: "work".into(),
            pane_id: 2,
        };
        db.reconcile_sleep(
            &scan_with(vec![(id.clone(), obs(300))], &[("work", Some(&[2]))]),
            1,
        )
        .unwrap();

        // Running agent vanished without a sleep request, pane 2 still there.
        db.reconcile_sleep(&scan_with(vec![], &[("work", Some(&[2]))]), 2)
            .unwrap();
        assert_eq!(db.sleep_record(&id).unwrap().unwrap().phase, "failed");

        // A failed-probe session (None) must not prune: fail open.
        db.reconcile_sleep(&scan_with(vec![], &[("work", None)]), 3)
            .unwrap();
        assert!(db.sleep_record(&id).unwrap().is_some());

        // The pane reads as gone, but one probe must not prune: the row is held
        // with a missing marker through the grace window.
        db.reconcile_sleep(&scan_with(vec![], &[("work", Some(&[9]))]), 4)
            .unwrap();
        assert!(db.sleep_record(&id).unwrap().is_some());

        // Still gone past the grace window → the row is finally forgotten.
        db.reconcile_sleep(
            &scan_with(vec![], &[("work", Some(&[9]))]),
            4 + PANE_GONE_GRACE_SECS,
        )
        .unwrap();
        assert!(db.sleep_record(&id).unwrap().is_none());
    }

    #[test]
    fn reconcile_holds_row_through_a_transient_pane_probe_miss() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("scan"), "META hooks=1\n").unwrap();
        let db = HostDatabase::open(&dir.path().join("state.redb"), dir.path()).unwrap();
        let id = AgentId {
            session: "work".into(),
            pane_id: 2,
        };
        // A sleeping row (pane kept, waiting for resume).
        db.reconcile_sleep(
            &scan_with(vec![(id.clone(), obs(300))], &[("work", Some(&[2]))]),
            1,
        )
        .unwrap();
        let mut sleeping = db.sleep_record(&id).unwrap().unwrap();
        sleeping.phase = "sleeping".into();
        db.upsert_sleep(&id, &sleeping).unwrap();

        // A single glitchy probe drops pane 2: keep the row, mark it missing.
        db.reconcile_sleep(&scan_with(vec![], &[("work", Some(&[9]))]), 2)
            .unwrap();
        let held = db.sleep_record(&id).unwrap().unwrap();
        assert_eq!(held.phase, "sleeping");
        assert_eq!(held.missing_since, Some(2));

        // Pane 2 comes back within grace → the marker clears, row survives.
        db.reconcile_sleep(&scan_with(vec![], &[("work", Some(&[2]))]), 3)
            .unwrap();
        let recovered = db.sleep_record(&id).unwrap().unwrap();
        assert_eq!(recovered.phase, "sleeping");
        assert_eq!(recovered.missing_since, None);

        // A later real close still prunes after the grace window.
        db.reconcile_sleep(&scan_with(vec![], &[("work", Some(&[9]))]), 4)
            .unwrap();
        assert!(db.sleep_record(&id).unwrap().is_some());
        db.reconcile_sleep(
            &scan_with(vec![], &[("work", Some(&[9]))]),
            4 + PANE_GONE_GRACE_SECS,
        )
        .unwrap();
        assert!(db.sleep_record(&id).unwrap().is_none());
    }

    #[test]
    fn invalid_legacy_input_does_not_commit_migration_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.redb");
        std::fs::create_dir(dir.path().join("scan")).unwrap();
        assert!(HostDatabase::open(&path, dir.path()).is_err());
        std::fs::remove_dir(dir.path().join("scan")).unwrap();
        std::fs::write(dir.path().join("scan"), "META hooks=1\n").unwrap();
        assert!(HostDatabase::open(&path, dir.path())
            .unwrap()
            .snapshot()
            .unwrap()
            .scan
            .is_some());
    }
}
