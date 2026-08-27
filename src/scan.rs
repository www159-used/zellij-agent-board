//! Native process scan. The host TUI calls this; WASM never does.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent::{keep_cursor_agent, AgentId, PanePlace};
use crate::protocol::{seen_dir, spool_dir, started_dir};

pub fn scan_host_text() -> String {
    let epoch = unix_now();
    let mut out = format!(
        "META hooks={} epoch={epoch}\n",
        if hooks_installed() { 1 } else { 0 }
    );
    let pids = agent_pids();
    if pids.is_empty() {
        return out;
    }
    let args_by_pid = ps_args(&pids);
    let env_by_pid = ps_env(&pids);
    let mut live_keys = Vec::new();
    for pid in &pids {
        let Some(args) = args_by_pid.get(pid) else {
            continue;
        };
        let argv = split_args(args);
        if !keep_cursor_agent(&argv) {
            continue;
        }
        let comm = argv
            .first()
            .map(|bin| bin.rsplit('/').next().unwrap_or(bin).to_string())
            .unwrap_or_else(|| "agent".into());
        if argv.last().map(String::as_str) == Some("ls") && !holding_chat_store(*pid) {
            continue;
        }
        let Some(blob) = env_by_pid.get(pid) else {
            continue;
        };
        let Some((session, pane)) = zellij_ids_from_env_blob(blob) else {
            continue;
        };
        out.push_str(&format!("SCAN {session} {pane} {comm} {args}\n"));
        let key = format!("{session}-{pane}");
        if let Some(hook) = read_spool(&key) {
            out.push_str(&hook);
            if !hook.ends_with('\n') {
                out.push('\n');
            }
        }
        if let Some(seen) = read_seen(&key) {
            out.push_str(&seen);
            if !seen.ends_with('\n') {
                out.push('\n');
            }
        }
        if let Some(started) = read_started(&key) {
            out.push_str(&started);
            if !started.ends_with('\n') {
                out.push('\n');
            }
        }
        live_keys.push(key);
    }
    // A failed match (0 live keys) must not wipe hook history.
    if !live_keys.is_empty() {
        prune_spool(&live_keys);
        prune_dir(&seen_dir(), &live_keys);
        prune_dir(&started_dir(), &live_keys);
    }
    out
}

/// Titles from every session's `list-panes`. The WASM bridge only sees
/// other sessions through SessionUpdate, which often arrives with blank
/// pane names — done/time still come from hooks.
pub fn scan_places() -> Vec<(AgentId, PanePlace)> {
    scan_places_for(&list_sessions())
}

pub fn scan_places_for(sessions: &[String]) -> Vec<(AgentId, PanePlace)> {
    let mut out = Vec::new();
    for session in sessions {
        if session.is_empty() {
            continue;
        }
        let Ok(output) = Command::new(zellij_bin())
            .args([
                "--session",
                session,
                "action",
                "list-panes",
                "--all",
                "--json",
            ])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let json = String::from_utf8_lossy(&output.stdout);
        out.extend(places_from_list_panes_json(session, &json));
    }
    out
}

pub fn places_from_list_panes_json(session: &str, json: &str) -> Vec<(AgentId, PanePlace)> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            if item.get("is_plugin").and_then(serde_json::Value::as_bool) != Some(false) {
                return None;
            }
            let pane_id = item.get("id").and_then(serde_json::Value::as_u64)? as u32;
            let tab_position = item
                .get("tab_position")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as usize;
            let tab_name = item
                .get("tab_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let pane_title = item
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
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
        })
        .collect()
}

