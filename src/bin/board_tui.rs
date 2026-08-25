//! Host TUI adapter. Scan, spool, places, keys, and ratatui live here.

use std::fs;
use std::io::{self, stdout, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{cursor, style::Print, QueueableCommand};
use zellij_agent_board::{
    focus_path, format_jump, frame_patch, paint_to_size, parse_focus, parse_places, persist_seen,
    places_path, scan_host_text, scan_places_for, spool_dir, Action, AgentId, Board, Frame, Key,
    PIPE_NAME,
};

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
    setup()?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.run()));
    restore()?;
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

    fn run(&mut self) -> io::Result<()> {
        let mut dirty = true;
        let mut last_frame: Option<Frame> = None;
        let mut term_size = size()?;
        loop {
            if dirty {
                draw(&mut last_frame, &self.board, &self.home)?;
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
                        Event::Resize(cols, rows) => {
                            if (cols, rows) != term_size {
                                term_size = (cols, rows);
                                last_frame = None;
                                dirty = true;
                            }
                        }
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

fn draw(last: &mut Option<Frame>, board: &Board, home: &str) -> io::Result<()> {
    let (cols, rows) = size()?;
    let frame = paint_to_size(board, home, rows, cols);
    let patch = frame_patch(last.as_ref(), &frame);
    if patch.is_empty() {
        return Ok(());
    }
    let mut out = stdout();
    for (y, line) in &patch.lines {
        out.queue(cursor::MoveTo(0, *y))?;
        out.queue(Print(*line))?;
        out.queue(Clear(ClearType::UntilNewLine))?;
    }
    if let Some(y) = patch.clear_from {
        out.queue(cursor::MoveTo(0, y))?;
        out.queue(Clear(ClearType::FromCursorDown))?;
    }
    out.flush()?;
    *last = Some(frame);
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

fn setup() -> io::Result<()> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, cursor::Hide)
}

fn restore() -> io::Result<()> {
    execute!(stdout(), cursor::Show, LeaveAlternateScreen)?;
    disable_raw_mode()
}
