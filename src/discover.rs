//! Process scan seam. The WASM adapter will run `ps` / spool read; this crate
//! only parses host text into rows. Existence comes from the scan, never the hook.

use crate::agent::AgentId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub id: AgentId,
    pub tool: String,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookNotice {
    pub id: AgentId,
    pub event: String,
    pub detail: String,
    /// Optional host wall clock from the hook script, e.g. `08-24 15:21`.
    pub at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostLine {
    Scan(Found),
    Hook(HookNotice),
    Meta { hooks_installed: bool },
}

/// One `SCAN session pane tool [argv…]` line from the host script.
pub fn parse_scan_line(line: &str) -> Option<Found> {
    match parse_host_line(line) {
        Some(HostLine::Scan(found)) => Some(found),
        _ => None,
    }
}

pub fn parse_host_line(line: &str) -> Option<HostLine> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut parts = line.split_whitespace();
    match parts.next()? {
        "SCAN" => {
            let session = parts.next()?.to_string();
            let pane_id = parts.next()?.parse().ok()?;
            let tool = parts.next()?.to_string();
            let argv = parts.map(str::to_string).collect();
            Some(HostLine::Scan(Found {
                id: AgentId { session, pane_id },
                tool,
                argv,
            }))
        }
        "HOOK" => {
            let session = parts.next()?.to_string();
            let pane_id = parts.next()?.parse().ok()?;
            let event = parts.next()?.to_string();
            let mut at = None;
            let mut detail_parts = Vec::new();
            for part in parts {
                if at.is_none() {
                    if let Some(stamp) = part.strip_prefix('@') {
                        at = Some(stamp.replace('T', " "));
                        continue;
                    }
                }
                detail_parts.push(part.to_string());
            }
            Some(HostLine::Hook(HookNotice {
                id: AgentId { session, pane_id },
                event,
                detail: detail_parts.join(" "),
                at,
            }))
        }
        "META" => {
            let mut hooks_installed = None;
            for token in parts {
                if let Some(value) = token.strip_prefix("hooks=") {
                    hooks_installed = Some(value == "1" || value == "true");
                }
            }
            Some(HostLine::Meta {
                hooks_installed: hooks_installed?,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_host_line, parse_scan_line, Found, HostLine};
    use crate::agent::AgentId;

    #[test]
    fn reads_a_scan_line_with_workspace_argv() {
        let found = parse_scan_line(
            "SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/w --continue",
        )
        .expect("scan line");
        assert_eq!(
            found,
            Found {
                id: AgentId {
                    session: "ww".into(),
                    pane_id: 3,
                },
                tool: "agent".into(),
                argv: vec![
                    "/Users/ww/.local/bin/agent".into(),
                    "--workspace".into(),
                    "/tmp/w".into(),
                    "--continue".into(),
                ],
            }
        );
    }

    #[test]
    fn ignores_noise_and_reads_hook_meta() {
        assert_eq!(parse_scan_line("agent ls"), None);
        assert_eq!(
            parse_host_line("HOOK lp 8 beforeSubmitPrompt"),
            Some(HostLine::Hook(super::HookNotice {
                id: AgentId {
                    session: "lp".into(),
                    pane_id: 8,
                },
                event: "beforeSubmitPrompt".into(),
                detail: String::new(),
                at: None,
            }))
        );
        assert_eq!(
            parse_host_line("HOOK ww 3 preToolUse Shell cargo test --lib"),
            Some(HostLine::Hook(super::HookNotice {
                id: AgentId {
                    session: "ww".into(),
                    pane_id: 3,
                },
                event: "preToolUse".into(),
                detail: "Shell cargo test --lib".into(),
                at: None,
            }))
        );
        assert_eq!(
            parse_host_line("HOOK ww 3 stop @08-24T15:21"),
            Some(HostLine::Hook(super::HookNotice {
                id: AgentId {
                    session: "ww".into(),
                    pane_id: 3,
                },
                event: "stop".into(),
                detail: String::new(),
                at: Some("08-24 15:21".into()),
            }))
        );
        assert_eq!(
            parse_host_line("META hooks=0"),
            Some(HostLine::Meta {
                hooks_installed: false
            })
        );
    }
}
