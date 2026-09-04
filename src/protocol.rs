//! File / pipe protocol between the WASM bridge and the host TUI.
//!
//! Durable host state lives in `~/.cache/zellij-agent-board`: titles, seen,
//! started, focus, and the last SCAN snapshot. First paint reads the
//! snapshot; a live scan only patches rows. Hook spool stays in `$TMPDIR`.

use std::path::{Path, PathBuf};

use crate::agent::{AgentId, PanePlace};

/// `zellij pipe --name` — never pass `--plugin` or Zellij will launch WASM.
pub const PIPE_NAME: &str = "zellij-agent-board";

/// Host-owned store. `ZAB_STATE_DIR` wins so e2e / tests stay isolated.
pub fn runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ZAB_STATE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if cfg!(test) {
        return tmp_root().join("zellij-agent-board");
    }
    state_dir_from(
        None,
        std::env::var("XDG_CACHE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        &tmp_root(),
    )
}

pub fn state_dir_from(
    zab_state_dir: Option<&str>,
    xdg_cache_home: Option<&str>,
    home: Option<&str>,
    tmp: &Path,
) -> PathBuf {
    if let Some(dir) = zab_state_dir.filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(xdg) = xdg_cache_home.filter(|dir| !dir.is_empty()) {
        return PathBuf::from(xdg).join("zellij-agent-board");
    }
    if let Some(home) = home.filter(|dir| !dir.is_empty()) {
        return PathBuf::from(home).join(".cache/zellij-agent-board");
    }
    tmp.join("zellij-agent-board")
}

pub fn places_path() -> PathBuf {
    runtime_dir().join("places")
}

/// Leftover TUI-only file. Read on migrate, then stop writing it.
#[cfg(not(target_arch = "wasm32"))]
pub fn host_places_path() -> PathBuf {
    runtime_dir().join("places.host")
}

pub fn focus_path() -> PathBuf {
    runtime_dir().join("focus")
}

pub fn spool_dir() -> PathBuf {
    tmp_root().join("zellij-agent-board-spool")
}

pub fn seen_dir() -> PathBuf {
    runtime_dir().join("seen")
}

