//! Small helpers shared by the Zellij environment.
use std::env;
use std::path::PathBuf;

use serde_json::Value;

pub(crate) fn resolve_zellij() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("ZAB_E2E_ZELLIJ") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "ZAB_E2E_ZELLIJ is not executable: {}",
            path.display()
        ));
    }
    which("zellij").ok_or_else(|| "zellij is required".to_owned())
}

pub(crate) fn which(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(name);
            candidate.is_file().then_some(candidate)
        })
    })
}

pub(crate) fn workspace_target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"))
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) fn parse_pane_id(text: &str) -> Option<i64> {
    for token in text.split_whitespace() {
        if let Some(rest) = token.strip_prefix("terminal_") {
            return rest.parse().ok();
        }
        if token.chars().all(|ch| ch.is_ascii_digit()) {
            return token.parse().ok();
        }
    }
    None
}

pub(crate) fn pane_blob(pane: &Value, keys: &[&str]) -> String {
    keys.iter()
        .filter_map(|key| pane.get(*key).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else {
            out.push(ch);
        }
    }
    out
}

pub(crate) fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
