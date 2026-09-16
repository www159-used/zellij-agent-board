//! Local usage facts. No network transport or persistent installation identity.
//! v2 events share a visit ID, sequence and monotonic elapsed time.
//! Raw input text and real session/pane identifiers are never serialized.

use crate::{AgentId, Board, Key};
use serde_json::json;
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

/// On-disk event schema version. Bump only on breaking changes; adding events
/// or fields does not need a bump.
pub const SCHEMA_VERSION: u64 = 2;

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
    for (key, value) in fields {
        map.insert((*key).to_string(), value.clone());
    }
    map.insert("v".into(), Value::from(SCHEMA_VERSION));
    map.insert("ts".into(), Value::from(now));
    map.insert("event".into(), Value::from(event));
    Value::Object(map).to_string()
}

/// A visit is one TUI lifetime. Identity maps stay in memory and reset on reopen.
pub struct Visit {
    path: Option<PathBuf>,
    id: String,
    start: Instant,
    seq: u64,
    agents: BTreeMap<AgentId, usize>,
    sessions: BTreeMap<String, usize>,
    last_snapshot: Option<Value>,
}

impl Default for Visit {
    fn default() -> Self {
        Self::new(enabled().then(stats_path))
    }
}

impl Visit {
    /// Construct a visit with an explicit sink; None disables recording.
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut bytes = [0u8; 16];
        let available = getrandom::fill(&mut bytes).is_ok();
        let id = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        // Entropy failure disables this visit rather than emitting ambiguous IDs.
        let path = path.filter(|_| available);
        Self {
            path,
            id,
            start: Instant::now(),
            seq: 0,
            agents: BTreeMap::new(),
            sessions: BTreeMap::new(),
            last_snapshot: None,
        }
    }

    pub fn record(&mut self, event: &str, fields: &[(&str, Value)]) -> u64 {
        if event == "open" && self.seq == 0 {
            self.start = Instant::now();
        }
        self.seq += 1;
        if let Some(path) = &self.path {
            let mut fields = fields.to_vec();
            fields.extend([
                ("visit_id", json!(self.id)),
                ("seq", json!(self.seq)),
                ("elapsed_ms", json!(self.start.elapsed().as_millis() as u64)),
                ("app_version", json!(env!("CARGO_PKG_VERSION"))),
            ]);
            append(path, &event_line(now_epoch(), event, &fields));
        }
        self.seq
    }

    fn agent_id(&mut self, id: &AgentId) -> usize {
        let next = self.agents.len();
        *self.agents.entry(id.clone()).or_insert(next)
    }

    fn session_id(&mut self, session: &str) -> usize {
        let next = self.sessions.len();
        *self.sessions.entry(session.to_string()).or_insert(next)
    }

    pub fn snapshot(&mut self, board: &Board, width: u16, height: u16) {
        if self.path.is_none() {
            return;
        }
        let rows: Vec<Value> = board
            .agents
            .iter()
            .map(|agent| {
                let id = self.agent_id(&agent.id);
                let session = self.session_id(&agent.id.session);
                json!({"id": id, "session_id": session, "tool": agent.tool,
                "status": agent.status.label(), "visited": agent.visited})
            })
            .collect();
        let state = json!({"rows": rows, "selected": board.selected,
            "width": width, "height": height, "searching": board.is_searching(),
            "hinting": board.is_hinting(), "picking": board.is_picking(),
            "help": board.help_visible, "search_length": board.search_query().chars().count(),
            "picker_length": board.picker_query().chars().count(),
            "picker_focus": format!("{:?}", board.picker_focus()),
            "picker_position": board.picker_position(), "picker_matches": board.picker_matches(),
            "hint_length": board.hint_query().chars().count()});
        if self.last_snapshot.as_ref() != Some(&state) {
            self.record(
                "state",
                &[
                    ("state", state.clone()),
                    ("agents", json!(board.agents.len())),
                ],
            );
            self.last_snapshot = Some(state);
        }
    }

    pub fn key(&mut self, key: Key) {
        let operation = match key {
            Key::Input(_) => "Input".to_string(),
            Key::Digit(_) => "Digit".to_string(),
            other => format!("{other:?}"),
        };
        self.record(
            "input",
            &[
                ("source", json!("keyboard")),
                ("operation", json!(operation)),
            ],
        );
    }

    pub fn jump(&mut self, session: &str, pane_id: u32) -> u64 {
        let target = self.agent_id(&AgentId {
            session: session.into(),
            pane_id,
        });
        self.record("jump", &[("target", json!(target))])
    }

    pub fn close(&mut self, reason: &str) {
        self.record(
            "close",
            &[
                ("reason", json!(reason)),
                ("secs", json!(self.start.elapsed().as_secs())),
            ],
        );
    }
}

