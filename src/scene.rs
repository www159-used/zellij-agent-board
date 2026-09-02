//! Host-side board scenes. Each input paints a real frame; checkpoints read it.

use crate::render::paint_to_size;
use crate::{Action, AgentId, Board, Key, PanePlace};

#[derive(Debug)]
pub struct SceneError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for SceneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for SceneError {}

/// Run a line-oriented scene. Empty / `#` lines are ignored.
pub fn run_scene(source: &str) -> Result<(), SceneError> {
    let mut run = Runner::new();
    for (index, raw) in source.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let verb = words.next().unwrap_or("");
        let args: Vec<&str> = words.collect();
        match verb {
            "size" => run.size(&args, line_no)?,
            "meta" | "scan" | "hook" | "started" | "seen" => {
                run.pending.push_str(&protocol_line(verb, &args, line_no)?);
                run.pending.push('\n');
            }
            "place" => run.place(&args, line_no)?,
            "key" => run.key(&args, line_no)?,
            "tick" => run.tick(&args, line_no)?,
            "expect" => run.expect(&args, line_no)?,
            other => {
                return Err(SceneError {
                    line: line_no,
                    message: format!("unknown {other}"),
                });
            }
        }
    }
    run.flush();
    Ok(())
}

struct Runner {
    board: Board,
    pending: String,
    last: Action,
    cols: u16,
    rows: u16,
    frame: Vec<String>,
}

impl Runner {
    fn new() -> Self {
        Self {
            board: Board::default(),
            pending: String::new(),
            last: Action::None,
            cols: 80,
            rows: 16,
            frame: Vec::new(),
        }
    }

    fn paint(&mut self) {
        self.board.set_list_geometry(self.cols, self.rows);
        self.board.set_page_len(visible_page(self.cols, self.rows));
        self.frame = paint_to_size(&self.board, "", self.rows, self.cols).texts();
    }

    fn flush(&mut self) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        let text = std::mem::take(&mut self.pending);
        let has_scan = text
            .lines()
            .any(|line| line.starts_with("SCAN ") || line.starts_with("META "));
        if has_scan || self.board.agents.is_empty() {
            self.board.ingest(&text);
        } else {
            self.board.ingest_notice(&text);
        }
        self.last = Action::None;
        self.paint();
        true
    }

    fn size(&mut self, args: &[&str], line: usize) -> Result<(), SceneError> {
        exact(args, 2, line, "size COLS ROWS")?;
        self.cols = parse_num(args.first(), line, "cols")?;
        self.rows = parse_num(args.get(1), line, "rows")?;
        self.flush();
        self.last = Action::None;
        self.paint();
        Ok(())
    }

    fn place(&mut self, args: &[&str], line: usize) -> Result<(), SceneError> {
        self.flush();
        if args.len() < 4 || args.len() > 5 {
            return Err(SceneError {
                line,
                message: "place SESSION PANE POS TAB [TITLE]".into(),
            });
        }
        apply_place(&mut self.board, args, line)?;
        self.last = Action::None;
        self.paint();
        Ok(())
    }

    fn key(&mut self, args: &[&str], line: usize) -> Result<(), SceneError> {
        self.flush();
        let key = parse_key(args, line)?;
        self.last = self.board.decide(key);
        self.paint();
        Ok(())
    }

    fn tick(&mut self, args: &[&str], line: usize) -> Result<(), SceneError> {
        exact(args, 0, line, "tick")?;
        self.flush();
        self.board.tick();
        self.last = Action::None;
        self.paint();
        Ok(())
    }

    fn expect(&mut self, args: &[&str], line: usize) -> Result<(), SceneError> {
        self.flush();
        let Some((kind, rest)) = args.split_first() else {
            return Err(SceneError {
                line,
                message: "expect needs selected/action/screen/status/hinting/searching".into(),
            });
        };
        match *kind {
            "selected" => {
                exact(rest, 2, line, "expect selected SESSION PANE")?;
                expect_selected(&self.board, rest, line)
            }
            "status" => {
                exact(rest, 3, line, "expect status SESSION PANE STATUS")?;
                expect_status(&self.board, rest, line)
            }
            "action" => expect_action(&self.last, rest, line),
            "screen" => expect_screen(&self.frame, rest, line),
            "hinting" => {
                exact(rest, 1, line, "expect hinting yes|no")?;
                expect_flag(self.board.is_hinting(), rest.first(), line, "hinting")
            }
            "searching" => {
                exact(rest, 1, line, "expect searching yes|no")?;
                expect_flag(self.board.is_searching(), rest.first(), line, "searching")
            }
            other => Err(SceneError {
                line,
                message: format!("unknown expect {other}"),
            }),
        }
    }
}

