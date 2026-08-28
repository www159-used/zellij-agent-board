//! Host TUI adapter. Scan, spool, places, keys, and ratatui live here.
//!
//! MVC loop: `Board` + the places file are the model. The first frame paints
//! that cache. A background `list-panes` pass is the controller — it only
//! writes the cache and dirty-redraws when a title actually changed.

use std::fs;
use std::io::{self, stdout, Write};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Attribute, Colors, Print, SetAttribute, SetColors};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as term_size, Clear as TermClear, ClearType,
    EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, ClearType as Region, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};
use ratatui::widgets::{Clear, Widget};
use ratatui::Terminal;
use zellij_agent_board::{
    focus_path, format_jump, host_places_path, load_places, parse_focus, persist_places,
    persist_seen, render_board, scan_host_text, scan_places_for, spool_dir, Action, AgentId, Board,
    Key, PanePlace, PIPE_NAME,
};

type HostTerminal = Terminal<PtyBackend>;

const POLL: Duration = Duration::from_millis(200);
const SCAN_EVERY: Duration = Duration::from_secs(2);
const PLACES_EVERY: Duration = Duration::from_secs(10);
const TICK_EVERY: Duration = Duration::from_secs(1);

struct App {
    board: Board,
    home: String,
    last_scan: Instant,
    last_places: Instant,
    last_tick: Instant,
    host_places_mtime: Option<SystemTime>,
    spool_mtime: Option<SystemTime>,
    places_rx: Option<Receiver<Vec<(AgentId, PanePlace)>>>,
}