/// Folded view of the whole log. New event kinds land in `events` for free.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub total: u64,
    pub visits: u64,
    pub incomplete_visits: u64,
    pub inputs: BTreeMap<String, u64>,
    pub mode_entries: BTreeMap<String, u64>,
    pub dispatch_results: BTreeMap<String, u64>,
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
    let mut opened = BTreeSet::new();
    let mut closed = BTreeSet::new();
    let mut facts = BTreeMap::new();
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
        if map.get("v").and_then(Value::as_u64) == Some(2) {
            if let (Some(visit), Some(seq)) = (
                map.get("visit_id").and_then(Value::as_str),
                map.get("seq").and_then(Value::as_u64),
            ) {
                match facts.entry((visit.to_string(), seq)) {
                    Entry::Occupied(_) => continue,
                    Entry::Vacant(entry) => {
                        entry.insert(map.clone());
                    }
                }
            }
        }
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
    // Sort by visit/sequence, independent of append order across processes.
    let mut modes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ((visit, _), map) in facts {
        match map.get("event").and_then(Value::as_str).unwrap_or("") {
            "open" => {
                opened.insert(visit);
            }
            "close" => {
                closed.insert(visit);
            }
            "input" => {
                if let (Some(source), Some(operation)) = (
                    map.get("source").and_then(Value::as_str),
                    map.get("operation").and_then(Value::as_str),
                ) {
                    *summary
                        .inputs
                        .entry(format!("{source}/{operation}"))
                        .or_default() += 1;
                }
            }
            "state" => {
                if let Some(state) = map.get("state").and_then(Value::as_object) {
                    let active: BTreeSet<String> = ["searching", "hinting", "picking", "help"]
                        .into_iter()
                        .filter(|key| state.get(*key).and_then(Value::as_bool) == Some(true))
                        .map(String::from)
                        .collect();
                    let previous = modes.entry(visit).or_default();
                    for mode in active.difference(previous) {
                        *summary.mode_entries.entry(mode.clone()).or_default() += 1;
                    }
                    *previous = active;
                }
            }
            "jump_result" => {
                if let Some(outcome) = map.get("outcome").and_then(Value::as_str) {
                    *summary.dispatch_results.entry(outcome.into()).or_default() += 1;
                }
            }
            _ => {}
        }
    }
    summary.visits = opened.len() as u64;
    summary.incomplete_visits = opened.difference(&closed).count() as u64;
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
    out.push_str(&format!(
        "  v2 visits {} ({} without close)\n",
        summary.visits, summary.incomplete_visits
    ));
    for (label, counts) in [
        ("input operations", &summary.inputs),
        ("mode entries (derived from state)", &summary.mode_entries),
        (
            "pipe outcomes (not focus confirmation)",
            &summary.dispatch_results,
        ),
    ] {
        if !counts.is_empty() {
            out.push_str(&format!("  {label}\n"));
            for (name, count) in counts {
                let display = if label == "mode entries (derived from state)" && name == "hinting" {
                    "Flash"
                } else {
                    name.as_str()
                };
                out.push_str(&format!("    {display:<24} {count}\n"));
            }
        }
    }
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
    fn opening_resets_startup_time() {
        let mut visit = super::Visit::new(None);
        let before = std::time::Instant::now();
        visit.start = before - std::time::Duration::from_secs(60);
        visit.record("open", &[]);
        assert!(visit.start >= before);
        let opened = visit.start;
        visit.key(crate::Key::Down);
        assert_eq!(visit.start, opened);
    }

    #[test]
    fn explicit_v1_and_v2_logs_keep_separate_visit_metrics() {
        let old = include_str!("../tests/fixtures/usage-v1.jsonl");
        let legacy = summarize(old);
        assert_eq!(legacy.count("open"), 1);
        assert_eq!(legacy.count("jump"), 2);
        assert_eq!(legacy.open_secs, 100);
        assert_eq!(legacy.max_agents, 5);
        assert_eq!(legacy.visits, 0);
        let current = concat!(
            "{\"v\":2,\"event\":\"open\",\"visit_id\":\"new\",\"seq\":1}\n",
            "{\"v\":2,\"event\":\"state\",\"visit_id\":\"new\",\"seq\":2,\"state\":{\"hinting\":true}}\n",
            "{\"v\":2,\"event\":\"close\",\"visit_id\":\"new\",\"seq\":3,\"secs\":2}\n"
        );
        let mixed = summarize(&format!("{old}{current}"));
        assert_eq!(mixed.count("open"), 2);
        assert_eq!(mixed.open_secs, 102);
        assert_eq!(mixed.visits, 1);
        assert_eq!(mixed.incomplete_visits, 0);
        assert_eq!(mixed.mode_entries["hinting"], 1);
        let report = format_summary(&mixed);
        assert!(report.contains("Flash"));
        assert!(!report.contains("hinting"));
    }

    #[test]
    fn visit_records_facts_without_input_or_session_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.jsonl");
        let mut visit = super::Visit::new(Some(path.clone()));
        let mut board = crate::Board::default();
        visit.record("open", &[]);
        visit.snapshot(&board, 80, 24);
        visit.snapshot(&board, 80, 24);
        visit.key(crate::Key::StartSearch);
        board.decide(crate::Key::StartSearch);
        visit.snapshot(&board, 80, 24);
        visit.key(crate::Key::Input('秘'));
        visit.jump("private-session", 123456);
        visit.close("dismiss");
        let log = std::fs::read_to_string(path).unwrap();
        assert!(!log.contains("private-session"));
        assert!(!log.contains("123456"));
        assert!(!log.contains('秘'));
        let rows: Vec<serde_json::Value> = log
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(row["seq"], index + 1);
            assert_eq!(row["visit_id"], rows[0]["visit_id"]);
        }
        let summary = summarize(&log);
        assert_eq!(summary.count("state"), 2);
        assert_eq!(summary.mode_entries["searching"], 1);
        assert_eq!(summary.inputs["keyboard/Input"], 1);
        assert_eq!(summary.incomplete_visits, 0);
        let reversed = log.lines().rev().collect::<Vec<_>>().join("\n");
        assert_eq!(summary, summarize(&format!("{reversed}\n{log}")));
    }

    #[test]
    fn incomplete_and_failed_visits_are_not_successes() {
        let log = concat!(
            "{\"v\":2,\"event\":\"open\",\"visit_id\":\"a\",\"seq\":1}\n",
            "{\"v\":2,\"event\":\"jump_result\",\"visit_id\":\"a\",\"seq\":3,\"request_seq\":2,\"outcome\":\"exit_error\"}\n",
            "{\"v\":2,\"event\":\"open\",\"visit_id\":\"b\",\"seq\":1}\n",
            "{\"v\":2,\"event\":\"close\",\"visit_id\":\"b\",\"seq\":2,\"secs\":5}\n"
        );
        let summary = summarize(log);
        assert_eq!(summary.visits, 2);
        assert_eq!(summary.incomplete_visits, 1);
        assert_eq!(summary.open_secs, 5);
        assert_eq!(summary.dispatch_results["exit_error"], 1);
        assert!(!summary.dispatch_results.contains_key("ok"));
    }

    #[test]
    fn disabled_visit_does_not_create_log() {
        let mut visit = super::Visit::new(None);
        visit.record("open", &[]);
        visit.snapshot(&crate::Board::default(), 80, 24);
        assert!(visit.last_snapshot.is_none());
    }

    #[test]
    fn reserved_event_fields_cannot_be_overwritten() {
        let value: serde_json::Value = serde_json::from_str(&event_line(
            10,
            "open",
            &[
                ("v", json!(99)),
                ("event", json!("wrong")),
                ("ts", json!(0)),
            ],
        ))
        .unwrap();
        assert_eq!(value["v"], 2);
        assert_eq!(value["event"], "open");
        assert_eq!(value["ts"], 10);
    }

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
