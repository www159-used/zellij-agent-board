//! Mob two-line rows inside overview chrome: theme, footer pill, help, groups.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::{theme::theme, Agent, Board, HintField, Status};

#[derive(Debug, Clone, Copy)]
pub struct PaintCtx<'a> {
    pub rows: usize,
    pub cols: usize,
    pub home: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Frame {
    pub lines: Vec<String>,
}

/// Cells the host TUI must write to turn `prev` into `next`.
/// Identical frames stay empty so iTerm2 is not forced to repaint.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FramePatch<'a> {
    pub lines: Vec<(u16, &'a str)>,
    pub clear_from: Option<u16>,
}

impl FramePatch<'_> {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.clear_from.is_none()
    }
}

pub fn frame_patch<'a>(prev: Option<&Frame>, next: &'a Frame) -> FramePatch<'a> {
    let prev_len = prev.map(|frame| frame.lines.len()).unwrap_or(0);
    let lines = next
        .lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let y = u16::try_from(index).ok()?;
            match prev.and_then(|frame| frame.lines.get(index)) {
                Some(old) if old == line => None,
                _ => Some((y, line.as_str())),
            }
        })
        .collect();
    let clear_from =
        (next.lines.len() < prev_len).then(|| u16::try_from(next.lines.len()).unwrap_or(u16::MAX));
    FramePatch { lines, clear_from }
}

impl Frame {
    pub fn texts(&self) -> Vec<String> {
        self.lines.iter().map(|line| strip_ansi(line)).collect()
    }
}

pub fn paint(board: &Board, ctx: PaintCtx<'_>) -> Frame {
    if ctx.rows == 0 || ctx.cols == 0 {
        return Frame::default();
    }

    let rows = clamp_dim(ctx.rows, 32);
    let cols = clamp_dim(ctx.cols, 120);
    let area = Rect::new(0, 0, cols, rows);
    let mut buffer = Buffer::empty(area);
    render_board(board, ctx.home, area, &mut buffer);

    Frame {
        lines: crate::ansi::encode_lines(&buffer),
    }
}

/// Full-pane paint for the host TUI. Unlike [`paint`], this does not clamp to 128×32.
pub fn paint_to_size(board: &Board, home: &str, rows: u16, cols: u16) -> Frame {
    if rows == 0 || cols == 0 {
        return Frame::default();
    }
    Frame {
        lines: crate::ansi::encode_lines(&paint_buffer(board, home, rows, cols)),
    }
}

fn paint_buffer(board: &Board, home: &str, rows: u16, cols: u16) -> Buffer {
    let area = Rect::new(0, 0, cols, rows);
    let mut buffer = Buffer::empty(area);
    if rows > 0 && cols > 0 {
        render_board(board, home, area, &mut buffer);
    }
    buffer
}

/// Draw into an existing buffer. The host TUI uses the full pane; tests go through [`paint`].
pub fn render_board(board: &Board, home: &str, area: Rect, buffer: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (content, footer) = if area.height >= 3 {
        let [content, footer] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
        (content, Some(footer))
    } else {
        (area, None)
    };

    if board.help_visible {
        render_help(content, buffer);
    } else {
        render_list(board, home, content, buffer);
    }
    if let Some(footer) = footer {
        render_footer(board, home, footer, buffer);
    }
}

fn clamp_dim(value: usize, unlimited: u16) -> u16 {
    if value == usize::MAX {
        unlimited
    } else {
        value.min(128) as u16
    }
}