pub fn started_dir() -> PathBuf {
    runtime_dir().join("started")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn scan_path() -> PathBuf {
    runtime_dir().join("scan")
}

fn tmp_root() -> PathBuf {
    std::env::var("TMPDIR")
        .or_else(|_| std::env::var("TMP"))
        .unwrap_or_else(|_| "/tmp".into())
        .into()
}

pub fn format_place_line(id: &AgentId, place: &PanePlace) -> String {
    format!(
        "PLACE {} {} {}\t{}\t{}",
        id.session, id.pane_id, place.tab_position, place.tab_name, place.pane_title
    )
}

pub fn parse_place_line(line: &str) -> Option<(AgentId, PanePlace)> {
    let line = line.trim();
    let rest = line.strip_prefix("PLACE ")?;
    let (session, rest) = rest.split_once(char::is_whitespace)?;
    let rest = rest.trim_start();
    let (pane, rest) = rest.split_once(char::is_whitespace)?;
    let rest = rest.trim_start();
    let (tab, names) = rest
        .split_once('\t')
        .or_else(|| rest.split_once(char::is_whitespace))?;
    let pane_id = pane.parse().ok()?;
    let tab_position = tab.parse().ok()?;
    let (tab_name, pane_title) = names
        .split_once('\t')
        .map(|(tab_name, pane_title)| (tab_name.to_string(), pane_title.to_string()))
        .unwrap_or_else(|| (names.to_string(), String::new()));
    Some((
        AgentId {
            session: session.to_string(),
            pane_id,
        },
        PanePlace {
            tab_position,
            tab_name,
            pane_title,
        },
    ))
}

pub fn format_places<I>(places: I) -> String
where
    I: IntoIterator<Item = (AgentId, PanePlace)>,
{
    let mut lines: Vec<String> = places
        .into_iter()
        .map(|(id, place)| format_place_line(&id, &place))
        .collect();
    lines.sort();
    lines.join("\n")
}

pub fn parse_places(text: &str) -> Vec<(AgentId, PanePlace)> {
    text.lines().filter_map(parse_place_line).collect()
}

/// Union `incoming` onto `existing`. Blank incoming names keep the old ones,
/// and keys only in `existing` stay so a later session cannot wipe a new one.
pub fn merge_places(
    existing: impl IntoIterator<Item = (AgentId, PanePlace)>,
    incoming: impl IntoIterator<Item = (AgentId, PanePlace)>,
) -> Vec<(AgentId, PanePlace)> {
    let mut map: std::collections::BTreeMap<AgentId, PanePlace> = existing.into_iter().collect();
    for (id, place) in incoming {
        map.insert(
            id.clone(),
            match map.get(&id) {
                Some(old) => place.keep_names(&old.tab_name, &old.pane_title),
                None => place,
            },
        );
    }
    map.into_iter().collect()
}

/// A successful `list-panes` for a session replaces that session's rows.
/// Stale pane ids (leftover `board-tui` floats) would never match SCAN and
/// would keep titles from attaching. Other sessions stay put. Empty
/// `incoming` is a no-op so a failed list does not wipe.
pub fn replace_session_places(
    existing: impl IntoIterator<Item = (AgentId, PanePlace)>,
    incoming: impl IntoIterator<Item = (AgentId, PanePlace)>,
) -> Vec<(AgentId, PanePlace)> {
    let incoming: Vec<(AgentId, PanePlace)> = incoming.into_iter().collect();
    if incoming.is_empty() {
        return existing.into_iter().collect();
    }
    let sessions: std::collections::BTreeSet<String> =
        incoming.iter().map(|(id, _)| id.session.clone()).collect();
    let kept: Vec<(AgentId, PanePlace)> = existing
        .into_iter()
        .filter(|(id, _)| !sessions.contains(&id.session))
        .collect();
    merge_places(kept, incoming)
}

pub fn format_jump(session: &str, pane_id: u32) -> String {
    format!("JUMP {session} {pane_id}")
}

pub fn format_seen(session: &str, pane_id: u32, finished_at: u64) -> String {
    format!("SEEN {session} {pane_id} {finished_at}")
}

pub fn format_started(session: &str, pane_id: u32, started_at: u64) -> String {
    format!("STARTED {session} {pane_id} {started_at}")
}

pub fn format_focus(session: &str, pane_id: u32) -> String {
    format!("FOCUS {session} {pane_id}")
}

#[cfg(not(target_arch = "wasm32"))]
fn ensure_migrated() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(migrate_tmpdir_state);
}