fn visible_page(width: u16, height: u16) -> usize {
    let per = if width >= 50 { 2 } else { 1 };
    let budget = usize::from(height.saturating_sub(4));
    (budget / per).max(1)
}

fn protocol_line(verb: &str, args: &[&str], line: usize) -> Result<String, SceneError> {
    match verb {
        "meta" => {
            let mut out = String::from("META");
            if args.is_empty() {
                out.push_str(" hooks=1");
            } else {
                for arg in args {
                    out.push(' ');
                    out.push_str(arg);
                }
            }
            Ok(out)
        }
        "scan" => {
            if args.len() < 2 || args.len() > 3 {
                return Err(SceneError {
                    line,
                    message: "scan SESSION PANE [WORKSPACE]".into(),
                });
            }
            let session = args[0];
            let pane = args[1];
            let workspace = args.get(2).copied().unwrap_or("");
            let argv = if workspace.is_empty() {
                "/Users/ww/.local/bin/agent".to_string()
            } else {
                format!("/Users/ww/.local/bin/agent --workspace {workspace}")
            };
            Ok(format!("SCAN {session} {pane} agent {argv}"))
        }
        "hook" => {
            if args.len() < 3 {
                return Err(SceneError {
                    line,
                    message: "hook SESSION PANE EVENT [detail…]".into(),
                });
            }
            Ok(format!(
                "HOOK {} {} {}",
                args[0],
                args[1],
                args[2..].join(" ")
            ))
        }
        "started" => {
            exact(args, 3, line, "started SESSION PANE EPOCH")?;
            Ok(format!("STARTED {} {} {}", args[0], args[1], args[2]))
        }
        "seen" => {
            exact(args, 3, line, "seen SESSION PANE FINISHED")?;
            Ok(format!("SEEN {} {} {}", args[0], args[1], args[2]))
        }
        _ => unreachable!("protocol_line verbs"),
    }
}

fn apply_place(board: &mut Board, args: &[&str], line: usize) -> Result<(), SceneError> {
    let session = need(args.first(), line, "session")?;
    let pane: u32 = parse_num(args.get(1), line, "pane")?;
    let tab_position: usize = parse_num(args.get(2), line, "tab position")?;
    let tab_name = need(args.get(3), line, "tab")?;
    let pane_title = args.get(4).copied().unwrap_or("");
    board.apply_places([(
        AgentId {
            session: (*session).to_string(),
            pane_id: pane,
        },
        PanePlace {
            tab_position,
            tab_name: (*tab_name).to_string(),
            pane_title: pane_title.to_string(),
        },
    )]);
    Ok(())
}

