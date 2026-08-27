//! File / pipe protocol between the WASM bridge and the host TUI.

use crate::agent::{AgentId, PanePlace};

/// `zellij pipe --name` — never pass `--plugin` or Zellij will launch WASM.
pub const PIPE_NAME: &str = "zellij-agent-board";

pub fn runtime_dir() -> std::path::PathBuf {
    tmp_root().join("zellij-agent-board")
}

pub fn places_path() -> std::path::PathBuf {
    runtime_dir().join("places")
}

/// TUI-owned cache. The WASM bridge still snapshots `places` and can wipe it.
pub fn host_places_path() -> std::path::PathBuf {
    runtime_dir().join("places.host")
}

pub fn focus_path() -> std::path::PathBuf {
    runtime_dir().join("focus")
}

pub fn spool_dir() -> std::path::PathBuf {
    tmp_root().join("zellij-agent-board-spool")
}

pub fn seen_dir() -> std::path::PathBuf {
    runtime_dir().join("seen")
}

pub fn started_dir() -> std::path::PathBuf {
    runtime_dir().join("started")
}

fn tmp_root() -> std::path::PathBuf {
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
pub fn load_places() -> Vec<(AgentId, PanePlace)> {
    let wasm = std::fs::read_to_string(places_path()).unwrap_or_default();
    let host = std::fs::read_to_string(host_places_path()).unwrap_or_default();
    merge_places(parse_places(&wasm), parse_places(&host))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn persist_places(incoming: impl IntoIterator<Item = (AgentId, PanePlace)>) {
    let incoming: Vec<(AgentId, PanePlace)> = incoming.into_iter().collect();
    if incoming.is_empty() {
        return;
    }
    if std::fs::create_dir_all(runtime_dir()).is_err() {
        return;
    }
    let host_path = host_places_path();
    let host = std::fs::read_to_string(&host_path).unwrap_or_default();
    let merged = merge_places(parse_places(&host), incoming);
    let text = format_places(merged);
    let _ = std::fs::write(&host_path, text);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn persist_seen(session: &str, pane_id: u32, finished_at: u64) {
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
        parse_jump, parse_place_line, parse_places,
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
}
