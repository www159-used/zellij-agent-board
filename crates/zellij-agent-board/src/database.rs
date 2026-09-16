//! Host-only durable snapshots. Only the daemon opens this database.
use std::io;
use std::path::Path;

use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::protocol::{format_places, merge_places, parse_places, replace_session_places};
use crate::{AgentId, PanePlace};

const STATE: TableDefinition<&str, &str> = TableDefinition::new("state");
const SCHEMA: &str = "1";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub revision: u64,
    pub scan: Option<String>,
    pub places: String,
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
        Ok(Snapshot {
            revision: get("revision")?
                .unwrap_or_default()
                .parse()
                .map_err(db_error)?,
            scan: get("scan")?,
            places: get("places")?.unwrap_or_default(),
        })
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
