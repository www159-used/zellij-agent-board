use ratatui::{
    buffer::{Buffer, CellWidth},
    style::{Color, Modifier},
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct CellStyle {
    fg: Color,
    bg: Color,
    modifier: Modifier,
}

/// Snapshot encoder for tests. The host TUI does not use this — it draws
/// through `ratatui::Terminal`, which diffs cells and lets crossterm talk
/// to the PTY. There is no path from a Zellij pane to iTerm2's GPU.
pub(crate) fn encode_lines(buffer: &Buffer) -> Vec<String> {
    let mut lines = Vec::with_capacity(buffer.area.height as usize);
    for y in buffer.area.top()..buffer.area.bottom() {
        let mut line = String::new();
        let mut current_style = None;
        let mut skip_next = false;
        for x in buffer.area.left()..buffer.area.right() {
            if skip_next {
                skip_next = false;
                continue;
            }
            let cell = &buffer[(x, y)];
            let symbol = cell.symbol();
            let style = CellStyle {
                fg: cell.fg,
                bg: cell.bg,
                modifier: cell.modifier,
            };
            if current_style != Some(style) {
                line.push_str("\u{1b}[0m");
                push_style(&mut line, style);
                current_style = Some(style);
            }
            line.push_str(symbol);
            skip_next = symbol.cell_width() > 1;
        }
        line.push_str("\u{1b}[0m");
        lines.push(line);
    }
    lines
}

fn push_style(output: &mut String, style: CellStyle) {
    if style.fg == Color::Reset && style.bg == Color::Reset && style.modifier == Modifier::empty() {
        return;
    }
    if style.fg != Color::Reset {
        push_color(output, style.fg, false);
    }
    if style.bg != Color::Reset {
        push_color(output, style.bg, true);
    }
    if style.modifier.contains(Modifier::BOLD) {
        output.push_str("\u{1b}[1m");
    }
    if style.modifier.contains(Modifier::DIM) {
        output.push_str("\u{1b}[2m");
    }
}

fn push_color(output: &mut String, color: Color, background: bool) {
    let code = match (color, background) {
        (Color::Reset, false) => "39",
        (Color::Reset, true) => "49",
        (Color::Black, false) => "30",
        (Color::Red, false) => "31",
        (Color::Green, false) => "32",
        (Color::Yellow, false) => "33",
        (Color::Blue, false) => "34",
        (Color::Magenta, false) => "35",
        (Color::Cyan, false) => "36",
        (Color::Gray, false) => "37",
        (Color::DarkGray, false) => "90",
        (Color::LightRed, false) => "91",
        (Color::LightGreen, false) => "92",
        (Color::LightYellow, false) => "93",
        (Color::LightBlue, false) => "94",
        (Color::LightMagenta, false) => "95",
        (Color::LightCyan, false) => "96",
        (Color::White, false) => "97",
        (Color::Black, true) => "40",
        (Color::Red, true) => "41",
        (Color::Green, true) => "42",
        (Color::Yellow, true) => "43",
        (Color::Blue, true) => "44",
        (Color::Magenta, true) => "45",
        (Color::Cyan, true) => "46",
        (Color::Gray, true) => "47",
        (Color::DarkGray, true) => "100",
        (Color::LightRed, true) => "101",
        (Color::LightGreen, true) => "102",
        (Color::LightYellow, true) => "103",
        (Color::LightBlue, true) => "104",
        (Color::LightMagenta, true) => "105",
        (Color::LightCyan, true) => "106",
        (Color::White, true) => "107",
        (Color::Rgb(r, g, b), false) => {
            output.push_str(&format!("\u{1b}[38;2;{r};{g};{b}m"));
            return;
        }
        (Color::Rgb(r, g, b), true) => {
            output.push_str(&format!("\u{1b}[48;2;{r};{g};{b}m"));
            return;
        }
        (Color::Indexed(index), false) => {
            output.push_str(&format!("\u{1b}[38;5;{index}m"));
            return;
        }
        (Color::Indexed(index), true) => {
            output.push_str(&format!("\u{1b}[48;5;{index}m"));
            return;
        }
    };
    output.push_str("\u{1b}[");
    output.push_str(code);
    output.push('m');
}
