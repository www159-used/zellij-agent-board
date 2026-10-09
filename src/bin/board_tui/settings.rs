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
    widgets::{Block, Paragraph, Widget},
};
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub idle_action: IdleAction,
    pub idle_seconds: u64,
    pub saver_style: SaverStyle,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            idle_action: IdleAction::Screensaver,
            idle_seconds: 30,
            saver_style: SaverStyle::Blank,
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
    notice: Option<String>,
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
                        Err("idle_seconds must be between 1 and 86400".into())
                    }
                }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Preferences::default()),
            Err(err) => Err(err.to_string()),
        };
        let (value, notice) = match loaded {
            Ok(value) => (value, None),
            Err(err) => (
                Preferences::default(),
                Some(format!("Could not load settings; using defaults: {err}")),
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

    fn choices(&self, row: usize) -> Vec<String> {
        match row {
            0 => ["Screensaver", "Monitor", "Close GUI", "Disabled"]
                .map(str::to_owned)
                .to_vec(),
            1 => {
                let mut choices = vec!["30s".into(), "1min".into(), "5min".into()];
                if ![30, 60, 300].contains(&self.value.idle_seconds) {
                    choices.push(format!("{}s", self.value.idle_seconds));
                }
                choices
            }
            2 => vec!["Blank".into(), "Clock".into()],
            _ => vec![],
        }
    }
    fn selected(&self, row: usize) -> usize {
        match row {
            0 => match self.value.idle_action {
                IdleAction::Screensaver => 0,
                IdleAction::Monitor => 1,
                IdleAction::Close => 2,
                IdleAction::Disabled => 3,
            },
            1 => [30, 60, 300]
                .iter()
                .position(|n| *n == self.value.idle_seconds)
                .unwrap_or(3),
            2 => usize::from(self.value.saver_style == SaverStyle::Clock),
            _ => 0,
        }
    }
    fn set(&mut self, row: usize, index: usize) -> Outcome {
        match row {
            0 => {
                self.value.idle_action = [
                    IdleAction::Screensaver,
                    IdleAction::Monitor,
                    IdleAction::Close,
                    IdleAction::Disabled,
                ][index]
            }
            1 if index < 3 => self.value.idle_seconds = [30, 60, 300][index],
            2 => self.value.saver_style = [SaverStyle::Blank, SaverStyle::Clock][index],
            _ => {}
        }
        self.notice = match self.save() {
            Ok(()) => None,
            Err(err) => Some(format!("Changes active, but could not save: {err}")),
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
            0..=2 => {
                let len = self.choices(self.focus).len();
                let index =
                    (self.selected(self.focus) as isize + direction).rem_euclid(len as isize);
                self.set(self.focus, index as usize)
            }
            3 => Outcome::Preview,
            _ => Outcome::Close,
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('S') => Outcome::Close,
            KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => {
                self.focus = (self.focus + 1) % 5;
                Outcome::Changed
            }
            KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => {
                self.focus = (self.focus + 4) % 5;
                Outcome::Changed
            }
            KeyCode::Left if self.focus < 3 => self.activate(-1),
            KeyCode::Right if self.focus < 3 => self.activate(1),
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
                    self.set(cell.row, option)
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
        let step = if inner.height >= 19 {
            3
        } else if inner.height >= 13 {
            2
        } else {
            1
        };
        let label_width = 18.min(inner.width / 3);
        let mut cells = vec![];
        for row in 0..5 {
            let y = inner.y.saturating_add(2 + row as u16 * step);
            let field = Rect::new(inner.x, y, inner.width, 1).intersection(inner);
            if field.height == 0 || field.width == 0 {
                continue;
            }
            if row >= 3 {
                cells.push(Cell {
                    rect: field,
                    row,
                    option: None,
                    text: if row == 3 {
                        "[ Preview screensaver ]"
                    } else {
                        "[ Back ]"
                    }
                    .into(),
                });
            } else if inner.width < 70 {
                let rect = Rect::new(
                    field.x + label_width,
                    y,
                    field.width.saturating_sub(label_width),
                    1,
                );
                cells.push(Cell {
                    rect,
                    row,
                    option: None,
                    text: format!("< {} >", self.choices(row)[self.selected(row)]),
                });
            } else {
                let mut x = field.x + label_width;
                for (option, text) in self.choices(row).into_iter().enumerate() {
                    let text = format!("[ {text} ] ");
                    let rect = Rect::new(x, y, text.len() as u16, 1).intersection(field);
                    x += text.len() as u16;
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
        let block = Block::bordered()
            .title(" settings ")
            .border_style(Style::default().fg(theme.card_border));
        let inner = block.inner(area);
        block.render(area, buffer);
        Paragraph::new("Privacy / idle protection")
            .style(Style::default().fg(theme.text))
            .render(
                Rect::new(inner.x, inner.y, inner.width, 1).intersection(inner),
                buffer,
            );
        let cells = self.cells(area);
        for (row, label) in ["Idle action", "Idle time", "Saver style"]
            .into_iter()
            .enumerate()
        {
            if let Some(cell) = cells.iter().find(|cell| cell.row == row) {
                let rect = Rect::new(inner.x, cell.rect.y, 18.min(inner.width / 3), 1);
                Paragraph::new(label)
                    .style(Style::default().fg(if self.focus == row {
                        theme.focus
                    } else {
                        theme.muted
                    }))
                    .render(rect, buffer);
            }
        }
        for cell in &cells {
            let selected = cell
                .option
                .is_some_and(|option| self.selected(cell.row) == option);
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
        if let (Some(last), Some(notice)) = (cells.last(), &self.notice) {
            let y = last.rect.y + 2;
            Paragraph::new(notice.as_str())
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
                saver_style: SaverStyle::Blank
            }
        );
        fs::write(&path, r#"{"idle_seconds":0}"#).unwrap();
        let invalid = Form::load(path);
        assert_eq!(invalid.value, Preferences::default());
        assert!(invalid
            .notice
            .as_deref()
            .unwrap()
            .contains("Could not load"));
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
        form.set(0, 3);
        assert_eq!(form.value.idle_action, IdleAction::Disabled);
        assert!(form.notice.as_deref().unwrap().contains("could not save"));
    }
}