fn render_help(area: Rect, buffer: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::default()
        .title(" help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().focus));
    let inner = block.inner(area);
    block.render(area, buffer);
    let key = |text: &str| {
        Span::styled(
            format!("{text:<22}"),
            Style::default()
                .fg(theme().tip_typed)
                .add_modifier(Modifier::BOLD),
        )
    };
    let lines = vec![
        Line::from(vec![key("j/k  h/l  arrows"), Span::raw("move")]),
        Line::from(vec![
            key("10j  0  gg  G"),
            Span::raw("count / first / last"),
        ]),
        Line::from(vec![
            key("C-d  C-u  C-f  C-b"),
            Span::raw("half page / page"),
        ]),
        Line::from(vec![key("e  Enter"), Span::raw("go")]),
        Line::from(vec![key("s"), Span::raw("search")]),
        Line::from(vec![key("/  n  N"), Span::raw("find / next / prev")]),
        Line::from(vec![key("q  Esc"), Span::raw("close")]),
        Line::from(vec![key("Alt+q"), Span::raw("toggle board")]),
        Line::from(vec![key("!"), Span::raw("done, not opened yet")]),
        Line::from(vec![key("?"), Span::raw("close help")]),
    ];
    Paragraph::new(lines).render(inner, buffer);
}

fn render_list(board: &Board, home: &str, area: Rect, buffer: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if board.agents.is_empty() {
        Paragraph::new("no cursor agents")
            .style(Style::default().fg(theme().mask_fg))
            .render(area, buffer);
        return;
    }

    let cols = usize::from(area.width);
    let wide = cols >= 50;
    let multi = board
        .agents
        .windows(2)
        .any(|pair| pair[0].id.session != pair[1].id.session);
    let hinting = board.is_hinting();
    let flash = (hinting && !board.hint_query().is_empty())
        || (board.is_searching() && !board.search_query().is_empty());
    if hinting {
        buffer.set_style(area, Style::default().bg(theme().mask));
    }

    let mut y = area.y;
    y = draw_line(area, buffer, y, header_line(board));
    if !board.hooks_installed {
        y = draw_line(
            area,
            buffer,
            y,
            Line::from(Span::styled(
                "hooks 未装",
                Style::default().fg(theme().mask_fg),
            )),
        );
    }

    let body = Rect::new(area.x, y, area.width, area.bottom().saturating_sub(y));
    if body.height == 0 {
        return;
    }

    let (start, end) = board.list_viewport(body.height, wide);

    let mut y = body.y;
    let mut last_session = None;
    for index in start..end {
        let agent = &board.agents[index];
        if multi && last_session != Some(agent.id.session.as_str()) {
            if last_session.is_some() {
                y = draw_separator(body, buffer, y);
            }
            y = draw_line(
                body,
                buffer,
                y,
                session_head(board, &agent.id.session, flash),
            );
            last_session = Some(agent.id.session.as_str());
        }

        let candidate = board.agent_matches(index);
        let masked = flash && !candidate;
        let activity = wide.then(|| activity_text(agent)).flatten();
        let selected = index == board.selected;
        if selected && !masked {
            let height = if activity.is_some() { 2 } else { 1 };
            let fill = Rect::new(
                body.x,
                y,
                body.width,
                height.min(body.bottom().saturating_sub(y)),
            );
            if fill.height > 0 {
                buffer.set_style(fill, Style::default().bg(theme().focus_fill));
            }
        }
        y = draw_line(
            body,
            buffer,
            y,
            agent_row(board, index, selected, cols, home, multi, masked),
        );
        if let Some(text) = activity {
            y = draw_line(body, buffer, y, activity_line(&text, cols, masked));
        }
        if y >= body.bottom() {
            break;
        }
    }

    render_scroll_arrows(body, start, end, board.agents.len(), buffer);
}

fn render_scroll_arrows(area: Rect, start: usize, end: usize, total: usize, buffer: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let style = Style::default().fg(theme().tip_bg);
    let x = area.right().saturating_sub(1);
    if start > 0 {
        buffer.set_string(x, area.y, "↑", style);
    }
    if end < total {
        buffer.set_string(x, area.bottom().saturating_sub(1), "↓", style);
    }
}

fn render_footer(board: &Board, home: &str, area: Rect, buffer: &mut Buffer) {
    let line = if board.help_visible {
        footer_hints(area.width, None, &[("q", "close"), ("?", "close")])
    } else if board.is_searching() {
        Line::from(vec![
            Span::styled(
                " / ",
                Style::default().fg(theme().tip_fg).bg(theme().tip_bg),
            ),
            Span::styled(
                format!("{}█", board.search_query()),
                Style::default()
                    .fg(ratatui::style::Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Enter accept  n/N  Esc",
                Style::default().fg(theme().mask_fg),
            ),
        ])
    } else if board.is_hinting() {
        Line::from(vec![
            Span::styled(
                " FLASH ",
                Style::default().fg(theme().tip_fg).bg(theme().focus),
            ),
            Span::raw(" type to search"),
            Span::styled(
                format!("  /{}█", board.hint_query()),
                Style::default()
                    .fg(theme().focus)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  tip:{}", board.hint_jump_prefix()),
                Style::default()
                    .fg(theme().tip_typed)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if board.hint_query().is_empty() {
                    "  Esc cancel"
                } else {
                    "  Enter go  Esc"
                },
                Style::default().fg(theme().mask_fg),
            ),
        ])
    } else {
        footer_hints(
            area.width,
            (!home.is_empty()).then_some(home),
            &[
                ("j/k", "move"),
                ("e", "go"),
                ("s", "search"),
                ("/", "find"),
                ("q", "close"),
                ("?", "more"),
            ],
        )
    };
    Paragraph::new(line).render(area, buffer);
}

fn footer_hints(width: u16, home: Option<&str>, pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    let mut used = 0usize;
    if let Some(name) = home {
        let pill = format!(" {name} ");
        used += pill.chars().count() + 2;
        spans.push(Span::styled(
            pill,
            Style::default().fg(theme().tip_fg).bg(theme().session),
        ));
        spans.push(Span::raw("  "));
    }
    let budget = usize::from(width);
    for (index, (key, action)) in pairs.iter().enumerate() {
        let labeled = key.chars().count() + action.chars().count() + 4;
        let bare = key.chars().count() + 2;
        let (chunk, extra) = if used + labeled <= budget {
            (
                vec![
                    Span::styled(
                        format!(" {key} "),
                        Style::default().fg(theme().tip_fg).bg(theme().tip_bg),
                    ),
                    Span::raw(format!(" {action} ")),
                ],
                labeled,
            )
        } else if used + bare <= budget {
            (
                vec![Span::styled(
                    format!(" {key} "),
                    Style::default().fg(theme().tip_fg).bg(theme().tip_bg),
                )],
                bare,
            )
        } else {
            break;
        };
        if index > 0 && used + extra <= budget {
            spans.push(Span::styled(" ", Style::default().fg(theme().mask_fg)));
            used += 1;
        }
        used += extra;
        spans.extend(chunk);
    }
    Line::from(spans)
}

fn header_line(board: &Board) -> Line<'static> {
    let mut parts = Vec::new();
    push_count(&mut parts, board, Status::Working, "working");
    push_count(&mut parts, board, Status::Compact, "compact");
    push_count(&mut parts, board, Status::Done, "done");
    push_count(&mut parts, board, Status::Idle, "idle");
    push_count(&mut parts, board, Status::Found, "found");
    push_count(&mut parts, board, Status::Unknown, "unknown");

    if parts.is_empty() {
        return Line::from(Span::styled(
            "no status yet",
            Style::default().fg(theme().mask_fg),
        ));
    }

    let mut spans = Vec::new();
    for (i, (n, label, status)) in parts.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(theme().mask_fg)));
        }
        spans.push(Span::styled(
            format!("{n} {label}"),
            Style::default().fg(status_color(status)),
        ));
    }
    Line::from(spans)
}

