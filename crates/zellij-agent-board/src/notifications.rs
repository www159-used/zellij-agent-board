//! Pending notifications are independent of the latest hook and live scan.
//! Hooks atomically publish one private file per notice; only explicit
//! acknowledgement removes it. No daemon is needed while agents run.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub session: String,
    pub pane_id: u32,
    pub event: String,
    pub detail: String,
    pub at_epoch: u64,
    pub stamp: String,
}

impl Notification {
    pub fn label(&self) -> &str {
        match self.event.as_str() {
            "stop" => "done",
            "permissionRequest" => "waiting",
            "stopFailure" => "failed",
            _ => &self.event,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn pending(dir: &std::path::Path) -> std::io::Result<Vec<Notification>> {
    use crate::{parse_host_line, HostLine};
    use std::{fs, io};
    let entries = match fs::read_dir(dir.join("notifications")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut notices = Vec::new();
    for entry in entries {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !id.starts_with("notice.") || !entry.file_type()?.is_file() {
            continue;
        }
        // An acknowledgement by another client may race this directory read.
        let body = match fs::read_to_string(entry.path()) {
            Ok(body) => body,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let Some(HostLine::Hook(hook)) = parse_host_line(&body) else {
            continue;
        };
        let modified = match entry.metadata() {
            Ok(metadata) => metadata.modified()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        notices.push((
            modified,
            Notification {
                id,
                session: hook.id.session,
                pane_id: hook.id.pane_id,
                event: hook.event,
                detail: hook.detail,
                at_epoch: hook.at_epoch.unwrap_or_default(),
                stamp: hook.at_stamp.unwrap_or_default(),
            },
        ));
    }
    notices.sort_by(|(time_a, a), (time_b, b)| time_a.cmp(time_b).then(a.id.cmp(&b.id)));
    Ok(notices.into_iter().map(|(_, notice)| notice).collect())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn acknowledge(dir: &std::path::Path, id: &str) -> std::io::Result<()> {
    use std::{fs, io};
    // The id is a file name supplied by a client; never accept paths.
    if !id.starts_with("notice.") || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid notification id",
        ));
    }
    let queue = dir.join("notifications");
    match fs::remove_file(queue.join(id)) {
        Ok(()) => fs::File::open(queue)?.sync_all(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::{fs, io};

    #[test]
    fn fifo_does_not_follow_pane_name_and_acknowledges_only_the_requested_notice() {
        let dir = tempfile::tempdir().unwrap();
        let queue = dir.path().join("notifications");
        fs::create_dir_all(queue.join(".pending")).unwrap();
        // Deliberately reverse alphabetical ids and equal hook seconds.
        for (id, pane, time) in [("notice.z", 8, 100), ("notice.a", 2, 101)] {
            let path = queue.join(id);
            fs::write(
                &path,
                format!("HOOK home {pane} stop @42 +10-15T17:00 done\n"),
            )
            .unwrap();
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(time))
                .unwrap();
        }
        fs::write(queue.join(".pending/notice.incomplete"), "HOOK home").unwrap();
        let first = pending(dir.path()).unwrap();
        assert_eq!(first.iter().map(|n| n.pane_id).collect::<Vec<_>>(), [8, 2]);
        assert_eq!(pending(dir.path()).unwrap(), first); // Reading is not acknowledgement.
        acknowledge(dir.path(), &first[0].id).unwrap();
        acknowledge(dir.path(), &first[0].id).unwrap(); // Safe retry.
        assert_eq!(pending(dir.path()).unwrap(), first[1..]);
        assert_eq!(
            acknowledge(dir.path(), "notice.../../scan")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(queue.join("notice.a").exists());
    }
}
