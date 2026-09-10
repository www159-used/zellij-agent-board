//! Host TUI adapter. Scan, spool, places, keys, and ratatui live here.
//!
//! MVC loop: `Board` plus the host store are the model. The first frame
//! paints the last SCAN snapshot. A sibling `--reconcile` process writes
//! the store; this process only reads.

use std::fs;
use std::io::{self, stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crossterm::cursor;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
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
    focus_path, format_jump, load_places, load_scan, parse_focus, persist_places, persist_seen,
    places_path, reconcile_once, render_board, run_reconcile, runtime_dir, scan_path,
    scan_places_for, spool_dir, zellij_bin, Action, AgentId, Board, Key, PIPE_NAME,
};

type HostTerminal = Terminal<PtyBackend>;

const POLL: Duration = Duration::from_millis(200);
const SCAN_EVERY: Duration = Duration::from_secs(2);
const TICK_EVERY: Duration = Duration::from_secs(1);

struct App {
    board: Board,
    home: String,
    /// Last pane size, so clicks can be resolved against the painted frame.
    view: Size,
    /// Last pointer position. Scrolling slides rows under a still pointer,
    /// so the hover preview has to be resolved again.
    pointer: Option<(u16, u16)>,
    last_reconcile: Instant,
    last_tick: Instant,
    scan_mtime: Option<SystemTime>,
    places_mtime: Option<SystemTime>,
    spool_mtime: Option<SystemTime>,
}

fn log_path() -> PathBuf {
    runtime_dir().join("board.log")
}

const LOG_ROTATE_BYTES: u64 = 10 * 1024 * 1024;
const LOG_KEEP_GZ: usize = 5;

/// File-only logger so the TUI PTY stays clean. Appends to `board.log`; at
/// 10 MiB the file is gzipped to `board-YYYYMMDD-HHMMSS.log.gz` (keep 5).
/// TUI and `--reconcile` share the path; rotation uses a lock file.
fn init_logging() {
    use std::sync::OnceLock;
    static LOGGER: OnceLock<BoardFileLogger> = OnceLock::new();
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let logger = LOGGER.get_or_init(|| BoardFileLogger { path });
    let _ = log::set_logger(logger).map(|()| log::set_max_level(log::LevelFilter::Info));
}

struct BoardFileLogger {
    path: PathBuf,
}

impl log::Log for BoardFileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {} {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            record.level(),
            record.args()
        );
        rotate_board_log_if_needed(&self.path);
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{line}");
        }
    }

    fn flush(&self) {}
}

fn rotate_board_log_if_needed(path: &Path) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() < LOG_ROTATE_BYTES {
        return;
    }
    let lock_path = path.with_extension("log.lock");
    let Ok(lock) = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
    else {
        return;
    };
    use fs2::FileExt;
    if lock.try_lock_exclusive().is_err() {
        return;
    }
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() < LOG_ROTATE_BYTES {
        return;
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let Some(parent) = path.parent() else {
        return;
    };
    let gz_path = parent.join(format!("board-{stamp}.log.gz"));
    let tmp_path = path.with_extension("log.rotating");
    if fs::rename(path, &tmp_path).is_err() {
        return;
    }
    let compressed = (|| -> io::Result<()> {
        let mut input = fs::File::open(&tmp_path)?;
        let output = fs::File::create(&gz_path)?;
        let mut encoder = flate2::write::GzEncoder::new(output, flate2::Compression::default());
        io::copy(&mut input, &mut encoder)?;
        encoder.finish()?;
        Ok(())
    })();
    let _ = fs::remove_file(&tmp_path);
    if compressed.is_err() {
        let _ = fs::remove_file(&gz_path);
        return;
    }
    prune_board_log_gz(parent, LOG_KEEP_GZ);
}

