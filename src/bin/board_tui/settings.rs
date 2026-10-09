//! Persistent preferences and the native settings form share one set of choices.
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Paragraph, Widget},
};
use serde::{Deserialize, Serialize};
use zellij_agent_board::i18n::{I18n, Locale, Message};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleAction {
    #[default]
    Screensaver,
    Monitor,
    Close,
    Disabled,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaverStyle {
    #[default]
    Blank,
    Clock,
}

/// Selectable values per row, paired with the label the form paints, and kept in
/// the order the form shows them. `choices`, `selected` and `set` all read these
/// tables, so the painted order and the stored value cannot drift apart.
const IDLE_ACTIONS: [(Message, IdleAction); 4] = [
    (Message::ChoiceScreensaver, IdleAction::Screensaver),
    (Message::ChoiceMonitor, IdleAction::Monitor),
    (Message::ChoiceClose, IdleAction::Close),
    (Message::ChoiceDisabled, IdleAction::Disabled),
];
const IDLE_SECONDS: [u64; 3] = [30, 60, 300];
const SAVER_STYLES: [(Message, SaverStyle); 2] = [
    (Message::ChoiceBlank, SaverStyle::Blank),
    (Message::ChoiceClock, SaverStyle::Clock),
];

/// One row per selector, then the Preview and Back buttons.
const ROWS: usize = 6;
const SELECTORS: usize = 4;

/// Selector rows in paint order. `choices`, `selected`, `set` and `render` all
/// dispatch on this enum, so a new setting is a compile error until every site
/// is updated instead of a row that silently paints but never stores.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    IdleAction,
    IdleTime,
    SaverStyle,
    Language,
}
impl Field {
    const ALL: [Field; SELECTORS] = [
        Field::IdleAction,
        Field::IdleTime,
        Field::SaverStyle,
        Field::Language,
    ];
    fn label(self) -> Message {
        match self {
            Field::IdleAction => Message::SettingsIdleAction,
            Field::IdleTime => Message::SettingsIdleTime,
            Field::SaverStyle => Message::SettingsSaverStyle,
            Field::Language => Message::SettingsLanguage,
        }
    }
}

fn label_width(inner: Rect) -> u16 {
    18.min(inner.width / 3)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub idle_action: IdleAction,
    pub idle_seconds: u64,
    pub saver_style: SaverStyle,
    pub language: Locale,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            idle_action: IdleAction::Screensaver,
            idle_seconds: 30,
            saver_style: SaverStyle::Blank,
            language: Locale::English,
        }
    }
}

pub fn config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ZAB_SETTINGS_PATH") {
        return path.into();
    }
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    root.map(|path| path.join("zellij-agent-board/settings.json"))
        .unwrap_or_else(|| zellij_agent_board::runtime_dir().join("settings.json"))
}

pub enum Outcome {
    Ignored,
    Changed,
    Preview,
    Close,
}