#[cfg(not(target_arch = "wasm32"))]
fn migrate_tmpdir_state() {
    let dest = runtime_dir();
    let src = tmp_root().join("zellij-agent-board");
    if src == dest || !src.is_dir() {
        return;
    }
    let _ = std::fs::create_dir_all(&dest);
    copy_if_absent(&src.join("focus"), &dest.join("focus"));
    copy_if_absent(&src.join("scan"), &dest.join("scan"));
    copy_if_absent(&src.join("scan.host"), &dest.join("scan"));
    copy_dir_if_absent(&src.join("seen"), &dest.join("seen"));
    copy_dir_if_absent(&src.join("started"), &dest.join("started"));
    let places = dest.join("places");
    if !places.exists() {
        let merged = merge_places(
            parse_places(&std::fs::read_to_string(src.join("places")).unwrap_or_default()),
            parse_places(&std::fs::read_to_string(src.join("places.host")).unwrap_or_default()),
        );
        if !merged.is_empty() {
            let _ = std::fs::write(&places, format_places(merged));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn copy_if_absent(src: &Path, dest: &Path) {
    if dest.exists() || !src.exists() {
        return;
    }
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::copy(src, dest);
}

#[cfg(not(target_arch = "wasm32"))]
fn copy_dir_if_absent(src: &Path, dest: &Path) {
    let Ok(entries) = std::fs::read_dir(src) else {
        return;
    };
    for entry in entries.flatten() {
        copy_if_absent(&entry.path(), &dest.join(entry.file_name()));
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn ensure_state() {
    ensure_migrated();
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_places() -> Vec<(AgentId, PanePlace)> {
    ensure_migrated();
    let places = parse_places(&std::fs::read_to_string(places_path()).unwrap_or_default());
    let leftover = parse_places(&std::fs::read_to_string(host_places_path()).unwrap_or_default());
    if leftover.is_empty() {
        return places;
    }
    let merged = merge_places(places, leftover);
    if std::fs::create_dir_all(runtime_dir()).is_ok() {
        let _ = std::fs::write(places_path(), format_places(merged.clone()));
        let _ = std::fs::remove_file(host_places_path());
    }
    merged
}

#[cfg(not(target_arch = "wasm32"))]
pub fn persist_places(incoming: impl IntoIterator<Item = (AgentId, PanePlace)>) {
    let incoming: Vec<(AgentId, PanePlace)> = incoming.into_iter().collect();
    if incoming.is_empty() {
        return;
    }
    ensure_migrated();
    if std::fs::create_dir_all(runtime_dir()).is_err() {
        return;
    }
    let path = places_path();
    let existing = parse_places(&std::fs::read_to_string(&path).unwrap_or_default());
    let leftover = parse_places(&std::fs::read_to_string(host_places_path()).unwrap_or_default());
    let merged = replace_session_places(merge_places(existing, leftover), incoming);
    let _ = std::fs::write(&path, format_places(merged));
    let _ = std::fs::remove_file(host_places_path());
}

#[cfg(not(target_arch = "wasm32"))]
pub fn persist_scan(text: &str) {
    if text.trim().is_empty() {
        return;
    }
    ensure_migrated();
    if std::fs::create_dir_all(runtime_dir()).is_err() {
        return;
    }
    let _ = std::fs::write(scan_path(), text);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_scan() -> Option<String> {
    ensure_migrated();
    let text = std::fs::read_to_string(scan_path()).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn persist_seen(session: &str, pane_id: u32, finished_at: u64) {
    ensure_migrated();
    let dir = seen_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("{session}-{pane_id}"));
    let _ = std::fs::write(
        path,
        format!("{}\n", format_seen(session, pane_id, finished_at)),
    );
}

#[cfg(not(target_arch = "wasm32"))]
pub fn persist_started(session: &str, pane_id: u32, started_at: u64) {
    ensure_migrated();
    let dir = started_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("{session}-{pane_id}"));
    let _ = std::fs::write(
        path,
        format!("{}\n", format_started(session, pane_id, started_at)),
    );
}

#[cfg(not(target_arch = "wasm32"))]
pub fn clear_started(session: &str, pane_id: u32) {
    ensure_migrated();
    let path = started_dir().join(format!("{session}-{pane_id}"));
    let _ = std::fs::remove_file(path);
}

pub fn parse_focus(text: &str) -> Option<(String, u32)> {
    let mut parts = text.split_whitespace();
    if parts.next()? != "FOCUS" {
        return None;
    }
    let session = parts.next()?.to_string();
    let pane_id = parts.next()?.parse().ok()?;
    Some((session, pane_id))
}

pub fn parse_jump(payload: &str) -> Option<(String, u32)> {
    let mut parts = payload.split_whitespace();
    if parts.next()? != "JUMP" {
        return None;
    }
    let session = parts.next()?.to_string();
    let pane_id = parts.next()?.parse().ok()?;
    Some((session, pane_id))
}

#[cfg(test)]
mod tests {
    use super::{
        format_focus, format_jump, format_place_line, format_seen, merge_places, parse_focus,
        parse_jump, parse_place_line, parse_places, state_dir_from,
    };
    use crate::discover::parse_host_line;
    use crate::{AgentId, PanePlace};

    #[test]
    fn place_round_trip_keeps_spaces_in_names() {
        let id = AgentId {
            session: "ww".into(),
            pane_id: 3,
        };
        let place = PanePlace {
            tab_position: 1,
            tab_name: "geo db".into(),
            pane_title: "agent ls".into(),
        };
        let line = format_place_line(&id, &place);
        assert_eq!(parse_place_line(&line), Some((id, place)));
    }

    #[test]
    fn jump_round_trip() {
        assert_eq!(parse_jump(&format_jump("lp", 8)), Some(("lp".into(), 8)));
        assert_eq!(parse_jump("HOOK ww 3 stop"), None);
    }

    #[test]
    fn seen_and_focus_lines_round_trip() {
        let seen = format_seen("ww", 3, 1_700_000_000);
        assert_eq!(
            parse_host_line(&seen),
            Some(crate::HostLine::Seen {
                id: crate::AgentId {
                    session: "ww".into(),
                    pane_id: 3,
                },
                finished_at: 1_700_000_000,
            })
        );
        assert_eq!(
            parse_focus(&format_focus("mysql_syncer", 2)),
            Some(("mysql_syncer".into(), 2))
        );
        assert_eq!(parse_focus("PLACE ww 3"), None);
    }

    #[test]
    fn parses_a_places_file() {
        let places = parse_places(
            "PLACE ww 3 1\tgeo db\tagent\n\
             PLACE lp 8 0\tmain\t\n",
        );
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].0.session, "ww");
        assert_eq!(places[0].1.tab_name, "geo db");
        assert_eq!(places[1].1.pane_title, "");
    }

    #[test]
    fn merge_places_keeps_other_sessions_and_non_blank_names() {
        let existing = parse_places(
            "PLACE lp 0 0\tmaster\tlog_parser2\n\
             PLACE ww 3 1\told\told-title\n",
        );
        let incoming = parse_places(
            "PLACE ww 3 1\tgeo db\tagent\n\
             PLACE ww 4 1\tnew-tab\t\n",
        );
        let merged = merge_places(existing, incoming);
        let lp = merged.iter().find(|(id, _)| id.session == "lp").unwrap();
        let ww3 = merged.iter().find(|(id, _)| id.pane_id == 3).unwrap();
        let ww4 = merged.iter().find(|(id, _)| id.pane_id == 4).unwrap();
        assert_eq!(lp.1.pane_title, "log_parser2");
        assert_eq!(ww3.1.tab_name, "geo db");
        assert_eq!(ww3.1.pane_title, "agent");
        assert_eq!(ww4.1.tab_name, "new-tab");
    }

    #[test]
    fn replace_session_places_drops_stale_ids_for_that_session() {
        let existing = parse_places(
            "PLACE lp 0 0\tmaster\tlog_parser2\n\
             PLACE zab 553 3\tfeature/better-search\tboard-tui\n\
             PLACE zab 11 3\told-title\tSearch First Design\n",
        );
        let incoming = parse_places("PLACE zab 11 3\tfeature/better-search\tSearch First Design\n");
        let replaced = super::replace_session_places(existing, incoming);
        assert!(replaced.iter().any(|(id, _)| id.session == "lp"));
        assert!(replaced.iter().any(|(id, place)| id.session == "zab"
            && id.pane_id == 11
            && !place.tab_name.is_empty()));
        assert!(
            !replaced
                .iter()
                .any(|(id, _)| id.session == "zab" && id.pane_id == 553),
            "stale zab pane must not linger:\n{replaced:?}"
        );
    }

    #[test]
    fn replace_session_places_empty_incoming_keeps_existing() {
        let existing = parse_places("PLACE zab 11 3\tfeature/better-search\tagent\n");
        let replaced = super::replace_session_places(existing.clone(), Vec::new());
        assert_eq!(replaced, existing);
    }

    #[test]
    fn state_dir_prefers_override_then_xdg_then_home() {
        let tmp = std::path::Path::new("/tmp");
        assert_eq!(
            state_dir_from(Some("/e2e/state"), Some("/xdg"), Some("/Users/ww"), tmp),
            std::path::PathBuf::from("/e2e/state")
        );
        assert_eq!(
            state_dir_from(None, Some("/xdg"), Some("/Users/ww"), tmp),
            std::path::PathBuf::from("/xdg/zellij-agent-board")
        );
        assert_eq!(
            state_dir_from(None, None, Some("/Users/ww"), tmp),
            std::path::PathBuf::from("/Users/ww/.cache/zellij-agent-board")
        );
        assert_eq!(
            state_dir_from(None, None, None, tmp),
            tmp.join("zellij-agent-board")
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn scan_snapshot_round_trips() {
        use super::{load_scan, persist_scan};
        let text = "META hooks=1\nSCAN ww 3 agent /bin/agent --workspace /tmp/ww\n";
        persist_scan(text);
        assert_eq!(load_scan().as_deref(), Some(text));
    }
}