fn prune_board_log_gz(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut gz: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("board-") && name.ends_with(".log.gz"))
        })
        .collect();
    gz.sort();
    let drop = gz.len().saturating_sub(keep);
    for path in gz.into_iter().take(drop) {
        let _ = fs::remove_file(path);
    }
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h" | "--help") => {
            eprint!(
                "\
board-tui — host dashboard for zellij-agent-board

  board-tui                         live board (needs a TTY)
  board-tui --reconcile             write the host store once and exit
  board-tui --replay FILE.scene     run an e2e scene; no TTY

Log: {}
  rotates at 10MiB → board-YYYYMMDD-HHMMSS.log.gz (keep 5)
",
                log_path().display()
            );
            return Ok(());
        }
        Some("--reconcile") => {
            init_logging();
            let _ = run_reconcile();
            return Ok(());
        }
        Some("--replay") => {
            let Some(path) = args.get(1) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "board-tui --replay FILE.scene",
                ));
            };
            return replay(path);
        }
        Some(other) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown argument {other}"),
            ));
        }
        None => {}
    }
    init_logging();
    let mut app = App::new();
    app.bootstrap();
    log::info!(
        "open session={} agents={}",
        app.home,
        app.board.agents.len()
    );
    let mut terminal = setup()?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.run(&mut terminal)));
    restore(&mut terminal)?;
    log::info!("close session={}", app.home);
    match result {
        Ok(ok) => ok,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn replay(path: &str) -> io::Result<()> {
    let source = fs::read_to_string(path)?;
    zellij_agent_board::run_scene(&source).map_err(|err| io::Error::other(format!("{path}: {err}")))
}

impl App {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            board: Board::default(),
            home: std::env::var("ZELLIJ_SESSION_NAME").unwrap_or_default(),
            view: Size::new(0, 0),
            pointer: None,
            last_reconcile: now - SCAN_EVERY,
            last_tick: now,
            scan_mtime: None,
            places_mtime: None,
            spool_mtime: None,
        }
    }

    fn bootstrap(&mut self) {
        self.load_model();
        spawn_reconcile();
        self.last_reconcile = Instant::now();
    }

    fn load_model(&mut self) {
        let had_cache = load_scan().is_some();
        if !had_cache {
            let _ = reconcile_once();
        }
        if let Some(cached) = load_scan() {
            self.board.ingest(&cached);
        }
        self.reload_places();
        self.fill_home_titles();
        self.reload_spool();
        self.mark_launch_focus();
        log::info!(
            "bootstrap session={} cache={} agents={}",
            self.home,
            had_cache,
            self.board.agents.len()
        );
    }

    /// One `list-panes` for this session before the first paint. Remote
    /// sessions wait for the background reconcile; home titles must not.
    fn fill_home_titles(&mut self) {
        if self.home.is_empty() {
            return;
        }
        if !self
            .board
            .sessions_missing_titles()
            .iter()
            .any(|session| session == &self.home)
        {
            return;
        }
        persist_places(scan_places_for(std::slice::from_ref(&self.home)));
        self.reload_places();
    }

    fn run(&mut self, terminal: &mut HostTerminal) -> io::Result<()> {
        let mut dirty = true;
        loop {
            if let Ok(size) = terminal.size() {
                dirty |= size != self.view;
                self.view = size;
            }
            if dirty {
                self.board
                    .set_list_geometry(self.view.width, self.view.height);
                draw(terminal, &self.board, &self.home)?;
                dirty = false;
            }
            if event::poll(POLL)? {
                match self.drain_input()? {
                    DrainInput::Quit => return Ok(()),
                    DrainInput::Dirty => dirty = true,
                    DrainInput::Idle => {}
                }
            }
            dirty |= self.take_store();
            let now = Instant::now();
            if now.duration_since(self.last_reconcile) >= SCAN_EVERY {
                spawn_reconcile();
                self.last_reconcile = Instant::now();
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

    fn drain_input(&mut self) -> io::Result<DrainInput> {
        let mut dirty = false;
        loop {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    match self.handle_key(key) {
                        Loop::Changed => dirty = true,
                        Loop::Quit => return Ok(DrainInput::Quit),
                        Loop::Ignored => {}
                    }
                }
                Event::Mouse(mouse) => match self.handle_mouse(mouse) {
                    Loop::Changed => dirty = true,
                    Loop::Quit => return Ok(DrainInput::Quit),
                    Loop::Ignored => {}
                },
                Event::Resize(_, _) => dirty = true,
                _ => {}
            }
            if !event::poll(Duration::ZERO)? {
                break;
            }
        }
        Ok(if dirty {
            DrainInput::Dirty
        } else {
            DrainInput::Idle
        })
    }

    fn handle_key(&mut self, event: KeyEvent) -> Loop {
        // C-e / C-y: same move-together scroll as the wheel (spotlight holds
        // its screen line; the list flows under it).
        if let Some(delta) = view_scroll_delta(event) {
            let moved = self.board.scroll_view(delta);
            let preview = self.refresh_hover();
            return if moved || preview {
                Loop::Changed
            } else {
                Loop::Ignored
            };
        }
        let mapped = if self.board.is_picking() {
            map_picker_key(event)
        } else {
            map_key(event, self.board.is_hinting(), self.board.is_searching())
        };
        let Some(key) = mapped else {
            return Loop::Ignored;
        };
        let before = ModeSnap::capture(&self.board);
        match self.board.decide(key) {
            Action::Dismiss => {
                log::info!("quit");
                Loop::Quit
            }
            Action::Jump { session, pane_id } => {
                log_mode_ends(&before, &self.board, "jump");
                persist_done_seen(&self.board, &session, pane_id);
                send_jump(&session, pane_id, "key");
                Loop::Changed
            }
            Action::None => {
                log_mode_change(key, &before, &self.board);
                Loop::Changed
            }
        }
    }

    /// Pointer input: a click jumps to the row under it, motion previews
    /// that row, the wheel scrolls the view. Hover never steals `selected`.
    /// Right button is ignored.
    fn handle_mouse(&mut self, mouse: MouseEvent) -> Loop {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let before = ModeSnap::capture(&self.board);
                match self
                    .board
                    .click(self.view.height, self.view.width, mouse.column, mouse.row)
                {
                    Action::Jump { session, pane_id } => {
                        log_mode_ends(&before, &self.board, "jump");
                        persist_done_seen(&self.board, &session, pane_id);
                        send_jump(&session, pane_id, "click");
                        Loop::Changed
                    }
                    Action::None => Loop::Changed,
                    Action::Dismiss => {
                        log::info!("quit");
                        Loop::Quit
                    }
                }
            }
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                self.pointer = Some((mouse.column, mouse.row));
                if self
                    .board
                    .hover(self.view.height, self.view.width, mouse.column, mouse.row)
                {
                    Loop::Changed
                } else {
                    Loop::Ignored
                }
            }
            // The wheel scrolls the view. Three lines per notch matches nvim's
            // `mousescroll=ver:3` — a trackpad quantizes smooth swipes into
            // discrete wheel ticks, and one line a tick is easy to miss.
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if mouse.kind == MouseEventKind::ScrollUp {
                    -3
                } else {
                    3
                };
                let moved = self.board.scroll_view(delta);
                let preview = self.refresh_hover();
                if moved || preview {
                    Loop::Changed
                } else {
                    Loop::Ignored
                }
            }
            _ => Loop::Ignored,
        }
    }

    /// Rows slid under a still pointer: re-resolve the hover so the preview
    /// keeps sitting under the cursor instead of riding the old row away.
    fn refresh_hover(&mut self) -> bool {
        let Some((column, row)) = self.pointer else {
            return false;
        };
        self.board
            .hover(self.view.height, self.view.width, column, row)
    }

    fn take_store(&mut self) -> bool {
        let mut dirty = false;
        if file_changed(&scan_path(), &mut self.scan_mtime) {
            if let Some(text) = load_scan() {
                let before = self.board.agents.len();
                if self.board.ingest(&text) {
                    dirty = true;
                    log::info!(
                        "scan_reload agents_before={before} agents={}",
                        self.board.agents.len()
                    );
                }
            }
        }
        if file_changed(&places_path(), &mut self.places_mtime) {
            if self.reload_places() {
                dirty = true;
                log::info!("places_reload");
            }
        }
        dirty
    }

    fn reload_places(&mut self) -> bool {
        self.board.apply_places(load_places())
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
        let changed = self.board.hooks_installed != before_hooks || self.board.agents != before;
        if changed {
            log::info!("spool_reload hooks={}", self.board.hooks_installed);
        }
        changed
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

enum DrainInput {
    Quit,
    Dirty,
    Idle,
}

fn spawn_reconcile() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.arg("--reconcile")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Leave the TUI process group so `q` / --close-on-exit does not SIGHUP
    // a list-panes pass that still has to write the home session.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let _ = cmd.spawn();
}

