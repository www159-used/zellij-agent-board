use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloatSize {
    pub x: String,
    pub y: String,
    pub width: String,
    pub height: String,
}

impl Default for FloatSize {
    fn default() -> Self {
        Self {
            x: String::from("3%"),
            y: String::from("3%"),
            width: String::from("94%"),
            height: String::from("94%"),
        }
    }
}

pub fn float_size_from_config(config: &BTreeMap<String, String>) -> FloatSize {
    let mut size = FloatSize::default();
    if let Some(width) = parse_extent(config.get("width").map(String::as_str)) {
        size.width = width;
    }
    if let Some(height) = parse_extent(config.get("height").map(String::as_str)) {
        size.height = height;
    }
    if let Some(x) = parse_origin(config.get("x").map(String::as_str)) {
        size.x = x;
    }
    if let Some(y) = parse_origin(config.get("y").map(String::as_str)) {
        size.y = y;
    }
    size
}

fn parse_percent(raw: Option<&str>) -> Option<u16> {
    let raw = raw?.trim();
    let number = raw.strip_suffix('%')?.trim();
    number.parse().ok()
}

fn parse_extent(raw: Option<&str>) -> Option<String> {
    parse_token(raw, 1)
}

fn parse_origin(raw: Option<&str>) -> Option<String> {
    parse_token(raw, 0)
}

fn parse_token(raw: Option<&str>, minimum: u16) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(percent) = parse_percent(Some(raw)) {
        if percent >= minimum && percent <= 100 {
            return Some(format!("{percent}%"));
        }
        return None;
    }
    let cells: u16 = raw.parse().ok()?;
    (cells >= minimum).then(|| cells.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn defaults_to_a_large_centered_pane() {
        assert_eq!(
            float_size_from_config(&BTreeMap::new()),
            FloatSize::default()
        );
    }

    #[test]
    fn invalid_values_keep_the_default() {
        assert_eq!(
            float_size_from_config(&config(&[("width", "0%"), ("x", "-1")])),
            FloatSize::default()
        );
    }
}
