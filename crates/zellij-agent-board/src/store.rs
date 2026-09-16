//! Host snapshot publication for scan, titles and status markers.
//! Hook/focus scripts use the same `.pending` + rename idea in bash.

use std::io::{self, Write};
use std::path::Path;

use tempfile::NamedTempFile;

// `.pending` is ignored by scan/prune and spool readers (immediate files only).
fn stage_snapshot(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<NamedTempFile> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "snapshot needs a parent directory",
        )
    })?;
    let staging = parent.join(".pending");
    std::fs::create_dir_all(&staging)?;
    let mut file = NamedTempFile::new_in(staging)?;
    file.write_all(contents.as_ref())?;
    Ok(file)
}

pub(crate) fn write_snapshot(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    stage_snapshot(path, contents)?
        .persist(path)
        .map_err(|err| err.error)?;
    Ok(())
}

/// A concurrent live writer always wins over legacy state migration.
pub(crate) fn write_snapshot_if_absent(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    match stage_snapshot(path, contents)?.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(err) if err.error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err.error),
    }
}

#[cfg(test)]
mod tests {
    use super::{write_snapshot, write_snapshot_if_absent};
    use std::fs::{self, File};
    use std::io::Read;

    #[test]
    fn a_reader_keeps_its_complete_snapshot_while_a_writer_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan");
        let old = b"META hooks=1\nSCAN ww 3 agent agent\n";
        let new = b"META hooks=1\nSCAN lp 8 claude claude\n";
        write_snapshot(&path, old).unwrap();
        let mut reader = File::open(&path).unwrap();
        let mut prefix = [0; 5];
        reader.read_exact(&mut prefix).unwrap();

        write_snapshot(&path, new).unwrap();

        let mut observed = prefix.to_vec();
        reader.read_to_end(&mut observed).unwrap();
        assert_eq!(observed, old, "an in-flight read must not mix snapshots");
        assert_eq!(fs::read(&path).unwrap(), new);
    }

    #[test]
    fn migration_does_not_overwrite_a_live_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/scan");
        write_snapshot_if_absent(&path, "legacy").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "legacy");
        write_snapshot(&path, "live").unwrap();
        write_snapshot_if_absent(&path, "stale").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "live");
        assert_eq!(
            fs::read_dir(path.parent().unwrap().join(".pending"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn failed_publication_cleans_up_the_staged_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan");
        fs::create_dir(&path).unwrap();
        assert!(write_snapshot(&path, "snapshot").is_err());
        assert!(path.is_dir());
        assert_eq!(
            fs::read_dir(dir.path().join(".pending")).unwrap().count(),
            0
        );
    }
}
