//! Local, opt-out usage telemetry. Host-only.
//!
//! The collection method is deliberately generic: `record(event, fields)`
//! appends one timestamped JSON object per line to an append-only log. Reading
//! it back, `summarize` folds every line into counts, so a brand new event
//! needs no reader change. Nothing ever leaves this machine, and setting
//! `ZELLIJ_AGENT_BOARD_NO_STATS` turns the whole thing off.
//!
//! # Schema contract
//!
//! Each line is a self-describing JSON object. Adding new events or new fields
//! is always backward compatible — the reader counts unknown events by name and
//! skips unknown / missing / wrong-typed fields. The only keys with a fixed
//! meaning are:
//!
//! - `v`      — schema version ([`SCHEMA_VERSION`]); bump on breaking changes.
//! - `ts`     — event time, epoch **seconds** (integer).
//! - `event`  — event name; treat as a stable id (renaming splits old/new counts).
//! - `secs`   — on `close`, session duration in **seconds**.
//! - `agents` / `max_agents` — agent counts; both feed the peak.
//!
//! Don't change the name, type, or unit of those without teaching `summarize`
//! to read both the old and new form (see how `max_agents` already accepts the
//! older `agents` key).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

/// On-disk event schema version. Bump only on breaking changes; adding events
/// or fields does not need a bump.
pub const SCHEMA_VERSION: u64 = 1;

/// Set (to anything) to disable usage collection entirely.
pub const DISABLE_ENV: &str = "ZELLIJ_AGENT_BOARD_NO_STATS";

/// Override the log location (mainly for tests).
pub const PATH_ENV: &str = "ZELLIJ_AGENT_BOARD_STATS";

pub fn enabled() -> bool {
    std::env::var_os(DISABLE_ENV).is_none()
}

pub fn stats_path() -> PathBuf {
    if let Some(explicit) = std::env::var_os(PATH_ENV) {
        if !explicit.is_empty() {
            return PathBuf::from(explicit);
        }
    }
    data_dir().join("usage.jsonl")
}

fn data_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("zellij-agent-board");
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".local/share/zellij-agent-board")
}

/// Append one event. `fields` are extra JSON columns merged onto `ts`/`event`.
/// Best effort: any IO error is swallowed so telemetry never breaks the board.
pub fn record(event: &str, fields: &[(&str, Value)]) {
    if !enabled() {
        return;
    }
    let line = event_line(now_epoch(), event, fields);
    append(&stats_path(), &line);
}

fn append(path: &Path, line: &str) {
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

pub fn read_log() -> String {
    std::fs::read_to_string(stats_path()).unwrap_or_default()
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Build one JSON line. Pure, so the on-disk shape is unit-testable.
pub fn event_line(now: u64, event: &str, fields: &[(&str, Value)]) -> String {
    let mut map = Map::new();
    map.insert("v".into(), Value::from(SCHEMA_VERSION));
    map.insert("ts".into(), Value::from(now));
    map.insert("event".into(), Value::from(event));
    for (key, value) in fields {
        map.insert((*key).to_string(), value.clone());
    }
    Value::Object(map).to_string()
}

/// Folded view of the whole log. New event kinds land in `events` for free.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub total: u64,
    pub events: BTreeMap<String, u64>,
    pub first_ts: Option<u64>,
    pub last_ts: Option<u64>,
    /// Sum of `close.secs` — total time the board stayed open.
    pub open_secs: u64,
    /// Peak agents seen in any single event.
    pub max_agents: u64,
}

impl Summary {
    pub fn count(&self, event: &str) -> u64 {
        self.events.get(event).copied().unwrap_or(0)
    }
}

pub fn summarize(log: &str) -> Summary {
    let mut summary = Summary::default();
    for line in log.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(event) = map.get("event").and_then(Value::as_str) else {
            continue;
        };
        summary.total += 1;
        *summary.events.entry(event.to_string()).or_default() += 1;
        if let Some(ts) = map.get("ts").and_then(Value::as_u64) {
            summary.first_ts = Some(summary.first_ts.map_or(ts, |seen| seen.min(ts)));
            summary.last_ts = Some(summary.last_ts.map_or(ts, |seen| seen.max(ts)));
        }
        if event == "close" {
            if let Some(secs) = map.get("secs").and_then(Value::as_u64) {
                summary.open_secs = summary.open_secs.saturating_add(secs);
            }
        }
        for key in ["agents", "max_agents"] {
            if let Some(agents) = map.get(key).and_then(Value::as_u64) {
                summary.max_agents = summary.max_agents.max(agents);
            }
        }
    }
    summary
}

pub fn format_summary(summary: &Summary) -> String {
    if summary.total == 0 {
        return "no usage recorded yet\n".to_string();
    }
    let mut out = String::new();
    out.push_str("zellij-agent-board usage\n");
    if let (Some(first), Some(last)) = (summary.first_ts, summary.last_ts) {
        out.push_str(&format!("  span      {} .. {} (epoch s)\n", first, last));
    }
    out.push_str(&format!("  events    {}\n", summary.total));
    out.push_str(&format!("  opens     {}\n", summary.count("open")));
    out.push_str(&format!("  jumps     {}\n", summary.count("jump")));
    out.push_str(&format!(
        "  open time {}\n",
        format_duration(summary.open_secs)
    ));
    out.push_str(&format!("  peak agents {}\n", summary.max_agents));
    out.push_str("  by event\n");
    for (event, count) in &summary.events {
        out.push_str(&format!("    {event:<12} {count}\n"));
    }
    out
}

fn format_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::{event_line, format_summary, summarize, Summary};
    use serde_json::json;

    #[test]
    fn event_line_carries_ts_event_and_fields() {
        let line = event_line(
            1_700_000_000,
            "jump",
            &[("from", json!("ww")), ("to", json!("lp"))],
        );
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["v"], super::SCHEMA_VERSION);
        assert_eq!(value["ts"], 1_700_000_000);
        assert_eq!(value["event"], "jump");
        assert_eq!(value["from"], "ww");
        assert_eq!(value["to"], "lp");
    }

    #[test]
    fn summarize_folds_counts_span_open_time_and_peak() {
        let log = "\
{\"ts\":100,\"event\":\"open\",\"agents\":3}
{\"ts\":140,\"event\":\"jump\",\"from\":\"ww\",\"to\":\"lp\"}
{\"ts\":150,\"event\":\"jump\",\"from\":\"ww\",\"to\":\"lp\"}
{\"ts\":200,\"event\":\"close\",\"secs\":100,\"max_agents\":5}
";
        let summary = summarize(log);
        assert_eq!(summary.total, 4);
        assert_eq!(summary.count("jump"), 2);
        assert_eq!(summary.count("open"), 1);
        assert_eq!(summary.first_ts, Some(100));
        assert_eq!(summary.last_ts, Some(200));
        assert_eq!(summary.open_secs, 100);
        assert_eq!(summary.max_agents, 5);
    }

    #[test]
    fn summarize_ignores_blank_and_malformed_lines() {
        let log = "\n{not json}\n{\"ts\":1,\"event\":\"open\"}\n{\"ts\":2}\n";
        let summary = summarize(log);
        assert_eq!(summary.total, 1);
        assert_eq!(summary.count("open"), 1);
    }

    #[test]
    fn format_summary_reports_empty_state() {
        assert_eq!(
            format_summary(&Summary::default()),
            "no usage recorded yet\n"
        );
    }
}
