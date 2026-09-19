//! Displayed text is data: control characters and terminal escape sequences in it are removed
//! before anything reaches the terminal. The store keeps text raw; JSON output is unchanged.

/// Where a `Cleaner` is inside an escape sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Text,
    /// After ESC.
    Esc,
    /// After ESC and an intermediate byte (`ESC ( B`).
    EscInter,
    /// Control sequence (`ESC [` or C1 CSI): parameters until a final byte.
    Csi,
    /// String sequence (OSC, DCS, SOS, PM, APC): until BEL or ST.
    Str,
    /// ESC inside a string sequence (`ESC \` ends it).
    StrEsc,
}

/// Streaming sanitizer: feed chars one at a time, get back what may be displayed.
/// C0 and C1 control characters and ESC-initiated sequences (CSI, OSC, DCS/SOS/PM/APC and
/// two-byte escapes) are dropped. Tab, CR and newline become a space, or with `keep_newlines`
/// newline stays a line break (and ends any unterminated sequence).
#[derive(Debug, Clone, Default)]
pub struct Cleaner {
    state: State,
    keep_newlines: bool,
}

impl Cleaner {
    pub fn new(keep_newlines: bool) -> Cleaner {
        Cleaner { state: State::Text, keep_newlines }
    }

    /// The char to display for `c`, if any.
    pub fn push(&mut self, c: char) -> Option<char> {
        let u = c as u32;
        if self.keep_newlines && c == '\n' {
            self.state = State::Text;
            return Some('\n');
        }
        match self.state {
            State::Text => {}
            State::Esc => {
                self.state = match c {
                    '[' => State::Csi,
                    ']' | 'P' | 'X' | '^' | '_' => State::Str,
                    '\x1b' => State::Esc,
                    ' '..='/' => State::EscInter,
                    _ if (0x30..=0x7e).contains(&u) => State::Text,
                    _ => {
                        self.state = State::Text;
                        return self.push(c);
                    }
                };
                return None;
            }
            State::EscInter => {
                if (0x20..=0x2f).contains(&u) {
                    return None;
                }
                self.state = State::Text;
                if (0x30..=0x7e).contains(&u) {
                    return None;
                }
                return self.push(c);
            }
            State::Csi => {
                if (0x20..=0x3f).contains(&u) {
                    return None;
                }
                self.state = State::Text;
                if (0x40..=0x7e).contains(&u) {
                    return None;
                }
                return self.push(c);
            }
            State::Str => {
                match c {
                    '\x07' | '\u{9c}' => self.state = State::Text,
                    '\x1b' => self.state = State::StrEsc,
                    _ => {}
                }
                return None;
            }
            State::StrEsc => {
                self.state = if c == '\\' { State::Text } else { State::Str };
                return None;
            }
        }
        match c {
            '\x1b' => self.state = State::Esc,
            '\u{9b}' => self.state = State::Csi,
            '\u{9d}' | '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => self.state = State::Str,
            '\t' | '\n' | '\r' => return Some(' '),
            _ if u < 0x20 || (0x7f..=0x9f).contains(&u) => {}
            _ => return Some(c),
        }
        None
    }
}

/// `s` safe to show on one line: no control characters or escape sequences; tabs and line
/// breaks become spaces.
pub fn sanitize(s: &str) -> String {
    clean(s, false)
}

/// Like `sanitize`, but line breaks are kept (multi-line output: descriptions, whole reports).
pub fn sanitize_lines(s: &str) -> String {
    clean(s, true)
}

/// Append `s` as one output line (leading blank lines kept): stored or remote text inside it
/// can never start a line of its own.
pub fn push_line(out: &mut String, s: &str) {
    let body = s.trim_start_matches('\n');
    for _ in 0..s.len() - body.len() {
        out.push('\n');
    }
    out.push_str(&sanitize(body));
    out.push('\n');
}

fn clean(s: &str, keep_newlines: bool) -> String {
    if !s.chars().any(needs_cleaning) {
        return s.to_string();
    }
    let mut c = Cleaner::new(keep_newlines);
    s.chars().filter_map(|ch| c.push(ch)).collect()
}

fn needs_cleaning(c: char) -> bool {
    let u = c as u32;
    u < 0x20 || (0x7f..=0x9f).contains(&u)
}

/// Clean a rendered frame cell by cell, one row at a time, so a sequence split over several
/// cells is removed whole. Cells that held only sequence bytes become blanks: the layout stays.
pub fn sanitize_buffer(buf: &mut ratatui::buffer::Buffer) {
    let w = buf.area.width as usize;
    if w == 0 {
        return;
    }
    for row in buf.content.chunks_mut(w) {
        if !row.iter().any(|cell| cell.symbol().chars().any(needs_cleaning)) {
            continue;
        }
        let mut c = Cleaner::new(false);
        for cell in row.iter_mut() {
            let sym = cell.symbol();
            let kept: String = sym.chars().filter_map(|ch| c.push(ch)).collect();
            if kept != sym {
                let kept = if kept.is_empty() && !sym.is_empty() { " ".to_string() } else { kept };
                cell.set_symbol(&kept);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_sequences_and_controls() {
        assert_eq!(sanitize("plain text · ünïcode"), "plain text · ünïcode");
        assert_eq!(sanitize("a\x1b[31mred\x1b[0m b"), "ared b");
        assert_eq!(sanitize("x\x1b]0;title\x07y"), "xy");
        assert_eq!(sanitize("x\x1b]8;;http://e\x1b\\link\x1b]8;;\x1b\\y"), "xlinky");
        assert_eq!(sanitize("clear\x1b[2J\x1b[Hdone"), "cleardone");
        assert_eq!(sanitize("c1\u{9b}31mcsi\u{9d}0;t\u{9c}end"), "c1csiend");
        assert_eq!(sanitize("stray \u{85}\u{8d}c1"), "stray c1");
        assert_eq!(sanitize("\x1b(Bcharset\x1bcreset\x1b7save"), "charsetresetsave");
        assert_eq!(sanitize("bell\x07 bs\x08 nul\0 del\x7f"), "bell bs nul del");
        assert_eq!(sanitize("tab\there\nline\r"), "tab here line ");
        assert_eq!(sanitize_lines("one\ntwo\x1b[1m\nthree\t."), "one\ntwo\nthree .");
        // an unterminated sequence swallows the rest, never shows it; a newline ends it
        assert_eq!(sanitize("ok\x1b]0;never ends"), "ok");
        assert_eq!(sanitize_lines("ok\x1b]0;cut\nnext"), "ok\nnext");
        // a CSI broken by a non-sequence char gives the char back
        assert_eq!(sanitize("a\x1b[12é"), "aé");
    }

    #[test]
    fn buffer_rows_lose_whole_sequences() {
        use ratatui::{backend::TestBackend, text::Line, widgets::Paragraph, Terminal};
        let mut t = Terminal::new(TestBackend::new(24, 2)).unwrap();
        t.draw(|f| {
            f.render_widget(Paragraph::new(vec![Line::from("A\x1b[31mB\u{9b}2JC"), Line::from("\x1b]0;x\x07D\tE")]), f.area());
            sanitize_buffer(f.buffer_mut());
        })
        .unwrap();
        let b = t.backend().buffer();
        let rows: Vec<String> =
            b.content.chunks(24).map(|r| r.iter().map(|c| c.symbol()).collect::<String>().trim_end().to_string()).collect();
        assert_eq!(rows, ["A     B   C", "      D E"]);
    }
}