fn push_count(
    parts: &mut Vec<(usize, &'static str, Status)>,
    board: &Board,
    status: Status,
    label: &'static str,
) {
    let n = board
        .agents
        .iter()
        .filter(|agent| agent.status == status)
        .count();
    if n > 0 {
        parts.push((n, label, status));
    }
}

fn session_head(board: &Board, name: &str, flash: bool) -> Line<'static> {
    let any_candidate = board
        .agents
        .iter()
        .enumerate()
        .any(|(index, agent)| agent.id.session == name && board.agent_matches(index));
    let muted = flash && !any_candidate;
    let mark = Style::default().fg(if muted {
        theme().mask_fg
    } else {
        theme().session
    });
    let mut spans = vec![Span::styled("◆ ", mark)];
    let range = (!muted)
        .then(|| text_match_range_local(name, board.highlight_query()))
        .flatten();
    if range.is_some() {
        spans.extend(highlight_text(name, range, mark));
    } else {
        spans.push(Span::styled(name.to_string(), mark));
    }
    Line::from(spans)
}

fn agent_row(
    board: &Board,
    index: usize,
    selected: bool,
    cols: usize,
    home: &str,
    multi: bool,
    masked: bool,
) -> Line<'static> {
    let agent = &board.agents[index];
    let status_style = if masked {
        Style::default().fg(theme().mask_fg)
    } else {
        Style::default().fg(row_status_color(agent))
    };
    let mut spans = Vec::new();
    if selected && !masked {
        spans.push(Span::styled(
            "› ",
            Style::default()
                .fg(theme().focus)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::raw("  "));
    }
    if let Some(label) = board.hint_label(index) {
        spans.extend(hint_badge(label, board.hint_jump_prefix().len()));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(format!("{} ", row_icon(agent)), status_style));
    spans.push(Span::styled(
        pad_right(agent.status.label(), 8),
        status_style,
    ));

    let (when, when_color) = time_cell(agent, board);
    let when_style = Style::default().fg(if masked { theme().mask_fg } else { when_color });
    spans.push(Span::styled(
        format!(" {}", pad_right(&when, 11)),
        when_style,
    ));

    let field = board.hint_match_field(index);
    // Flash highlights the matched field. The normal place column may show
    // project while the hit was tab/session — swap in the hit text so the
    // match (and tip row) always has a visible highlight.
    let place = if !masked && !board.highlight_query().is_empty() {
        place_for_match(agent, field, home, multi)
    } else {
        place_label(agent, home, multi)
    };
    let place_range = (!masked && !board.highlight_query().is_empty())
        .then(|| text_match_range_local(&place, board.highlight_query()))
        .flatten();
    spans.push(Span::raw(" "));
    if masked {
        spans.push(Span::styled(
            pad_right(&truncate(&place, 12), 12),
            Style::default().fg(theme().mask_fg),
        ));
    } else if let Some(range) = place_range {
        spans.extend(highlight_padded(&place, range, 12));
    } else {
        spans.push(Span::raw(pad_right(&truncate(&place, 12), 12)));
    }

    let used = line_width(&spans);
    let room = cols.saturating_sub(used + 1);
    let task = agent.display_task();
    if room > 3 && !task.is_empty() {
        let clipped = truncate(task, room);
        spans.push(Span::raw(" "));
        if masked {
            spans.push(Span::styled(clipped, Style::default().fg(theme().mask_fg)));
        } else {
            spans.push(Span::styled(
                clipped,
                Style::default()
                    .fg(theme().task)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }
    Line::from(spans)
}

fn place_label(agent: &Agent, home: &str, multi: bool) -> String {
    // Session name lives in ◆ headers when multiple sessions are listed.
    // Alone and away from home, the place column still needs the session.
    if !multi && !home.is_empty() && agent.id.session != home {
        return agent.id.session.clone();
    }
    let project = agent.project();
    if project != "-" {
        project.to_string()
    } else if !agent.tab_name.is_empty() {
        agent.tab_name.clone()
    } else {
        String::new()
    }
}

fn place_for_match(agent: &Agent, field: Option<HintField>, home: &str, multi: bool) -> String {
    match field {
        Some(HintField::Session) => agent.id.session.clone(),
        Some(HintField::Project) => {
            let project = agent.project();
            if project != "-" {
                project.to_string()
            } else {
                place_label(agent, home, multi)
            }
        }
        Some(HintField::Tab) if !agent.tab_name.is_empty() => agent.tab_name.clone(),
        _ => place_label(agent, home, multi),
    }
}

fn activity_text(agent: &Agent) -> Option<String> {
    (!agent.detail.is_empty()).then(|| agent.detail.clone())
}

fn activity_line(text: &str, cols: usize, masked: bool) -> Line<'static> {
    let _ = masked;
    Line::from(Span::styled(
        truncate(&format!("      └ {text}"), cols),
        Style::default().fg(theme().mask_fg),
    ))
}

fn hint_badge(label: &str, prefix_len: usize) -> Vec<Span<'static>> {
    let prefix_len = prefix_len.min(label.len());
    let (typed, remaining) = label.split_at(prefix_len);
    let mut spans = Vec::new();
    if !typed.is_empty() {
        spans.push(Span::styled(
            typed.to_string(),
            Style::default()
                .fg(theme().tip_typed)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if !remaining.is_empty() {
        spans.push(Span::styled(
            remaining.to_string(),
            Style::default().fg(theme().tip_fg).bg(theme().focus),
        ));
    }
    spans
}

fn highlight_text(text: &str, range: Option<(usize, usize)>, base: Style) -> Vec<Span<'static>> {
    let Some((start, len)) = range else {
        return vec![Span::styled(text.to_string(), base)];
    };
    let chars: Vec<char> = text.chars().collect();
    if start >= chars.len() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let end = (start + len).min(chars.len());
    let before: String = chars[..start].iter().collect();
    let matched: String = chars[start..end].iter().collect();
    let after: String = chars[end..].iter().collect();
    vec![
        Span::styled(before, base),
        Span::styled(
            matched,
            Style::default()
                .fg(theme().match_fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(after, base),
    ]
}

fn highlight_padded(text: &str, range: (usize, usize), width: usize) -> Vec<Span<'static>> {
    let clipped = truncate(text, width);
    let mut spans = highlight_text(&clipped, Some(range), Style::default());
    let used = spans.iter().map(Span::width).sum::<usize>();
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    spans
}

fn text_match_range_local(text: &str, query: &str) -> Option<(usize, usize)> {
    if query.is_empty() {
        return None;
    }
    let text_chars: Vec<char> = text.chars().map(|ch| ch.to_ascii_lowercase()).collect();
    let query_chars: Vec<char> = query.chars().map(|ch| ch.to_ascii_lowercase()).collect();
    text_chars
        .windows(query_chars.len())
        .position(|window| window == query_chars)
        .map(|start| (start, query_chars.len()))
}

fn pad_right(text: &str, width: usize) -> String {
    let used = Span::raw(text).width();
    if used >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - used))
    }
}

fn row_icon(agent: &crate::Agent) -> &'static str {
    if agent.unread_done() {
        "!"
    } else {
        agent.status.icon()
    }
}

