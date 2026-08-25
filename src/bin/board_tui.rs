//! Host TUI adapter. Scan, spool, places, keys, and ratatui live here.

use std::fs;
use std::io::{self, stdout, Write};
use std::path::Path;
use std::process::Command;
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
    focus_path, format_jump, parse_focus, parse_places, persist_seen, places_path, render_board,
    scan_host_text, scan_places_for, spool_dir, Action, AgentId, Board, Key, PIPE_NAME,
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
    places_mtime: Option<SystemTime>,
    spool_mtime: Option<SystemTime>,
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
            places_mtime: None,
            spool_mtime: None,
        }
    }

    fn bootstrap(&mut self) {
        self.rescan();
        self.reload_places();
        self.reload_spool();
        self.mark_launch_focus();
    }

    fn run(&mut self, terminal: &mut HostTerminal) -> io::Result<()> {
        let mut dirty = true;
        loop {
            if dirty {
                draw(terminal, &self.board, &self.home)?;
                dirty = false;
            }
            if event::poll(POLL)? {
                loop {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => {
                            match self.handle_key(key) {
                                Loop::Continue => dirty = true,
                                Loop::Quit => return Ok(()),
                            }
                        }
                        Event::Resize(_, _) => dirty = true,
                        _ => {}
                    }
                    if !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
                continue;
            }
            let now = Instant::now();
            if now.duration_since(self.last_scan) >= SCAN_EVERY {
                dirty |= self.rescan();
            }
            if file_changed(&places_path(), &mut self.places_mtime) {
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

    fn handle_key(&mut self, event: KeyEvent) -> Loop {
        let Some(key) = map_key(event, self.board.is_hinting()) else {
            return Loop::Continue;
        };
        match self.board.decide(key) {
            Action::Dismiss => Loop::Quit,
            Action::Jump { session, pane_id } => {
                persist_done_seen(&self.board, &session, pane_id);
                send_jump(&session, pane_id);
                Loop::Continue
            }
            Action::None => Loop::Continue,
        }
    }

    fn rescan(&mut self) -> bool {
        self.last_scan = Instant::now();
        let ingest = self.board.ingest(&scan_host_text());
        if self.should_refresh_places() {
            self.last_places = Instant::now();
            ingest | self.reload_host_places()
        } else {
            ingest
        }
    }

    fn should_refresh_places(&self) -> bool {
        self.board
            .agents
            .iter()
            .any(|agent| agent.pane_title.is_empty())
            || Instant::now().duration_since(self.last_places) >= PLACES_EVERY
    }

    fn reload_places(&mut self) -> bool {
        let text = fs::read_to_string(places_path()).unwrap_or_default();
        self.board.apply_places(parse_places(&text))
    }

    fn reload_host_places(&mut self) -> bool {
        let mut sessions: Vec<String> = self
            .board
            .agents
            .iter()
            .map(|agent| agent.id.session.clone())
            .collect();
        sessions.sort();
        sessions.dedup();
        self.board.apply_places(scan_places_for(&sessions))
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
    Continue,
    Quit,
}

fn map_key(event: KeyEvent, hinting: bool) -> Option<Key> {
    if event.modifiers != KeyModifiers::NONE && event.modifiers != KeyModifiers::SHIFT {
        return None;
    }
    match event.code {
        KeyCode::Esc | KeyCode::Char('q') => Some(Key::Dismiss),
        KeyCode::Char('?') if !hinting => Some(Key::ToggleHelp),
        KeyCode::Backspace if hinting => Some(Key::Backspace),
        KeyCode::Char('s') if !hinting => Some(Key::StartHint),
        KeyCode::Char(ch) if hinting => Some(Key::Input(ch)),
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Left | KeyCode::Char('h') if !hinting => {
            Some(Key::Up)
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Right | KeyCode::Char('l') if !hinting => {
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
    Ok(Terminal::new(PtyBackend { out })?)
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