/// C-e / C-y: the view-scroll pair. Positive reveals later rows.
fn view_scroll_delta(event: KeyEvent) -> Option<i32> {
    if event.modifiers != KeyModifiers::CONTROL {
        return None;
    }
    match event.code {
        KeyCode::Char('e') => Some(1),
        KeyCode::Char('y') => Some(-1),
        _ => None,
    }
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
        // Flash/search (like flash.nvim): Esc aborts. `q` is a tip/query
        // char while typing; only idle `q` quits the board.
        KeyCode::Esc => Some(Key::Dismiss),
        KeyCode::Char('q') if !typing => Some(Key::Dismiss),
        KeyCode::Char('?') if !typing => Some(Key::ToggleHelp),
        KeyCode::Backspace if typing => Some(Key::Backspace),
        KeyCode::Char('s') if !typing => Some(Key::StartHint),
        KeyCode::Char('/') if !typing => Some(Key::StartSearch),
        KeyCode::Char('p') if !typing => Some(Key::StartPicker),
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

/// Keys while the floating picker is up. Printable chars go to the query
/// or to tip labels; `decide` splits them by focus. Esc closes, Tab flips.
fn map_picker_key(event: KeyEvent) -> Option<Key> {
    if event.modifiers != KeyModifiers::NONE && event.modifiers != KeyModifiers::SHIFT {
        return None;
    }
    match event.code {
        KeyCode::Esc => Some(Key::Dismiss),
        KeyCode::Tab => Some(Key::TogglePickerFocus),
        KeyCode::Enter => Some(Key::Confirm),
        KeyCode::Backspace => Some(Key::Backspace),
        // Query and tips both need `q` as a typed char; Esc still closes.
        KeyCode::Char(ch) => Some(Key::Input(ch)),
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

fn send_jump(session: &str, pane_id: u32, via: &str) {
    let payload = format_jump(session, pane_id);
    log::info!("jump to={session} pane={pane_id} via={via}");
    let mut cmd = Command::new(zellij_bin());
    if let Ok(home) = std::env::var("ZELLIJ_SESSION_NAME") {
        if !home.is_empty() {
            cmd.args(["--session", &home]);
        }
    }
    match cmd
        .args(["pipe", "--name", PIPE_NAME, "--", &payload])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => log::warn!(
            "jump_pipe_fail to={session} pane={pane_id} via={via} code={}",
            status.code().unwrap_or(-1)
        ),
        Err(err) => log::warn!("jump_pipe_fail to={session} pane={pane_id} via={via} err={err}"),
    }
}

#[derive(Clone, Copy)]
struct ModeSnap {
    hinting: bool,
    searching: bool,
    picking: bool,
}

impl ModeSnap {
    fn capture(board: &Board) -> Self {
        Self {
            hinting: board.is_hinting(),
            searching: board.is_searching(),
            picking: board.is_picking(),
        }
    }
}

fn log_mode_ends(before: &ModeSnap, board: &Board, reason: &str) {
    let after = ModeSnap::capture(board);
    if before.hinting && !after.hinting {
        log::info!("flash_end reason={reason}");
    }
    if before.searching && !after.searching {
        log::info!("search_end reason={reason}");
    }
    if before.picking && !after.picking {
        log::info!("picker_end reason={reason}");
    }
}

fn log_mode_change(key: Key, before: &ModeSnap, board: &Board) {
    let after = ModeSnap::capture(board);
    if !before.hinting && after.hinting {
        log::info!("flash_start");
    } else if before.hinting && !after.hinting {
        let reason = match key {
            Key::Dismiss => "abort",
            _ => "end",
        };
        log::info!("flash_end reason={reason}");
    }
    if !before.searching && after.searching {
        log::info!("search_start");
    } else if before.searching && !after.searching {
        let reason = match key {
            Key::Dismiss => "abort",
            _ => "end",
        };
        log::info!("search_end reason={reason}");
    }
    if !before.picking && after.picking {
        log::info!("picker_start");
    } else if before.picking && !after.picking {
        let reason = match key {
            Key::Dismiss => "abort",
            _ => "end",
        };
        log::info!("picker_end reason={reason}");
    }
    if matches!(key, Key::ToggleHelp) {
        log::info!("help_toggle visible={}", board.help_visible);
    }
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
    // Mouse mode is what lets a click jump; 1003 adds motion so hover
    // can paint without a button down. Restore must undo both or the
    // shell keeps eating clicks after the board closes.
    execute!(out, EnterAlternateScreen, cursor::Hide, EnableMouseCapture)?;
    write!(out, "\x1b[?1003h")?;
    Terminal::new(PtyBackend { out })
}

fn restore(terminal: &mut HostTerminal) -> io::Result<()> {
    write!(terminal.backend_mut().out, "\x1b[?1003l")?;
    execute!(
        terminal.backend_mut(),
        cursor::Show,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
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
