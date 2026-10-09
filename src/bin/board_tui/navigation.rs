//! Navigation geometry is shared by painting and pointer dispatch.
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    widgets::{Paragraph, Widget},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Home,
    Monitor,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Page(Page),
    Settings,
    Quit,
}

pub struct Navigation {
    pub content: Rect,
    home: Rect,
    monitor: Rect,
    settings: Rect,
    quit: Rect,
    area: Rect,
}
impl Navigation {
    pub fn new(area: Rect) -> Self {
        let height = area.height.min(if area.height > 3 { 2 } else { 1 });
        let quit_width = area.width.min(8);
        let available = area.width.saturating_sub(quit_width);
        let home_width = (available / 3).min(14);
        let monitor_width = (available / 3).min(14);
        let settings_width = available.saturating_sub(home_width + monitor_width).min(14);
        Self {
            content: Rect::new(area.x, area.y + height, area.width, area.height - height),
            home: Rect::new(area.x, area.y, home_width, height.min(1)),
            monitor: Rect::new(area.x + home_width, area.y, monitor_width, height.min(1)),
            settings: Rect::new(
                area.x + home_width + monitor_width,
                area.y,
                settings_width,
                height.min(1),
            ),
            quit: Rect::new(area.right() - quit_width, area.y, quit_width, height.min(1)),
            area,
        }
    }
    pub fn hit(&self, column: u16, row: u16) -> Option<Target> {
        let position = Position::new(column, row);
        if self.home.contains(position) {
            Some(Target::Page(Page::Home))
        } else if self.monitor.contains(position) {
            Some(Target::Page(Page::Monitor))
        } else if self.settings.contains(position) {
            Some(Target::Settings)
        } else if self.quit.contains(position) {
            Some(Target::Quit)
        } else {
            None
        }
    }
    pub fn render(
        &self,
        page: Page,
        settings_open: bool,
        hover: Option<Target>,
        buffer: &mut Buffer,
    ) {
        let theme = zellij_agent_board::theme();
        for (rect, target, label, compact) in [
            (self.home, Target::Page(Page::Home), " h Agents ", "h"),
            (
                self.monitor,
                Target::Page(Page::Monitor),
                " t Monitor ",
                "t",
            ),
            (self.settings, Target::Settings, " S Settings ", "S"),
            (self.quit, Target::Quit, " q Quit ", "q"),
        ] {
            let selected = if settings_open {
                target == Target::Settings
            } else {
                target == Target::Page(page)
            };
            let style = if selected {
                Style::default()
                    .fg(theme.focus)
                    .bg(theme.focus_fill)
                    .add_modifier(Modifier::BOLD)
            } else if hover == Some(target) {
                Style::default().fg(theme.text).bg(theme.hover_fill)
            } else {
                Style::default().fg(theme.muted)
            };
            Paragraph::new(if rect.width >= label.len() as u16 {
                label
            } else {
                compact
            })
            .style(style)
            .render(rect, buffer);
        }
        if self.content.y > self.area.y + 1 {
            Paragraph::new("─".repeat(usize::from(self.area.width)))
                .style(Style::default().fg(theme.muted))
                .render(
                    Rect::new(self.area.x, self.area.y + 1, self.area.width, 1),
                    buffer,
                );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_hit_boxes_match_painted_cells_at_all_sizes() {
        for width in 0..120 {
            for height in 0..8 {
                let nav = Navigation::new(Rect::new(0, 0, width, height));
                assert_eq!(nav.home.intersection(nav.monitor).width, 0);
                assert_eq!(nav.monitor.intersection(nav.quit).width, 0);
                assert_eq!(nav.monitor.intersection(nav.settings).width, 0);
                assert_eq!(nav.settings.intersection(nav.quit).width, 0);
                for (rect, target) in [
                    (nav.home, Target::Page(Page::Home)),
                    (nav.monitor, Target::Page(Page::Monitor)),
                    (nav.settings, Target::Settings),
                    (nav.quit, Target::Quit),
                ] {
                    for x in rect.x..rect.right() {
                        for y in rect.y..rect.bottom() {
                            assert_eq!(nav.hit(x, y), Some(target));
                        }
                    }
                }
                assert_eq!(nav.hit(0, nav.content.y), None);
            }
        }
    }
}
