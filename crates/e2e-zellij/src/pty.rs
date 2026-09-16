//! Attached PTY client for a real Zellij process.
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use nix::libc::{cfmakeraw, ioctl, winsize, TIOCSWINSZ};
use nix::pty::{openpty, OpenptyResult, Winsize};
use nix::unistd::dup;

/// Terminal size every scenario attaches with.
const ROWS: u16 = 32;
const COLS: u16 = 120;

pub struct PtyClient {
    child: Child,
    /// Kept open so the child retains a live PTY master; only the reader
    /// thread's clone is read from.
    _master: File,
    buffer: Arc<Mutex<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
}

impl PtyClient {
    pub fn spawn(argv: &[String], env: &[(String, String)]) -> io::Result<Self> {
        let winsize = Winsize {
            ws_row: ROWS,
            ws_col: COLS,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let OpenptyResult { master, slave } =
            openpty(Some(&winsize), None).map_err(io::Error::from)?;
        unsafe {
            let mut termios = std::mem::MaybeUninit::<nix::libc::termios>::uninit();
            if nix::libc::tcgetattr(slave.as_raw_fd(), termios.as_mut_ptr()) == 0 {
                let mut termios = termios.assume_init();
                cfmakeraw(&mut termios);
                let _ = nix::libc::tcsetattr(slave.as_raw_fd(), nix::libc::TCSANOW, &termios);
            }
            let size = winsize {
                ws_row: ROWS,
                ws_col: COLS,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let _ = ioctl(slave.as_raw_fd(), TIOCSWINSZ, &size);
        }
        let slave_fd = slave.into_raw_fd();
        let stdin = dup_stdio(slave_fd)?;
        let stdout = dup_stdio(slave_fd)?;
        let stderr = dup_stdio(slave_fd)?;
        unsafe {
            let _ = OwnedFd::from_raw_fd(slave_fd);
        }
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        command.stdin(stdin).stdout(stdout).stderr(stderr);
        for (key, value) in env {
            command.env(key, value);
        }
        command.env_remove("ZELLIJ");
        command.env_remove("ZELLIJ_SESSION_NAME");
        unsafe {
            command.pre_exec(|| {
                nix::unistd::setsid().map(|_| ()).map_err(io::Error::from)?;
                Ok(())
            });
        }
        let child = command.spawn()?;
        let master = File::from(master);
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let reader_buffer = Arc::clone(&buffer);
        let mut master_reader = master.try_clone()?;
        let reader = thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            loop {
                match master_reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => reader_buffer.lock().unwrap().extend_from_slice(&chunk[..n]),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            child,
            _master: master,
            buffer,
            reader: Some(reader),
        })
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn transcript(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).into_owned()
    }

    /// Substring probe for health checks that run on a repeating tick — the
    /// transcript only grows, so copying it each time is the expensive part.
    pub fn contains(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let bytes = needle.as_bytes();
        self.buffer
            .lock()
            .unwrap()
            .windows(bytes.len())
            .any(|window| window == bytes)
    }

    pub fn close(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for PtyClient {
    fn drop(&mut self) {
        self.close();
    }
}

fn dup_stdio(fd: i32) -> io::Result<Stdio> {
    let duplicated = dup(fd).map_err(io::Error::from)?;
    Ok(Stdio::from(unsafe { OwnedFd::from_raw_fd(duplicated) }))
}