fn list_sessions() -> Vec<String> {
    let Ok(output) = Command::new(zellij_bin())
        .args(["list-sessions", "-n"])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

fn zellij_bin() -> String {
    first_bin(
        &["/opt/homebrew/bin/zellij", "/usr/local/bin/zellij"],
        "zellij",
    )
}

pub fn zellij_ids_from_env_blob(blob: &str) -> Option<(String, u32)> {
    let pane = env_value(blob, "ZELLIJ_PANE_ID")?;
    let session = env_value(blob, "ZELLIJ_SESSION_NAME")?;
    let pane_id = pane.parse().ok()?;
    (!session.is_empty()).then_some((session, pane_id))
}

fn env_value(blob: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let start = blob.find(&needle)? + needle.len();
    let rest = &blob[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let value = &rest[..end];
    (!value.is_empty()).then(|| value.to_string())
}

fn hooks_installed() -> bool {
    let path = home_dir().join(".cursor/hooks.json");
    std::fs::read_to_string(path)
        .map(|text| text.contains("zellij-agent-board-hook"))
        .unwrap_or(false)
}

fn home_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(Into::into)
        .unwrap_or_else(|_| "/tmp".into())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn agent_pids() -> Vec<u32> {
    let mut pids = pgrep("agent");
    pids.extend(pgrep("cursor-agent"));
    pids.sort_unstable();
    pids.dedup();
    pids
}

fn first_bin(candidates: &[&str], fallback: &str) -> String {
    candidates
        .iter()
        .copied()
        .find(|path| Path::new(path).is_file())
        .unwrap_or(fallback)
        .to_string()
}

fn pgrep_bin() -> String {
    first_bin(&["/usr/bin/pgrep", "/bin/pgrep"], "pgrep")
}

fn ps_bin() -> String {
    first_bin(&["/bin/ps", "/usr/bin/ps"], "ps")
}

fn lsof_bin() -> String {
    first_bin(&["/usr/sbin/lsof", "/usr/bin/lsof"], "lsof")
}

fn pgrep(name: &str) -> Vec<u32> {
    let output = Command::new(pgrep_bin()).args(["-x", name]).output();
    let Ok(output) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

fn ps_args(pids: &[u32]) -> std::collections::BTreeMap<u32, String> {
    ps_map(ps_command_args(pids, false))
}

fn ps_env(pids: &[u32]) -> std::collections::BTreeMap<u32, String> {
    ps_map(ps_command_args(pids, true))
}

/// macOS `ps` accepts BSD flags like `eww` only immediately after the verb.
/// `ps -p 1 eww` is `illegal argument: eww` and returns no env.
fn ps_command_args(pids: &[u32], include_env: bool) -> Vec<String> {
    let list = pids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut args = Vec::new();
    if include_env {
        args.push("eww".into());
    }
    args.push("-p".into());
    args.push(list);
    args.extend(["-ww".into(), "-o".into(), "pid=".into(), "-o".into()]);
    args.push(if include_env { "command=" } else { "args=" }.into());
    args
}

fn ps_map(args: Vec<String>) -> std::collections::BTreeMap<u32, String> {
    let mut map = std::collections::BTreeMap::new();
    if args.is_empty() {
        return map;
    }
    let Ok(output) = Command::new(ps_bin()).args(&args).output() else {
        return map;
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        let Some((pid, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if let Ok(pid) = pid.parse::<u32>() {
            map.insert(pid, rest.trim().to_string());
        }
    }
    map
}

fn split_args(args: &str) -> Vec<String> {
    args.split_whitespace().map(str::to_string).collect()
}

fn holding_chat_store(pid: u32) -> bool {
    let Ok(output) = Command::new(lsof_bin())
        .args(["-p", &pid.to_string(), "-Fn"])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.contains("/.cursor/chats/") && line.contains("store.db"))
}

fn read_spool(key: &str) -> Option<String> {
    std::fs::read_to_string(spool_dir().join(key)).ok()
}

fn read_seen(key: &str) -> Option<String> {
    std::fs::read_to_string(seen_dir().join(key)).ok()
}

fn read_started(key: &str) -> Option<String> {
    std::fs::read_to_string(started_dir().join(key)).ok()
}

fn prune_spool(live_keys: &[String]) {
    prune_dir(&spool_dir(), live_keys);
}

fn prune_dir(dir: &Path, live_keys: &[String]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !live_keys.iter().any(|key| key == name) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        places_from_list_panes_json, ps_command_args, scan_places_for, zellij_ids_from_env_blob,
    };

    #[test]
    fn macos_ps_eww_comes_before_dash_p() {
        let args = ps_command_args(&[2880, 2957], true);
        assert_eq!(
            args,
            vec![
                "eww",
                "-p",
                "2880,2957",
                "-ww",
                "-o",
                "pid=",
                "-o",
                "command="
            ]
        );
        let args = ps_command_args(&[2880], false);
        assert_eq!(args, vec!["-p", "2880", "-ww", "-o", "pid=", "-o", "args="]);
    }

    #[test]
    fn reads_zellij_ids_from_a_ps_eww_blob() {
        let blob = "/Users/ww/.local/bin/agent --workspace /tmp/w ZELLIJ_PANE_ID=3 ZELLIJ_SESSION_NAME=ww HOME=/Users/ww";
        assert_eq!(zellij_ids_from_env_blob(blob), Some(("ww".into(), 3)));
    }

    #[test]
    fn ignores_a_blob_without_zellij() {
        assert_eq!(zellij_ids_from_env_blob("HOME=/tmp PATH=/bin"), None);
    }

    #[test]
    fn live_scan_finds_zellij_agents_when_pgrep_does() {
        let has_agent = std::process::Command::new(super::pgrep_bin())
            .args(["-x", "agent"])
            .output()
            .ok()
            .is_some_and(|output| output.status.success() && !output.stdout.is_empty());
        if !has_agent {
            return;
        }
        let text = super::scan_host_text();
        assert!(text.lines().any(|line| line.starts_with("SCAN ")), "{text}");
    }

    #[test]
    fn empty_session_list_does_not_query_zellij() {
        assert!(scan_places_for(&[]).is_empty());
    }

    #[test]
    fn list_panes_json_keeps_mysql_syncer_agent_title() {
        let json = r#"[
            {"id":139,"is_plugin":true,"title":"zellij:tab-bar","tab_position":0,"tab_name":"master"},
            {"id":2,"is_plugin":false,"title":"refactor/use-yotta","tab_position":1,"tab_name":"refactor/use-yotta"},
            {"id":3,"is_plugin":false,"title":"zsh","tab_position":1,"tab_name":"refactor/use-yotta"}
        ]"#;
        let places = places_from_list_panes_json("mysql_syncer", json);
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].0.session, "mysql_syncer");
        assert_eq!(places[0].0.pane_id, 2);
        assert_eq!(places[0].1.tab_name, "refactor/use-yotta");
        assert_eq!(places[0].1.pane_title, "refactor/use-yotta");
    }

    #[test]
    fn prefers_absolute_host_bins_when_present() {
        if Path::new("/usr/bin/pgrep").is_file() {
            assert_eq!(super::pgrep_bin(), "/usr/bin/pgrep");
        }
        if Path::new("/bin/ps").is_file() {
            assert_eq!(super::ps_bin(), "/bin/ps");
        }
        if Path::new("/usr/sbin/lsof").is_file() {
            assert_eq!(super::lsof_bin(), "/usr/sbin/lsof");
        }
    }
}