fn parse_key(args: &[&str], line: usize) -> Result<Key, SceneError> {
    match args.first().copied() {
        Some("enter") | Some("e") => {
            exact(args, 1, line, "key enter")?;
            Ok(Key::Confirm)
        }
        Some("esc") | Some("q") => {
            exact(args, 1, line, "key q")?;
            Ok(Key::Dismiss)
        }
        Some("s") => {
            exact(args, 1, line, "key s")?;
            Ok(Key::StartHint)
        }
        Some("/") => {
            exact(args, 1, line, "key /")?;
            Ok(Key::StartSearch)
        }
        Some("n") => {
            exact(args, 1, line, "key n")?;
            Ok(Key::NextMatch)
        }
        Some("N") => {
            exact(args, 1, line, "key N")?;
            Ok(Key::PrevMatch)
        }
        Some("g") => {
            exact(args, 1, line, "key g")?;
            Ok(Key::GPrefix)
        }
        Some("G") => {
            exact(args, 1, line, "key G")?;
            Ok(Key::Last)
        }
        Some("?") => {
            exact(args, 1, line, "key ?")?;
            Ok(Key::ToggleHelp)
        }
        Some("backspace") => {
            exact(args, 1, line, "key backspace")?;
            Ok(Key::Backspace)
        }
        Some("j") | Some("down") => {
            exact(args, 1, line, "key j")?;
            Ok(Key::Down)
        }
        Some("k") | Some("up") => {
            exact(args, 1, line, "key k")?;
            Ok(Key::Up)
        }
        Some("ctrl-d") => {
            exact(args, 1, line, "key ctrl-d")?;
            Ok(Key::HalfPageDown)
        }
        Some("ctrl-u") => {
            exact(args, 1, line, "key ctrl-u")?;
            Ok(Key::HalfPageUp)
        }
        Some("ctrl-f") | Some("pgdn") => {
            exact(args, 1, line, "key ctrl-f")?;
            Ok(Key::PageDown)
        }
        Some("ctrl-b") | Some("pgup") => {
            exact(args, 1, line, "key ctrl-b")?;
            Ok(Key::PageUp)
        }
        Some("input") => {
            exact(args, 2, line, "key input CHAR")?;
            let ch = args
                .get(1)
                .and_then(|word| word.chars().next())
                .ok_or_else(|| SceneError {
                    line,
                    message: "key input needs a character".into(),
                })?;
            Ok(Key::Input(ch))
        }
        Some(digit) if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() => {
            exact(args, 1, line, "key DIGIT")?;
            Ok(Key::Digit(digit.as_bytes()[0] - b'0'))
        }
        Some(other) => Err(SceneError {
            line,
            message: format!("unknown key {other}"),
        }),
        None => Err(SceneError {
            line,
            message: "key needs a name".into(),
        }),
    }
}

fn expect_selected(board: &Board, args: &[&str], line: usize) -> Result<(), SceneError> {
    let session = need(args.first(), line, "session")?;
    let pane: u32 = parse_num(args.get(1), line, "pane")?;
    let actual = board.agents.get(board.selected).ok_or_else(|| SceneError {
        line,
        message: "board has no rows".into(),
    })?;
    if actual.id.session != session || actual.id.pane_id != pane {
        return Err(SceneError {
            line,
            message: format!(
                "selected {} {} != {session} {pane}",
                actual.id.session, actual.id.pane_id
            ),
        });
    }
    Ok(())
}

fn expect_status(board: &Board, args: &[&str], line: usize) -> Result<(), SceneError> {
    let session = need(args.first(), line, "session")?;
    let pane: u32 = parse_num(args.get(1), line, "pane")?;
    let want = need(args.get(2), line, "status")?;
    let agent = board
        .agents
        .iter()
        .find(|agent| agent.id.session == *session && agent.id.pane_id == pane)
        .ok_or_else(|| SceneError {
            line,
            message: format!("no agent {session}:{pane}"),
        })?;
    if agent.status.label() != want {
        return Err(SceneError {
            line,
            message: format!("status {} != {want}", agent.status.label()),
        });
    }
    Ok(())
}

fn expect_action(actual: &Action, args: &[&str], line: usize) -> Result<(), SceneError> {
    let expected = match args.first().copied() {
        Some("none") => {
            exact(args, 1, line, "expect action none")?;
            Action::None
        }
        Some("dismiss") => {
            exact(args, 1, line, "expect action dismiss")?;
            Action::Dismiss
        }
        Some("jump") => {
            exact(args, 3, line, "expect action jump SESSION PANE")?;
            Action::Jump {
                session: need(args.get(1), line, "session")?.to_string(),
                pane_id: parse_num(args.get(2), line, "pane")?,
            }
        }
        Some(other) => {
            return Err(SceneError {
                line,
                message: format!("unknown action {other}"),
            });
        }
        None => {
            return Err(SceneError {
                line,
                message: "expect action none|dismiss|jump".into(),
            });
        }
    };
    if actual != &expected {
        return Err(SceneError {
            line,
            message: format!("action {actual:?} != {expected:?}"),
        });
    }
    Ok(())
}