fn main() -> io::Result<()> {
    let mut app = App::new();
    app.bootstrap();
    let mut terminal = setup()?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.run(&mut terminal)));
    restore(&mut terminal)?;
    match result {
        Ok(ok) => ok,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

impl App {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            board: Board::default(),
            home: std::env::var("ZELLIJ_SESSION_NAME").unwrap_or_default(),
            last_scan: now - SCAN_EVERY,
            last_places: now - PLACES_EVERY,
            last_tick: now,
            host_places_mtime: None,
            spool_mtime: None,
            places_rx: None,
        }
    }

    fn bootstrap(&mut self) {
        self.last_scan = Instant::now();
        self.load_model();
        self.start_reconcile();
    }

    fn load_model(&mut self) {
        self.board.ingest(&scan_host_text());
        self.reload_places();
        self.reload_spool();
        self.mark_launch_focus();
    }

    fn run(&mut self, terminal: &mut HostTerminal) -> io::Result<()> {
        let mut dirty = true;
        loop {
            if dirty {
                if let Ok(size) = terminal.size() {
                    self.board
                        .set_page_len(visible_page(size.width, size.height));
                }
                draw(terminal, &self.board, &self.home)?;
                dirty = false;
            }
            if event::poll(POLL)? {
                match self.drain_keys()? {
                    DrainKeys::Quit => return Ok(()),
                    DrainKeys::Dirty => dirty = true,
                    DrainKeys::Idle => {}
                }
            }
            dirty |= self.take_host_places();
            let now = Instant::now();
            if now.duration_since(self.last_scan) >= SCAN_EVERY {
                dirty |= self.rescan();
            }
            if file_changed(&host_places_path(), &mut self.host_places_mtime) {
                dirty |= self.reload_places();
            }
            if dir_changed(&spool_dir(), &mut self.spool_mtime) {
                dirty |= self.reload_spool();
            }
            if self.board.needs_clock() && now.duration_since(self.last_tick) >= TICK_EVERY {
                self.board.tick();
                self.last_tick = now;
                dirty = true;
            }
        }
    }

    fn drain_keys(&mut self) -> io::Result<DrainKeys> {
        let mut dirty = false;
        loop {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    match self.handle_key(key) {
                        Loop::Changed => dirty = true,
                        Loop::Quit => return Ok(DrainKeys::Quit),
                        Loop::Ignored => {}
                    }
                }
                Event::Resize(_, _) => dirty = true,
                _ => {}
            }
            if !event::poll(Duration::ZERO)? {
                break;
            }
        }
        Ok(if dirty {
            DrainKeys::Dirty
        } else {
            DrainKeys::Idle
        })
    }

    fn handle_key(&mut self, event: KeyEvent) -> Loop {
        let Some(key) = map_key(event, self.board.is_hinting(), self.board.is_searching()) else {
            return Loop::Ignored;
        };
        match self.board.decide(key) {
            Action::Dismiss => Loop::Quit,
            Action::Jump { session, pane_id } => {
                persist_done_seen(&self.board, &session, pane_id);
                send_jump(&session, pane_id);
                Loop::Changed
            }
            Action::None => Loop::Changed,
        }
    }

    fn rescan(&mut self) -> bool {
        self.last_scan = Instant::now();
        let ingest = self.board.ingest(&scan_host_text());
        if self.places_rx.is_none() && places_due(self.last_places.elapsed()) {
            self.start_reconcile();
        }
        ingest
    }

    fn reload_places(&mut self) -> bool {
        self.board.apply_places(load_places())
    }

    fn start_reconcile(&mut self) {
        if self.places_rx.is_some() {
            return;
        }
        let sessions = reconcile_sessions(
            self.board
                .agents
                .iter()
                .map(|agent| agent.id.session.clone()),
            self.board.sessions_missing_titles(),
            &self.home,
        );
        if sessions.is_empty() {
            self.last_places = Instant::now();
            return;
        }
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for session in sessions {
                if tx.send(scan_places_for(&[session])).is_err() {
                    break;
                }
            }
        });
        self.places_rx = Some(rx);
    }

    fn take_host_places(&mut self) -> bool {
        let mut batch = Vec::new();
        let mut done = false;
        if let Some(rx) = &self.places_rx {
            loop {
                match rx.try_recv() {
                    Ok(places) => batch.push(places),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
        }
        if done {
            self.places_rx = None;
            self.last_places = Instant::now();
        }
        let mut dirty = false;
        for places in batch {
            persist_places(places.clone());
            if let Ok(meta) = fs::metadata(host_places_path()) {
                self.host_places_mtime = meta.modified().ok();
            }
            dirty |= self.board.apply_places(places);
        }
        dirty
    }

    fn reload_spool(&mut self) -> bool {
        let before = self.board.agents.clone();
        let before_hooks = self.board.hooks_installed;
        let mut text = String::new();
        if let Ok(entries) = fs::read_dir(spool_dir()) {
            for entry in entries.flatten() {
                if let Ok(body) = fs::read_to_string(entry.path()) {
                    text.push_str(&body);
                    if !body.ends_with('\n') {
                        text.push('\n');
                    }
                }
            }
        }
        self.board.ingest_notice(&text);
        self.board.hooks_installed != before_hooks || self.board.agents != before
    }

    fn mark_launch_focus(&mut self) {
        let text = fs::read_to_string(focus_path()).unwrap_or_default();
        let Some((session, pane_id)) = parse_focus(&text) else {
            return;
        };
        let id = AgentId {
            session: session.clone(),
            pane_id,
        };
        if self.board.mark_visited(&id) {
            persist_done_seen(&self.board, &session, pane_id);
        }
    }
}

enum Loop {
    Changed,
    Quit,
    Ignored,
}

enum DrainKeys {
    Quit,
    Dirty,
    Idle,
}

fn places_due(elapsed: Duration) -> bool {
    elapsed >= PLACES_EVERY
}

fn reconcile_sessions(
    sessions: impl IntoIterator<Item = String>,
    missing: impl IntoIterator<Item = String>,
    home: &str,
) -> Vec<String> {
    let mut rest: Vec<String> = sessions
        .into_iter()
        .filter(|name| !name.is_empty())
        .collect();
    rest.sort();
    rest.dedup();
    let mut missing: Vec<String> = missing
        .into_iter()
        .filter(|name| !name.is_empty() && name != home)
        .collect();
    missing.sort();
    missing.dedup();
    let mut out = Vec::new();
    if !home.is_empty() && rest.iter().any(|name| name == home) {
        out.push(home.to_string());
    }
    for name in missing {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    for name in rest {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

fn map_key(event: KeyEvent, hinting: bool, searching: bool) -> Option<Key> {
    let typing = hinting || searching;
    if event.modifiers.contains(KeyModifiers::CONTROL) && !typing {
        return match event.code {
            KeyCode::Char('d') => Some(Key::HalfPageDown),
            KeyCode::Char('u') => Some(Key::HalfPageUp),
            KeyCode::Char('f') => Some(Key::PageDown),
            KeyCode::Char('b') => Some(Key::PageUp),
            _ => None,
        };
    }
    if event.modifiers != KeyModifiers::NONE && event.modifiers != KeyModifiers::SHIFT {
        return None;
    }
    match event.code {
        KeyCode::Esc | KeyCode::Char('q') => Some(Key::Dismiss),
        KeyCode::Char('?') if !typing => Some(Key::ToggleHelp),
        KeyCode::Backspace if typing => Some(Key::Backspace),
        KeyCode::Char('s') if !typing => Some(Key::StartHint),
        KeyCode::Char('/') if !typing => Some(Key::StartSearch),
        KeyCode::Char('n') if !typing => Some(Key::NextMatch),
        KeyCode::Char('N') if !typing => Some(Key::PrevMatch),
        KeyCode::Char(ch) if typing => Some(Key::Input(ch)),
        KeyCode::Home if !typing => Some(Key::First),
        KeyCode::End if !typing => Some(Key::Last),
        KeyCode::PageDown if !typing => Some(Key::PageDown),
        KeyCode::PageUp if !typing => Some(Key::PageUp),
        KeyCode::Char('g') if !typing => Some(Key::GPrefix),
        KeyCode::Char('G') if !typing => Some(Key::Last),
        KeyCode::Char(ch) if !typing && ch.is_ascii_digit() => Some(Key::Digit(ch as u8 - b'0')),
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Left | KeyCode::Char('h') if !typing => {
            Some(Key::Up)
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Right | KeyCode::Char('l') if !typing => {
            Some(Key::Down)
        }
        KeyCode::Enter | KeyCode::Char('e') => Some(Key::Confirm),
        _ => None,
    }
}

fn persist_done_seen(board: &Board, session: &str, pane_id: u32) {
    let Some(agent) = board
        .agents
        .iter()
        .find(|agent| agent.id.session == session && agent.id.pane_id == pane_id)
    else {
        return;
    };
    let Some(finished_at) = agent.finished_at else {
        return;
    };
    persist_seen(session, pane_id, finished_at);
}

fn send_jump(session: &str, pane_id: u32) {
    let payload = format_jump(session, pane_id);
    let mut cmd = Command::new(zellij_bin());
    if let Ok(home) = std::env::var("ZELLIJ_SESSION_NAME") {
        if !home.is_empty() {
            cmd.args(["--session", &home]);
        }
    }
    let _ = cmd
        .args(["pipe", "--name", PIPE_NAME, "--", &payload])
        .status();
}

fn zellij_bin() -> String {
    ["/opt/homebrew/bin/zellij", "/usr/local/bin/zellij"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .unwrap_or("zellij")
        .to_string()
}

/// Draw through ratatui's cell diff. The PTY is still the only pipe out of
/// a Zellij pane — there is no direct Metal/iTerm2 handle — but we no longer
/// stitch ANSI lines ourselves.
fn visible_page(width: u16, height: u16) -> usize {
    let per = if width >= 50 { 2 } else { 1 };
    let budget = usize::from(height.saturating_sub(4));
    (budget / per).max(1)
}

fn draw(terminal: &mut HostTerminal, board: &Board, home: &str) -> io::Result<()> {
    terminal.draw(|frame| {
        let area = frame.area();
        Clear.render(area, frame.buffer_mut());
        render_board(board, home, area, frame.buffer_mut());
    })?;
    Ok(())
}

fn file_changed(path: &Path, seen: &mut Option<SystemTime>) -> bool {
    let mtime = fs::metadata(path).and_then(|meta| meta.modified()).ok();
    if mtime != *seen {
        *seen = mtime;
        mtime.is_some()
    } else {
        false
    }
}

fn dir_changed(path: &Path, seen: &mut Option<SystemTime>) -> bool {
    let mut latest = fs::metadata(path).and_then(|meta| meta.modified()).ok();
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(mtime) = entry.metadata().and_then(|meta| meta.modified()) {
                latest = Some(latest.map_or(mtime, |prev| prev.max(mtime)));
            }
        }
    }
    if latest != *seen {
        *seen = latest;
        latest.is_some()
    } else {
        false
    }
}

fn setup() -> io::Result<HostTerminal> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, cursor::Hide)?;
    Terminal::new(PtyBackend { out })
}

fn restore(terminal: &mut HostTerminal) -> io::Result<()> {
    execute!(terminal.backend_mut(), cursor::Show, LeaveAlternateScreen)?;
    disable_raw_mode()
}

/// Crossterm command backend. Ratatui diffs cells; we queue cursor / color /
/// print commands instead of stitching SGR strings. The PTY is still the only
/// way out of a Zellij pane — iTerm2's GPU is not reachable from here.
struct PtyBackend {
    out: io::Stdout,
}

impl Backend for PtyBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut last: Option<(u16, u16)> = None;
        let mut fg = Color::Reset;
        let mut bg = Color::Reset;
        let mut modifier = Modifier::empty();
        for (x, y, cell) in content {
            if !matches!(last, Some((px, py)) if x == px.saturating_add(1) && y == py) {
                queue!(self.out, cursor::MoveTo(x, y))?;
            }
            last = Some((x, y));
            if cell.modifier != modifier {
                queue!(self.out, SetAttribute(Attribute::Reset))?;
                if cell.modifier.contains(Modifier::BOLD) {
                    queue!(self.out, SetAttribute(Attribute::Bold))?;
                }
                if cell.modifier.contains(Modifier::DIM) {
                    queue!(self.out, SetAttribute(Attribute::Dim))?;
                }
                modifier = cell.modifier;
                fg = Color::Reset;
                bg = Color::Reset;
            }
            if cell.fg != fg || cell.bg != bg {
                queue!(
                    self.out,
                    SetColors(Colors::new(to_crossterm(cell.fg), to_crossterm(cell.bg)))
                )?;
                fg = cell.fg;
                bg = cell.bg;
            }
            queue!(self.out, Print(cell.symbol()))?;
        }
        queue!(
            self.out,
            SetAttribute(Attribute::Reset),
            SetColors(Colors::new(
                crossterm::style::Color::Reset,
                crossterm::style::Color::Reset
            ))
        )
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(self.out, cursor::Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(self.out, cursor::Show)
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        let (x, y) = cursor::position()?;
        Ok(Position { x, y })
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        execute!(self.out, cursor::MoveTo(position.x, position.y))
    }

    fn clear(&mut self) -> io::Result<()> {
        execute!(self.out, TermClear(ClearType::All))
    }

    fn clear_region(&mut self, clear_type: Region) -> io::Result<()> {
        let kind = match clear_type {
            Region::All => ClearType::All,
            Region::AfterCursor => ClearType::FromCursorDown,
            Region::BeforeCursor => ClearType::FromCursorUp,
            Region::CurrentLine => ClearType::CurrentLine,
            Region::UntilNewLine => ClearType::UntilNewLine,
        };
        execute!(self.out, TermClear(kind))
    }

    fn size(&self) -> io::Result<Size> {
        let (width, height) = term_size()?;
        Ok(Size::new(width, height))
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        let columns_rows = self.size()?;
        Ok(WindowSize {
            columns_rows,
            pixels: Size::new(0, 0),
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

impl io::Write for PtyBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.out.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

fn to_crossterm(color: Color) -> crossterm::style::Color {
    use crossterm::style::Color as C;
    match color {
        Color::Reset => C::Reset,
        Color::Black => C::Black,
        Color::Red => C::Red,
        Color::Green => C::Green,
        Color::Yellow => C::Yellow,
        Color::Blue => C::Blue,
        Color::Magenta => C::Magenta,
        Color::Cyan => C::Cyan,
        Color::Gray => C::Grey,
        Color::DarkGray => C::DarkGrey,
        Color::LightRed => C::DarkRed,
        Color::LightGreen => C::DarkGreen,
        Color::LightYellow => C::DarkYellow,
        Color::LightBlue => C::DarkBlue,
        Color::LightMagenta => C::DarkMagenta,
        Color::LightCyan => C::DarkCyan,
        Color::White => C::White,
        Color::Rgb(r, g, b) => C::Rgb { r, g, b },
        Color::Indexed(index) => C::AnsiValue(index),
    }
}

#[cfg(test)]
mod tests {
    use super::{places_due, reconcile_sessions, PLACES_EVERY};
    use std::time::Duration;

    #[test]
    fn first_paint_does_not_refresh_places_just_because_titles_are_empty() {
        assert!(!places_due(Duration::ZERO));
        assert!(!places_due(PLACES_EVERY - Duration::from_millis(1)));
        assert!(places_due(PLACES_EVERY));
    }

    #[test]
    fn reconcile_checks_home_then_missing_then_the_rest() {
        assert_eq!(
            reconcile_sessions(
                ["ww".into(), "lp".into(), "daily".into(), "ww".into()],
                ["lp".into(), "daily".into()],
                "ww"
            ),
            vec!["ww", "daily", "lp"]
        );
        assert_eq!(
            reconcile_sessions(["lp".into(), "".into()], ["lp".into()], ""),
            vec!["lp"]
        );
        assert_eq!(
            reconcile_sessions(
                ["ww".into(), "lp".into(), "jcm".into()],
                ["lp".into()],
                "ww"
            ),
            vec!["ww", "lp", "jcm"]
        );
    }
}