pub struct Form {
    pub value: Preferences,
    path: PathBuf,
    focus: usize,
    hover: Option<(usize, Option<usize>)>,
    notice: Option<(Message, String)>,
}
impl Form {
    pub fn load(path: PathBuf) -> Self {
        let loaded = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Preferences>(&bytes)
                .map_err(|err| err.to_string())
                .and_then(|value| {
                    if (1..=86400).contains(&value.idle_seconds) {
                        Ok(value)
                    } else {
                        Err(I18n::new(value.language).text(Message::InvalidIdleSeconds))
                    }
                }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Preferences::default()),
            Err(err) => Err(err.to_string()),
        };
        let (value, notice) = match loaded {
            Ok(value) => (value, None),
            Err(err) => (
                Preferences::default(),
                Some((Message::SettingsLoadError, err)),
            ),
        };
        Self {
            value,
            path,
            focus: 0,
            hover: None,
            notice,
        }
    }

    pub fn i18n(&self) -> I18n {
        I18n::new(self.value.language)
    }

    fn choices(&self, field: Field) -> Vec<String> {
        let lang = self.i18n();
        match field {
            Field::IdleAction => IDLE_ACTIONS.map(|(label, _)| lang.text(label)).to_vec(),
            Field::IdleTime => {
                let mut choices = vec![
                    lang.format(Message::Seconds, &[("seconds", "30")]),
                    lang.format(Message::Minutes, &[("minutes", "1")]),
                    lang.format(Message::Minutes, &[("minutes", "5")]),
                ];
                if !IDLE_SECONDS.contains(&self.value.idle_seconds) {
                    choices.push(lang.format(
                        Message::Seconds,
                        &[("seconds", &self.value.idle_seconds.to_string())],
                    ));
                }
                choices
            }
            Field::SaverStyle => SAVER_STYLES.map(|(label, _)| lang.text(label)).to_vec(),
            Field::Language => Locale::ALL
                .iter()
                .map(|locale| locale.native_name().to_owned())
                .collect(),
        }
    }
    fn selected(&self, field: Field) -> usize {
        match field {
            Field::IdleAction => IDLE_ACTIONS
                .iter()
                .position(|(_, action)| *action == self.value.idle_action)
                .unwrap_or(0),
            Field::IdleTime => IDLE_SECONDS
                .iter()
                .position(|seconds| *seconds == self.value.idle_seconds)
                .unwrap_or(IDLE_SECONDS.len()),
            Field::SaverStyle => SAVER_STYLES
                .iter()
                .position(|(_, style)| *style == self.value.saver_style)
                .unwrap_or(0),
            Field::Language => Locale::ALL
                .iter()
                .position(|locale| *locale == self.value.language)
                .unwrap_or(0),
        }
    }
    fn set(&mut self, field: Field, index: usize) -> Outcome {
        match field {
            Field::IdleAction => self.value.idle_action = IDLE_ACTIONS[index].1,
            Field::IdleTime if index < IDLE_SECONDS.len() => {
                self.value.idle_seconds = IDLE_SECONDS[index]
            }
            Field::IdleTime => {}
            Field::SaverStyle => self.value.saver_style = SAVER_STYLES[index].1,
            Field::Language => self.value.language = Locale::ALL[index],
        }
        self.notice = match self.save() {
            Ok(()) => None,
            Err(err) => Some((Message::SettingsSaveError, err.to_string())),
        };
        Outcome::Changed
    }
    fn save(&self) -> io::Result<()> {
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut file, &self.value)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(&self.path).map_err(|err| err.error)?;
        Ok(())
    }
    fn activate(&mut self, direction: isize) -> Outcome {
        match self.focus {
            0..SELECTORS => {
                let field = Field::ALL[self.focus];
                let len = self.choices(field).len();
                let index = (self.selected(field) as isize + direction).rem_euclid(len as isize);
                self.set(field, index as usize)
            }
            SELECTORS => Outcome::Preview,
            _ => Outcome::Close,
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('S') => Outcome::Close,
            KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => {
                self.focus = (self.focus + 1) % ROWS;
                Outcome::Changed
            }
            KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => {
                self.focus = (self.focus + ROWS - 1) % ROWS;
                Outcome::Changed
            }
            KeyCode::Left if self.focus < SELECTORS => self.activate(-1),
            KeyCode::Right if self.focus < SELECTORS => self.activate(1),
            KeyCode::Enter | KeyCode::Char(' ') => self.activate(1),
            _ => Outcome::Ignored,
        }
    }
    pub fn mouse(&mut self, mouse: MouseEvent, area: Rect) -> Outcome {
        let hit = self
            .cells(area)
            .into_iter()
            .find(|cell| cell.rect.contains(Position::new(mouse.column, mouse.row)));
        let target = hit.as_ref().map(|cell| (cell.row, cell.option));
        let changed = self.hover != target;
        self.hover = target;
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            if let Some(cell) = hit {
                self.focus = cell.row;
                return if let Some(option) = cell.option {
                    self.set(Field::ALL[cell.row], option)
                } else {
                    self.activate(1)
                };
            }
        }
        if changed {
            Outcome::Changed
        } else {
            Outcome::Ignored
        }
    }
    fn cells(&self, area: Rect) -> Vec<Cell> {
        let inner = Block::bordered().inner(area);
        let step = if inner.height >= 22 {
            3
        } else if inner.height >= 15 {
            2
        } else {
            1
        };
        let label_width = label_width(inner);
        let mut cells = vec![];
        for row in 0..ROWS {
            let y = inner.y.saturating_add(2 + row as u16 * step);
            let row_rect = Rect::new(inner.x, y, inner.width, 1).intersection(inner);
            if row_rect.height == 0 || row_rect.width == 0 {
                continue;
            }
            if row >= SELECTORS {
                cells.push(Cell {
                    rect: row_rect,
                    row,
                    option: None,
                    text: format!(
                        "[ {} ]",
                        self.i18n().text(if row == SELECTORS {
                            Message::PreviewScreensaver
                        } else {
                            Message::Back
                        })
                    ),
                });
                continue;
            }
            let field = Field::ALL[row];
            if inner.width < 70 {
                let rect = Rect::new(
                    row_rect.x + label_width,
                    y,
                    row_rect.width.saturating_sub(label_width),
                    1,
                );
                cells.push(Cell {
                    rect,
                    row,
                    option: None,
                    text: format!("< {} >", self.choices(field)[self.selected(field)]),
                });
            } else {
                let mut x = row_rect.x + label_width;
                for (option, text) in self.choices(field).into_iter().enumerate() {
                    let text = format!("[ {text} ] ");
                    let width = Line::from(text.as_str()).width() as u16;
                    let rect = Rect::new(x, y, width, 1).intersection(row_rect);
                    x += width;
                    if rect.width > 0 {
                        cells.push(Cell {
                            rect,
                            row,
                            option: Some(option),
                            text,
                        });
                    }
                }
            }
        }
        cells
    }
    pub fn render(&self, area: Rect, buffer: &mut Buffer) {
        let theme = zellij_agent_board::theme();
        let lang = self.i18n();
        let block = Block::bordered()
            .title(format!(" {} ", lang.text(Message::SettingsTitle)))
            .border_style(Style::default().fg(theme.card_border));
        let inner = block.inner(area);
        block.render(area, buffer);
        Paragraph::new(lang.text(Message::SettingsHeading))
            .style(Style::default().fg(theme.text))
            .render(
                Rect::new(inner.x, inner.y, inner.width, 1).intersection(inner),
                buffer,
            );
        let cells = self.cells(area);
        for (row, field) in Field::ALL.into_iter().enumerate() {
            if let Some(cell) = cells.iter().find(|cell| cell.row == row) {
                let rect = Rect::new(inner.x, cell.rect.y, label_width(inner), 1);
                Paragraph::new(lang.text(field.label()))
                    .style(Style::default().fg(if self.focus == row {
                        theme.focus
                    } else {
                        theme.muted
                    }))
                    .render(rect, buffer);
            }
        }
        for cell in &cells {
            let selected = matches!(
                (Field::ALL.get(cell.row).copied(), cell.option),
                (Some(field), Some(option)) if self.selected(field) == option
            );
            let hover = self.hover == Some((cell.row, cell.option));
            let style = Style::default()
                .fg(if selected || self.focus == cell.row {
                    theme.focus
                } else {
                    theme.text
                })
                .bg(if hover {
                    theme.hover_fill
                } else if selected {
                    theme.focus_fill
                } else {
                    theme.surface
                });
            let style = if self.focus == cell.row {
                style.add_modifier(Modifier::BOLD)
            } else {
                style
            };
            Paragraph::new(cell.text.as_str())
                .style(style)
                .render(cell.rect, buffer);
        }
        if let (Some(last), Some((message, error))) = (cells.last(), &self.notice) {
            let y = last.rect.y + 2;
            Paragraph::new(lang.format(*message, &[("error", error)]))
                .style(Style::default().fg(theme.text))
                .render(
                    Rect::new(inner.x, y, inner.width, 1).intersection(inner),
                    buffer,
                );
        }
    }
}
struct Cell {
    rect: Rect,
    row: usize,
    option: Option<usize>,
    text: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    #[test]
    fn form_changes_persist_and_invalid_settings_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/settings.json");
        let mut form = Form::load(path.clone());
        form.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        form.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        form.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(
            Form::load(path.clone()).value,
            Preferences {
                idle_action: IdleAction::Monitor,
                idle_seconds: 60,
                saver_style: SaverStyle::Blank,
                ..Preferences::default()
            }
        );
        fs::write(&path, r#"{"idle_seconds":0}"#).unwrap();
        let invalid = Form::load(path);
        assert_eq!(invalid.value, Preferences::default());
        assert_eq!(invalid.notice.unwrap().0, Message::SettingsLoadError);
    }
    #[test]
    fn painted_choices_are_clickable_and_small_forms_do_not_overflow() {
        let dir = tempfile::tempdir().unwrap();
        let mut form = Form::load(dir.path().join("settings.json"));
        let area = Rect::new(0, 2, 120, 28);
        let cell = form
            .cells(area)
            .into_iter()
            .find(|c| c.row == 2 && c.option == Some(1))
            .unwrap();
        form.mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: cell.rect.x,
                row: cell.rect.y,
                modifiers: KeyModifiers::NONE,
            },
            area,
        );
        assert_eq!(
            Form::load(form.path.clone()).value.saver_style,
            SaverStyle::Clock
        );
        for width in 0..100 {
            for height in 0..25 {
                let area = Rect::new(0, 0, width, height);
                form.render(area, &mut Buffer::empty(area));
            }
        }
    }

    #[test]
    fn failed_save_keeps_preferences_active_and_reports_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::create_dir(&path).unwrap();
        let mut form = Form::load(path);
        form.set(Field::IdleAction, 3);
        assert_eq!(form.value.idle_action, IdleAction::Disabled);
        assert_eq!(form.notice.unwrap().0, Message::SettingsSaveError);
    }

    #[test]
    fn language_changes_live_and_persists_without_translating_protocol_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut form = Form::load(path.clone());
        for _ in 0..3 {
            form.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        }
        form.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(form.value.language, Locale::SimplifiedChinese);
        assert_eq!(form.i18n().text(Message::SettingsIdleAction), "空闲动作");
        assert_eq!(
            Form::load(path.clone()).value.language,
            Locale::SimplifiedChinese
        );
        let encoded = fs::read_to_string(&path).unwrap();
        assert!(encoded.contains("\"language\": \"zh-CN\""));
        assert!(encoded.contains("\"idle_action\": \"screensaver\""));
        let area = Rect::new(0, 2, 120, 28);
        let mut buffer = Buffer::empty(area);
        form.render(area, &mut buffer);
        // Buffer cells following wide glyphs contain padding, not extra text.
        let text: String = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .flat_map(str::chars)
            .filter(|ch| !ch.is_whitespace())
            .collect();
        assert!(text.contains("隐私与空闲保护"));
        assert!(text.contains("预览屏保"));
        // Render and dispatch Chinese choices after resizing too.
        for width in [120, 50] {
            let area = Rect::new(0, 2, width, 28);
            let cell = form
                .cells(area)
                .into_iter()
                .find(|cell| cell.row == 2 && (cell.option == Some(1) || cell.option.is_none()))
                .unwrap();
            form.value.saver_style = SaverStyle::Blank;
            form.mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: cell.rect.x,
                    row: cell.rect.y,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
            assert_eq!(form.value.saver_style, SaverStyle::Clock);
        }
        form.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        form.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        form.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        form.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(form.value.language, Locale::English);
    }

    #[test]
    fn old_and_unknown_language_settings_preserve_other_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for value in [
            r#"{"idle_seconds":60}"#,
            r#"{"idle_seconds":60,"language":"fr-FR"}"#,
        ] {
            fs::write(&path, value).unwrap();
            let form = Form::load(path.clone());
            assert_eq!(form.value.language, Locale::English);
            assert_eq!(form.value.idle_seconds, 60);
            assert!(form.notice.is_none());
        }
    }
}
