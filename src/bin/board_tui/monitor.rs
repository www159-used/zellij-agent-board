//! A private PTY isolates bottom's terminal/event loop from the host navigation.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Paragraph, Widget},
};
use std::{
    io::{self, Read, Write},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
};
use tui_term::{
    vt100::{self, MouseProtocolEncoding, MouseProtocolMode},
    widget::PseudoTerminal,
};

const CHILD_ENV: &str = "ZAB_INTERNAL_BOTTOM";
pub fn is_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}
pub fn run_child() -> io::Result<()> {
    let mut cleanup = false;
    bottom::start_bottom(&mut cleanup).map_err(|err| {
        if cleanup {
            bottom::reset_stdout();
        }
        io::Error::other(err.to_string())
    })
}

#[derive(Default)]
struct Replies(Vec<u8>);
impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if c == 'n' && i1.is_none() && params.first().and_then(|p| p.first()) == Some(&6) {
            let (row, col) = screen.cursor_position();
            self.0
                .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
        } else if c == 'c' {
            self.0.extend_from_slice(b"\x1b[?1;2c");
        }
    }
}

pub struct Monitor {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    parser: vt100::Parser<Replies>,
    output: Option<Receiver<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    pub error: Option<String>,
}
impl Monitor {
    pub fn spawn(area: Rect) -> io::Result<Self> {
        let size = pty_size(area);
        let pair = native_pty_system()
            .openpty(size)
            .map_err(io::Error::other)?;
        let mut input = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let writer = pair.master.take_writer().map_err(io::Error::other)?;
        let mut command = CommandBuilder::new(std::env::current_exe()?);
        command.env(CHILD_ENV, "1");
        if let Some(config) = std::env::var_os("ZAB_BOTTOM_CONFIG") {
            command.arg("--config");
            command.arg(config);
        }
        command.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(io::Error::other)?;
        drop(pair.slave);
        let (sender, output) = mpsc::sync_channel(32);
        let reader = thread::spawn(move || {
            let mut bytes = [0; 8192];
            loop {
                match input.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(len) => {
                        if sender.send(bytes[..len].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self {
            master: pair.master,
            child,
            writer,
            parser: vt100::Parser::new_with_callbacks(size.rows, size.cols, 0, Replies::default()),
            output: Some(output),
            reader: Some(reader),
            error: None,
        })
    }
    pub fn poll(&mut self) -> bool {
        let mut dirty = false;
        if let Some(output) = &self.output {
            while let Ok(bytes) = output.try_recv() {
                self.parser.process(&bytes);
                dirty = true;
            }
        }
        let replies = std::mem::take(&mut self.parser.callbacks_mut().0);
        if !replies.is_empty() {
            self.write(&replies);
        }
        if self.error.is_none() {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.error = Some(format!(
                        "bottom {status}. Press t to restart or h for Agents."
                    ));
                    dirty = true;
                }
                Err(err) => {
                    self.error = Some(err.to_string());
                    dirty = true;
                }
                Ok(None) => {}
            }
        }
        dirty
    }
    pub fn resize(&mut self, area: Rect) -> io::Result<()> {
        let size = pty_size(area);
        if self.parser.screen().size() != (size.rows, size.cols) {
            self.master.resize(size).map_err(io::Error::other)?;
            self.parser.screen_mut().set_size(size.rows, size.cols);
        }
        Ok(())
    }
    pub fn render(&self, area: Rect, buffer: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        PseudoTerminal::new(self.parser.screen()).render(area, buffer);
        if let Some(error) = &self.error {
            Paragraph::new(error.as_str())
                .render(Rect::new(area.x, area.bottom() - 1, area.width, 1), buffer);
        }
    }
    fn write(&mut self, bytes: &[u8]) {
        if self.error.is_none() {
            if let Err(err) = self
                .writer
                .write_all(bytes)
                .and_then(|()| self.writer.flush())
            {
                self.error = Some(err.to_string());
            }
        }
    }
    pub fn key(&mut self, event: KeyEvent) {
        self.write(&key_bytes(event, self.parser.screen().application_cursor()));
    }
    pub fn paste(&mut self, text: &str) {
        let text = if self.parser.screen().bracketed_paste() {
            format!("\x1b[200~{text}\x1b[201~")
        } else {
            text.to_owned()
        };
        self.write(text.as_bytes());
    }
    pub fn mouse(&mut self, event: MouseEvent) {
        let screen = self.parser.screen();
        let mode = screen.mouse_protocol_mode();
        if mode == MouseProtocolMode::None {
            return;
        }
        let button = |b| match b {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        };
        let (mut code, release) = match event.kind {
            MouseEventKind::Down(b) => (button(b), false),
            MouseEventKind::Up(b) if mode != MouseProtocolMode::Press => (button(b), true),
            MouseEventKind::Drag(b)
                if matches!(
                    mode,
                    MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion
                ) =>
            {
                (button(b) + 32, false)
            }
            MouseEventKind::Moved if mode == MouseProtocolMode::AnyMotion => (35, false),
            MouseEventKind::ScrollUp => (64, false),
            MouseEventKind::ScrollDown => (65, false),
            MouseEventKind::ScrollLeft => (66, false),
            MouseEventKind::ScrollRight => (67, false),
            _ => return,
        };
        if event.modifiers.contains(KeyModifiers::SHIFT) {
            code += 4;
        }
        if event.modifiers.contains(KeyModifiers::ALT) {
            code += 8;
        }
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            code += 16;
        }
        let x = u32::from(event.column) + 1;
        let y = u32::from(event.row) + 1;
        let bytes = match screen.mouse_protocol_encoding() {
            MouseProtocolEncoding::Sgr => {
                format!("\x1b[<{code};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes()
            }
            encoding => {
                if release {
                    code = 3 + (code & 28);
                }
                let mut bytes = b"\x1b[M".to_vec();
                for value in [code + 32, x + 32, y + 32] {
                    if encoding == MouseProtocolEncoding::Utf8 {
                        let Some(c) = char::from_u32(value) else {
                            return;
                        };
                        bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                    } else if let Ok(value) = u8::try_from(value) {
                        bytes.push(value);
                    } else {
                        return;
                    }
                }
                bytes
            }
        };
        self.write(&bytes);
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        self.output.take(); // Unblock a reader waiting on the bounded channel.
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
fn pty_size(area: Rect) -> PtySize {
    PtySize {
        rows: area.height.max(1),
        cols: area.width.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}
fn key_bytes(event: KeyEvent, application: bool) -> Vec<u8> {
    let modifiers = event.modifiers;
    let number = 1
        + u8::from(modifiers.contains(KeyModifiers::SHIFT))
        + 2 * u8::from(modifiers.contains(KeyModifiers::ALT))
        + 4 * u8::from(modifiers.contains(KeyModifiers::CONTROL));
    let csi = |final_char: char| {
        if number > 1 {
            format!("\x1b[1;{number}{final_char}")
        } else {
            format!("\x1b{}{final_char}", if application { 'O' } else { '[' })
        }
    };
    let tilde = |n| {
        if number > 1 {
            format!("\x1b[{n};{number}~")
        } else {
            format!("\x1b[{n}~")
        }
    };
    let mut bytes = match event.code {
        KeyCode::Char(c) => {
            if modifiers.contains(KeyModifiers::CONTROL)
                && (c.is_ascii_alphabetic() || ('@'..='_').contains(&c) || c == ' ')
            {
                vec![(c.to_ascii_uppercase() as u8) & 31]
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![127],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![27],
        KeyCode::Up => csi('A').into_bytes(),
        KeyCode::Down => csi('B').into_bytes(),
        KeyCode::Right => csi('C').into_bytes(),
        KeyCode::Left => csi('D').into_bytes(),
        KeyCode::Home => csi('H').into_bytes(),
        KeyCode::End => csi('F').into_bytes(),
        KeyCode::Insert => tilde(2).into_bytes(),
        KeyCode::Delete => tilde(3).into_bytes(),
        KeyCode::PageUp => tilde(5).into_bytes(),
        KeyCode::PageDown => tilde(6).into_bytes(),
        KeyCode::F(n @ 1..=4) => {
            if number > 1 {
                csi(char::from(b'P' + n - 1)).into_bytes()
            } else {
                format!("\x1bO{}", char::from(b'P' + n - 1)).into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => {
            tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]).into_bytes()
        }
        _ => vec![],
    };
    if modifiers.contains(KeyModifiers::ALT)
        && matches!(
            event.code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace
        )
    {
        bytes.insert(0, 27);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_keys_preserve_control_unicode_and_application_modes() {
        let key = |code, mods| KeyEvent::new(code, mods);
        assert_eq!(
            key_bytes(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            b"\x03"
        );
        assert_eq!(
            key_bytes(key(KeyCode::Char('中'), KeyModifiers::NONE), false),
            "中".as_bytes()
        );
        assert_eq!(
            key_bytes(key(KeyCode::Up, KeyModifiers::NONE), true),
            b"\x1bOA"
        );
        assert_eq!(
            key_bytes(key(KeyCode::Left, KeyModifiers::CONTROL), true),
            b"\x1b[1;5D"
        );
    }
    #[test]
    fn cursor_queries_are_answered_inside_the_private_terminal() {
        let mut parser = vt100::Parser::new_with_callbacks(10, 80, 0, Replies::default());
        parser.process(b"\x1b[4;7H\x1b[6n");
        assert_eq!(parser.callbacks_mut().0, b"\x1b[4;7R");
    }
}