fn expect_screen(frame: &[String], args: &[&str], line: usize) -> Result<(), SceneError> {
    let Some((op, rest)) = args.split_first() else {
        return Err(SceneError {
            line,
            message: "expect screen contains|excludes TEXT".into(),
        });
    };
    let needle = rest.join(" ");
    if needle.is_empty() {
        return Err(SceneError {
            line,
            message: "expect screen needs text".into(),
        });
    }
    let joined = frame.join("\n");
    match *op {
        "contains" => {
            if !joined.contains(&needle) {
                return Err(SceneError {
                    line,
                    message: format!("screen missing {needle:?}\n{joined}"),
                });
            }
        }
        "excludes" => {
            if joined.contains(&needle) {
                return Err(SceneError {
                    line,
                    message: format!("screen still has {needle:?}\n{joined}"),
                });
            }
        }
        other => {
            return Err(SceneError {
                line,
                message: format!("expect screen wants contains|excludes, got {other}"),
            });
        }
    }
    Ok(())
}

fn expect_flag(
    actual: bool,
    raw: Option<&&str>,
    line: usize,
    what: &str,
) -> Result<(), SceneError> {
    let expected = match raw.copied() {
        Some("yes") | Some("true") => true,
        Some("no") | Some("false") => false,
        Some(other) => {
            return Err(SceneError {
                line,
                message: format!("{what} wants yes/no, got {other}"),
            });
        }
        None => {
            return Err(SceneError {
                line,
                message: format!("{what} wants yes/no"),
            });
        }
    };
    if actual != expected {
        return Err(SceneError {
            line,
            message: format!("{what} {actual} != {expected}"),
        });
    }
    Ok(())
}

fn exact(args: &[&str], n: usize, line: usize, usage: &str) -> Result<(), SceneError> {
    if args.len() == n {
        Ok(())
    } else {
        Err(SceneError {
            line,
            message: usage.to_string(),
        })
    }
}

fn need<'a>(raw: Option<&&'a str>, line: usize, what: &str) -> Result<&'a str, SceneError> {
    raw.copied().ok_or_else(|| SceneError {
        line,
        message: format!("need {what}"),
    })
}

fn parse_num<T: std::str::FromStr>(
    raw: Option<&&str>,
    line: usize,
    what: &str,
) -> Result<T, SceneError> {
    raw.and_then(|word| word.parse().ok())
        .ok_or_else(|| SceneError {
            line,
            message: format!("need {what}"),
        })
}

#[cfg(test)]
mod tests {
    use super::run_scene;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn scenes_in_e2e_pass() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("e2e/scenes");
        let mut files: Vec<_> = fs::read_dir(&dir)
            .unwrap_or_else(|err| panic!("{}: {err}", dir.display()))
            .map(|entry| entry.expect("scene entry").path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("scene"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no .scene files in {}", dir.display());
        for path in files {
            let source = fs::read_to_string(&path).unwrap_or_else(|err| {
                panic!("{}: {err}", path.display());
            });
            if let Err(err) = run_scene(&source) {
                panic!("{}: {err}", path.display());
            }
        }
    }

    #[test]
    fn bare_expect_is_an_error() {
        let err = run_scene("scan ww 3 /tmp/ww\nexpect\n").expect_err("bare expect");
        assert!(err.message.contains("expect needs"), "{}", err.message);
    }

    #[test]
    fn unknown_expect_is_an_error() {
        let err = run_scene("scan ww 3 /tmp/ww\nexpect focussed ww 3\n").expect_err("typo");
        assert!(err.message.contains("unknown expect"), "{}", err.message);
    }
}
