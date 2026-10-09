//! An idle timer and a curtain independent of Home/Monitor state.
use super::settings::{IdleAction, Preferences, SaverStyle};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Style},
    widgets::{Clear, Paragraph, Widget},
};
use std::time::{Duration, Instant};

pub struct Privacy {
    last_input: Instant,
    fired: bool,
    pub active: bool,
}
impl Privacy {
    pub fn new(now: Instant) -> Self {
        Self {
            last_input: now,
            fired: false,
            active: false,
        }
    }
    pub fn activity(&mut self, now: Instant) {
        self.last_input = now;
        self.fired = false;
    }
    pub fn due(&mut self, now: Instant, settings: &Preferences) -> Option<IdleAction> {
        if self.active
            || self.fired
            || settings.idle_action == IdleAction::Disabled
            || now.saturating_duration_since(self.last_input)
                < Duration::from_secs(settings.idle_seconds)
        {
            return None;
        }
        self.fired = true;
        Some(settings.idle_action)
    }
    pub fn show(&mut self) {
        self.active = true;
        self.fired = true;
    }
    pub fn wake(&mut self, now: Instant) {
        self.active = false;
        self.activity(now);
    }
}

pub fn resume_button(area: Rect) -> Rect {
    let width = area.width.min(28);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height / 2,
        width,
        u16::from(area.height > 0),
    )
    .intersection(area)
}
pub fn render(area: Rect, style: SaverStyle, buffer: &mut Buffer) {
    Clear.render(area, buffer);
    buffer.set_style(area, Style::default().bg(Color::Black).fg(Color::Gray));
    let button = resume_button(area);
    if style == SaverStyle::Clock && button.y >= area.y + 2 {
        Paragraph::new(chrono::Local::now().format("%H:%M:%S").to_string())
            .alignment(Alignment::Center)
            .render(Rect::new(area.x, button.y - 2, area.width, 1), buffer);
    }
    Paragraph::new("[ Space / click to resume ]")
        .alignment(Alignment::Center)
        .render(button, buffer);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactivity_is_input_based_and_actions_fire_once_until_next_input() {
        let now = Instant::now();
        let mut idle = Privacy::new(now);
        let mut settings = Preferences::default();
        assert_eq!(idle.due(now + Duration::from_secs(29), &settings), None);
        idle.activity(now + Duration::from_secs(29));
        assert_eq!(idle.due(now + Duration::from_secs(30), &settings), None);
        assert_eq!(
            idle.due(now + Duration::from_secs(59), &settings),
            Some(IdleAction::Screensaver)
        );
        assert_eq!(idle.due(now + Duration::from_secs(60), &settings), None);
        idle.show();
        assert_eq!(idle.due(now + Duration::from_secs(120), &settings), None);
        idle.wake(now + Duration::from_secs(120));
        assert_eq!(idle.due(now + Duration::from_secs(149), &settings), None);
        settings.idle_action = IdleAction::Disabled;
        assert_eq!(idle.due(now + Duration::from_secs(180), &settings), None);
        for action in [IdleAction::Monitor, IdleAction::Close] {
            settings.idle_action = action;
            idle.activity(now);
            assert_eq!(
                idle.due(now + Duration::from_secs(30), &settings),
                Some(action)
            );
        }
    }
    #[test]
    fn curtain_erases_existing_contents_and_contains_no_application_data() {
        let area = Rect::new(0, 0, 100, 30);
        for style in [SaverStyle::Blank, SaverStyle::Clock] {
            let mut buffer = Buffer::empty(area);
            Paragraph::new("secret session workspace process").render(area, &mut buffer);
            render(area, style, &mut buffer);
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains("Space / click to resume"));
            assert!(!text.contains("secret"));
            assert!(!text.contains("process"));
        }
    }
}
