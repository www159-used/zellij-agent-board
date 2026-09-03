use std::collections::BTreeMap;
use std::sync::Mutex;

use ratatui::style::Color;

pub const PACKED_THEME_CSS: &str = include_str!("theme.css");

static CURRENT: Mutex<Option<Theme>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub focus: Color,
    pub focus_fill: Color,
    pub session: Color,
    pub task: Color,
    pub pin_mark: Color,
    pub pin_border: Color,
    pub match_fg: Color,
    pub match_border: Color,
    pub mask: Color,
    pub mask_fg: Color,
    pub card_border: Color,
    pub separator: Color,
    pub tip_fg: Color,
    pub tip_bg: Color,
    pub tip_typed: Color,
    pub tool_codebuddy: Color,
}

pub fn theme() -> Theme {
    let mut current = CURRENT.lock().expect("theme lock");
    *current.get_or_insert_with(|| Theme::from_css(PACKED_THEME_CSS))
}

impl Theme {
    fn from_css(css: &str) -> Self {
        Self::try_from_vars(&parse_css_vars(css))
            .unwrap_or_else(|| panic!("packed theme.css is incomplete"))
    }

    fn try_from_vars(vars: &BTreeMap<String, String>) -> Option<Self> {
        Some(Self {
            focus: try_color(vars, "focus")?,
            focus_fill: try_color(vars, "focus-fill")?,
            session: try_color(vars, "session")?,
            task: try_color(vars, "task")?,
            pin_mark: try_color(vars, "pin-mark")?,
            pin_border: try_color(vars, "pin-border")?,
            match_fg: try_color(vars, "match")?,
            match_border: try_color(vars, "match-border")?,
            mask: try_color(vars, "mask")?,
            mask_fg: try_color(vars, "mask-fg")?,
            card_border: try_color(vars, "card-border")?,
            separator: try_color(vars, "separator")?,
            tip_fg: try_color(vars, "tip-fg")?,
            tip_bg: try_color(vars, "tip-bg")?,
            tip_typed: try_color(vars, "tip-typed")?,
            tool_codebuddy: try_color(vars, "tool-codebuddy")?,
        })
    }
}

fn try_color(vars: &BTreeMap<String, String>, name: &str) -> Option<Color> {
    resolve_color(vars, name, 0)
}

fn resolve_color(vars: &BTreeMap<String, String>, name: &str, depth: u8) -> Option<Color> {
    let raw = vars.get(name)?;
    if let Some(other) = raw
        .strip_prefix("var(--")
        .and_then(|value| value.strip_suffix(')'))
    {
        if depth > 8 {
            return None;
        }
        return resolve_color(vars, other, depth + 1);
    }
    parse_hex(raw)
}

fn parse_css_vars(css: &str) -> BTreeMap<String, String> {
    let stripped = strip_block_comments(css);
    let mut vars = BTreeMap::new();
    let mut rest = stripped.as_str();
    while let Some(start) = rest.find("--") {
        rest = &rest[start + 2..];
        let Some((name, after)) = rest.split_once(':') else {
            break;
        };
        let name = name.trim();
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            continue;
        }
        let end = after.find([';', '}']).unwrap_or(after.len());
        let value = after[..end].trim();
        if !value.is_empty() {
            vars.insert(name.to_owned(), value.to_owned());
        }
        rest = if end < after.len() {
            &after[end + 1..]
        } else {
            ""
        };
    }
    vars
}

fn strip_block_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some((before, after)) = rest.split_once("/*") {
        out.push_str(before);
        match after.split_once("*/") {
            Some((_, next)) => rest = next,
            None => break,
        }
    }
    out.push_str(rest);
    out
}

pub(crate) fn color_from_hex(value: &str) -> Option<Color> {
    parse_hex(value)
}

fn parse_hex(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    let (red, green, blue) = match hex.len() {
        3 => {
            let bytes = hex.as_bytes();
            (
                expand_nibble(bytes[0])?,
                expand_nibble(bytes[1])?,
                expand_nibble(bytes[2])?,
            )
        }
        6 => {
            let n = u32::from_str_radix(hex, 16).ok()?;
            (
                ((n >> 16) & 0xff) as u8,
                ((n >> 8) & 0xff) as u8,
                (n & 0xff) as u8,
            )
        }
        _ => return None,
    };
    Some(Color::Rgb(red, green, blue))
}

fn expand_nibble(byte: u8) -> Option<u8> {
    let nibble = match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => return None,
    };
    Some(nibble * 16 + nibble)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_theme_matches_overview() {
        let theme = Theme::from_css(PACKED_THEME_CSS);
        assert_eq!(theme.session, Color::Rgb(105, 208, 196));
        assert_eq!(theme.task, Color::Rgb(0xe4, 0xd4, 0xff));
        assert_eq!(theme.focus_fill, Color::Rgb(0x3a, 0x2f, 0x52));
        assert_eq!(theme.separator, theme.card_border);
        assert_eq!(theme.tip_typed, theme.focus);
        assert_eq!(theme.tool_codebuddy, Color::Rgb(0x86, 0xb6, 0xf2));
    }
}