fn row_status_color(agent: &crate::Agent) -> ratatui::style::Color {
    if agent.unread_done() {
        theme().pin_mark
    } else {
        status_color(agent.status)
    }
}

fn status_color(status: Status) -> ratatui::style::Color {
    match status {
        Status::Working | Status::Compact => theme().focus,
        Status::Done => theme().match_fg,
        Status::Failed | Status::Waiting | Status::IdleWait => theme().pin_mark,
        Status::Idle | Status::Found | Status::Unknown | Status::Ended => theme().mask_fg,
    }
}

/// Seconds shown in the clock column (working elapsed or done ago).
fn agent_time_secs(agent: &Agent, board: &Board) -> Option<u64> {
    match agent.status {
        Status::Working | Status::Compact => agent
            .started_at
            .map(|started| board.wall_now.saturating_sub(started)),
        Status::Done => {
            let base = if board.wall_at_open > 0 {
                board.wall_at_open
            } else {
                board.wall_now
            };
            agent
                .finished_at
                .map(|finished| base.saturating_sub(finished))
        }
        _ => None,
    }
}

/// Clock column: working stays neutral; done uses two tiers plus a "new" label.
fn time_cell(agent: &Agent, board: &Board) -> (String, ratatui::style::Color) {
    const STALE_DONE_SECS: u64 = 2 * 3600;

    let theme = theme();
    match agent.status {
        Status::Working | Status::Compact => {
            let when = agent_time_secs(agent, board)
                .map(fmt_elapsed)
                .unwrap_or_default();
            (when, theme.tip_bg)
        }
        Status::Done => {
            let Some(secs) = agent_time_secs(agent, board) else {
                return (String::new(), theme.mask_fg);
            };
            if agent.unread_done() {
                (format!("new {}", fmt_elapsed(secs)), theme.pin_mark)
            } else if secs > STALE_DONE_SECS {
                (fmt_ago(secs), theme.mask_fg)
            } else {
                (fmt_ago(secs), theme.tip_bg)
            }
        }
        _ => (String::new(), theme.mask_fg),
    }
}

fn draw_line(area: Rect, buffer: &mut Buffer, y: u16, line: Line<'_>) -> u16 {
    if y >= area.bottom() {
        return y;
    }
    Paragraph::new(line).render(Rect::new(area.x, y, area.width, 1), buffer);
    y.saturating_add(1)
}

fn draw_separator(area: Rect, buffer: &mut Buffer, y: u16) -> u16 {
    if y >= area.bottom() || area.width == 0 {
        return y;
    }
    Paragraph::new(Line::from(Span::styled(
        "─".repeat(usize::from(area.width)),
        Style::default().fg(theme().separator),
    )))
    .render(Rect::new(area.x, y, area.width, 1), buffer);
    y.saturating_add(1)
}

fn line_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn truncate(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return text.chars().take(1).collect();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

fn fmt_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn fmt_ago(secs: u64) -> String {
    format!("{} ago", fmt_elapsed(secs))
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{fmt_ago, fmt_elapsed, frame_patch, paint, paint_to_size, strip_ansi, PaintCtx};
    use crate::{Action, Agent, AgentId, Board, Key, Status};

    fn agent() -> Agent {
        Agent {
            id: AgentId {
                session: "ww".into(),
                pane_id: 3,
            },
            tool: "agent".into(),
            status: Status::Working,
            workspace: Some("/tmp/api".into()),
            tab_name: "notes".into(),
            tab_position: Some(1),
            pane_title: "Add retry".into(),
            detail: "Shell cargo test --lib".into(),
            status_since: 0,
            started_at: Some(1_700_000_000),
            finished_at: None,
            visited: false,
        }
    }

    fn painted(board: &Board, rows: usize, cols: usize, home: &str) -> String {
        paint(board, PaintCtx { rows, cols, home })
            .texts()
            .join("\n")
    }

    #[test]
    fn working_row_keeps_status_time_place_and_activity() {
        let mut board = Board {
            hooks_installed: true,
            now: 134,
            wall_now: 1_700_000_134,
            ..Board::default()
        };
        board.agents.push(agent());
        let text = painted(&board, 16, 110, "");
        for expect in [
            "›",
            "●",
            "working",
            "2m14s",
            "api",
            "Add retry",
            "└",
            "Shell cargo test --lib",
        ] {
            assert!(text.contains(expect), "missing {expect:?} in {text:?}");
        }
        assert!(!text.contains("agent "), "tool name is noise: {text:?}");
        assert!(!text.contains("tab:"), "tab id is noise: {text:?}");
        assert!(!text.contains("pane:"), "pane id is noise: {text:?}");
    }

    #[test]
    fn working_counts_up_and_done_shows_how_long_ago() {
        let mut board = Board {
            hooks_installed: true,
            now: 134,
            wall_now: 1_700_000_134,
            ..Board::default()
        };
        board.agents.push(agent());
        let mut done = agent();
        done.id.pane_id = 9;
        done.status = Status::Done;
        done.started_at = None;
        done.finished_at = Some(1_700_000_000);
        board.agents.push(done);
        let text = painted(&board, 20, 110, "");
        assert!(text.contains("working"));
        assert!(text.contains("2m14s"));
        assert!(text.contains("done"));
        assert!(text.contains("new 2m14s"), "{text}");
    }

    #[test]
    fn found_row_has_no_elapsed_and_no_placeholder_activity() {
        let mut board = Board {
            hooks_installed: true,
            now: 134,
            wall_now: 1_700_000_134,
            ..Board::default()
        };
        let mut agent = agent();
        agent.status = Status::Found;
        agent.detail.clear();
        board.agents.push(agent);
        let text = painted(&board, 16, 110, "");
        assert!(text.contains("found"));
        assert!(!text.contains("2m14s"));
        assert!(!text.contains("no report yet"));
        assert!(!text.contains('└'));
    }

    #[test]
    fn foreign_row_shows_session_instead_of_project() {
        let mut board = Board {
            hooks_installed: true,
            ..Board::default()
        };
        board.agents.push(agent());
        let home = painted(&board, 16, 110, "ww");
        assert!(home.contains("api"));
        let away = painted(&board, 16, 110, "lp");
        assert!(away.contains("ww"));
    }

    #[test]
    fn footer_has_session_pill_and_overview_keys() {
        let mut board = Board {
            hooks_installed: true,
            ..Board::default()
        };
        board.agents.push(agent());
        let text = painted(&board, 12, 80, "ww");
        assert!(text.contains(" ww "));
        assert!(text.contains(" go "));
        assert!(text.contains(" search "));
        assert!(text.contains(" more "));
        assert!(!text.contains("hjkl"));
        assert!(!text.contains("^d"));
    }

    #[test]
    fn groups_foreign_sessions_with_session_heads() {
        let mut board = Board {
            hooks_installed: true,
            ..Board::default()
        };
        let mut other = agent();
        other.id.session = "lp".into();
        other.id.pane_id = 8;
        board.agents.push(other);
        board.agents.push(agent());
        let text = painted(&board, 20, 80, "ww");
        assert!(text.contains('◆'));
        assert!(text.contains("lp"));
        assert!(text.contains("ww"));
    }

    #[test]
    fn columns_share_a_gutter_across_statuses() {
        let mut board = Board {
            hooks_installed: true,
            now: 134,
            wall_now: 1_700_000_134,
            ..Board::default()
        };
        board.agents.push(agent());
        let mut done = agent();
        done.id.pane_id = 9;
        done.status = Status::Done;
        done.started_at = None;
        done.finished_at = Some(1_700_000_000);
        done.pane_title = "Ship it".into();
        done.visited = true;
        board.agents.push(done);
        let text = painted(&board, 20, 110, "ww");
        let working = text
            .lines()
            .find(|line| line.contains("working") && line.contains("2m14s"))
            .expect("working row");
        let finished = text
            .lines()
            .find(|line| line.contains("2m14s ago"))
            .expect("done row");
        let w = display_index(working, "api").expect("working place");
        let d = display_index(finished, "api").expect("done place");
        assert_eq!(w, d, "place column drifted:\n{working}\n{finished}");
    }

    fn display_index(text: &str, needle: &str) -> Option<usize> {
        let bytes = text.find(needle)?;
        Some(ratatui::text::Span::raw(&text[..bytes]).width())
    }

    #[test]
    fn entering_flash_tints_the_list_and_keeps_the_status_header() {
        let mut board = Board {
            hooks_installed: true,
            now: 134,
            wall_now: 1_700_000_134,
            ..Board::default()
        };
        board.agents.push(agent());
        let browse_ansi = paint(
            &board,
            PaintCtx {
                rows: 16,
                cols: 80,
                home: "ww",
            },
        )
        .lines
        .join("\n");
        let browse = painted(&board, 16, 80, "ww");
        let browse_head = browse.lines().next().expect("browse header");
        assert!(
            browse_head.contains("working"),
            "browse should lead with status:\n{browse}"
        );
        assert!(
            !browse_ansi.contains("\u{1b}[48;2;30;24;48m"),
            "browse must not already be the flash tint:\n{browse_ansi}"
        );

        board.decide(Key::StartHint);
        let flash_ansi = paint(
            &board,
            PaintCtx {
                rows: 16,
                cols: 80,
                home: "ww",
            },
        )
        .lines
        .join("\n");
        let flash = painted(&board, 16, 80, "ww");
        let flash_head = flash.lines().next().expect("flash header");
        assert!(
            flash_head.contains("working"),
            "status header stays put:\n{flash}"
        );
        assert!(
            !flash_head.contains("FLASH"),
            "FLASH belongs in the footer, not the header:\n{flash}"
        );
        assert!(
            flash.contains("FLASH"),
            "footer still names the mode:\n{flash}"
        );
        assert!(
            flash_ansi.contains("\u{1b}[48;2;30;24;48m"),
            "empty flash must still tint the pane:\n{flash_ansi}"
        );
        assert!(
            flash_ansi.contains("\u{1b}[48;2;228;212;255m"),
            "FLASH pill should use focus lilac, not tip cool:\n{flash_ansi}"
        );

        board.decide(Key::Dismiss);
        board.decide(Key::StartSearch);
        let search_ansi = paint(
            &board,
            PaintCtx {
                rows: 16,
                cols: 80,
                home: "ww",
            },
        )
        .lines
        .join("\n");
        assert!(
            !search_ansi.contains("\u{1b}[48;2;30;24;48m"),
            "search must not reuse the flash tint:\n{search_ansi}"
        );
        assert!(
            search_ansi.contains("\u{1b}[48;2;162;197;206m"),
            "search pill stays on cool tip-bg:\n{search_ansi}"
        );
    }

    #[test]
    fn flash_shows_place_for_tab_and_project_hits() {
        let mut board = Board {
            hooks_installed: true,
            ..Board::default()
        };
        let mut tab_hit = agent();
        tab_hit.id.session = "lp".into();
        tab_hit.id.pane_id = 8;
        tab_hit.workspace = Some("/tmp/learn".into());
        tab_hit.tab_name = "notes".into();
        let mut project_hit = agent();
        project_hit.id.session = "ww".into();
        project_hit.id.pane_id = 3;
        project_hit.workspace = Some("/tmp/openapi".into());
        project_hit.tab_name = "main".into();
        board.agents = vec![tab_hit, project_hit];
        board.decide(Key::StartHint);
        assert_eq!(board.decide(Key::Input('o')), Action::None);
        assert_eq!(board.hint_query(), "o");
        assert!(board.agent_matches(0));
        assert!(board.agent_matches(1));
        let text = painted(&board, 20, 110, "ww");
        assert!(
            text.contains("notes"),
            "tab hit missing from place column:\n{text}"
        );
        assert!(
            text.contains("openapi"),
            "project hit missing from place column:\n{text}"
        );
        assert!(board.hint_label(0).is_some());
        assert!(board.hint_label(1).is_some());
    }

    #[test]
    fn help_replaces_the_list() {
        let mut board = Board {
            hooks_installed: true,
            ..Board::default()
        };
        board.agents.push(agent());
        board.decide(Key::ToggleHelp);
        let text = painted(&board, 16, 60, "ww");
        assert!(text.contains("help"));
        assert!(text.contains("C-d"));
        assert!(text.contains("count / first / last"));
        assert!(!text.contains("Shell cargo test --lib"));
    }

    #[test]
    fn empty_state_is_a_gray_line() {
        let board = Board {
            hooks_installed: true,
            ..Board::default()
        };
        let text = painted(&board, 8, 40, "ww");
        assert!(text.contains("no cursor agents"));
    }

    #[test]
    fn fmt_elapsed_matches_mob() {
        assert_eq!(fmt_elapsed(12), "12s");
        assert_eq!(fmt_elapsed(134), "2m14s");
        assert_eq!(fmt_elapsed(3661), "1h1m");
        assert_eq!(fmt_ago(134), "2m14s ago");
    }

    #[test]
    fn done_time_uses_two_colors_and_a_new_label() {
        let mut board = Board {
            hooks_installed: true,
            wall_now: 1_700_000_134,
            wall_at_open: 1_700_000_134,
            ..Board::default()
        };
        let mut unread = agent();
        unread.status = Status::Done;
        unread.started_at = None;
        unread.finished_at = Some(1_700_000_000);
        unread.visited = false;

        let mut fresh = agent();
        fresh.id.pane_id = 4;
        fresh.status = Status::Done;
        fresh.started_at = None;
        fresh.finished_at = Some(1_700_000_100);
        fresh.visited = true;

        let mut stale = agent();
        stale.id.pane_id = 5;
        stale.status = Status::Done;
        stale.started_at = None;
        stale.finished_at = Some(1_699_992_000);
        stale.visited = true;

        board.agents = vec![unread, fresh, stale];
        let text = paint(
            &board,
            PaintCtx {
                rows: 20,
                cols: 110,
                home: "ww",
            },
        )
        .lines
        .join("\n");
        let unread_line = text
            .lines()
            .find(|line| line.contains('!') && line.contains("new 2m14s"))
            .expect("unread done row");
        let fresh_line = text
            .lines()
            .find(|line| line.contains("34s ago"))
            .expect("fresh done row");
        let stale_line = text
            .lines()
            .find(|line| line.contains("2h15m ago"))
            .expect("stale done row");

        assert!(
            unread_line.contains("\u{1b}[38;2;249;169;182m"),
            "unread done time should use pin-mark: {unread_line}"
        );
        assert!(
            fresh_line.contains("\u{1b}[38;2;162;197;206m"),
            "read done time should stay on tip-bg: {fresh_line}"
        );
        assert!(
            stale_line.contains("\u{1b}[38;2;61;68;84m"),
            "stale done time should fade to mask-fg: {stale_line}"
        );
    }

    #[test]
    fn working_time_stays_neutral() {
        let mut board = Board {
            hooks_installed: true,
            wall_now: 1_700_000_800,
            ..Board::default()
        };
        let mut agent = agent();
        agent.started_at = Some(1_700_000_000);
        board.agents.push(agent);
        let text = paint(
            &board,
            PaintCtx {
                rows: 16,
                cols: 110,
                home: "",
            },
        )
        .lines
        .join("\n");
        let row = text
            .lines()
            .find(|line| line.contains("working") && line.contains("13m20s"))
            .expect("working row");
        assert!(
            row.contains("\u{1b}[38;2;162;197;206m"),
            "working time should stay on tip-bg: {row}"
        );
    }

    #[test]
    fn strip_ansi_keeps_visible_text() {
        assert_eq!(strip_ansi("\u{1b}[0m\u{1b}[38;2;1;2;3mhi\u{1b}[0m"), "hi");
    }

    #[test]
    fn identical_host_frames_write_nothing() {
        let mut board = Board {
            hooks_installed: true,
            now: 134,
            wall_now: 1_700_000_134,
            ..Board::default()
        };
        board.agents.push(agent());
        let first = paint_to_size(&board, "ww", 16, 80);
        let second = paint_to_size(&board, "ww", 16, 80);
        assert!(!first.lines.is_empty());
        assert!(frame_patch(Some(&first), &second).is_empty());
    }

    #[test]
    fn clock_tick_rewrites_only_changed_host_lines() {
        let mut board = Board {
            hooks_installed: true,
            now: 134,
            wall_now: 1_700_000_134,
            ..Board::default()
        };
        board.agents.push(agent());
        let before = paint_to_size(&board, "ww", 16, 80);
        board.tick();
        let after = paint_to_size(&board, "ww", 16, 80);
        let patch = frame_patch(Some(&before), &after);
        assert!(!patch.is_empty(), "elapsed text must move");
        assert!(
            patch.lines.len() < after.lines.len(),
            "tick rewrote the whole pane: {} of {}",
            patch.lines.len(),
            after.lines.len()
        );
        let full: usize = after.lines.iter().map(String::len).sum();
        let patch_bytes: usize = patch.lines.iter().map(|(_, line)| line.len()).sum();
        assert!(
            patch_bytes < full / 2,
            "tick patch {patch_bytes} vs full clear {full}"
        );
    }

    #[test]
    fn list_viewport_keeps_selected_row_visible_with_session_heads() {
        let mut board = Board::default();
        let mut scan = String::from("META hooks=1\n");
        for pane in 1..=6 {
            scan.push_str(&format!(
                "SCAN ww {pane} agent /Users/ww/.local/bin/agent --workspace /tmp/{pane}\n"
            ));
        }
        board.ingest(&scan);
        board.selected = 5;
        let text = painted(&board, 6, 80, "ww");
        assert!(
            text.lines().any(|line| line.starts_with('›')),
            "selected row should stay on screen:\n{text}"
        );
        assert!(
            !text
                .lines()
                .any(|line| line.starts_with('›') && line.contains("/1")),
            "viewport should scroll past the first row:\n{text}"
        );
    }
}
