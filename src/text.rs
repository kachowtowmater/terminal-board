//! Displayed text is data: control characters and terminal escape sequences in it are removed
//! before anything reaches the terminal. The store keeps text raw; JSON output goes through the
//! same cleaner (`sanitize_json`), so piping `--json` output never pastes control bytes the
//! screen would have removed.
//!
//! One cleaner, three destinations. What is removed never changes: every control character and
//! every escape sequence, whole. What differs is which **whitespace** counts as content there —
//! `Keep`. A screen has fixed columns, so a tab (jump to the next tab stop) and a CR (back to
//! column one, over what is already drawn) are layout, not text, and become one space. JSON is
//! data for a parser, so a tab is just a tab: `serde_json` writes it as `\t` and the reader gets
//! U+0009 back, which is why `tb export --json` piped through `tb edit --from` returns a
//! description unchanged. A CR is folded there too — it is the one whitespace that moves the
//! cursor backwards, so a description holding `real text\rspoofed` would print as `spoofed`
//! in any log or pager that shows a decoded value.

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

/// Which whitespace a `Cleaner` treats as content and passes through as itself. Everything
/// else about a cleaner is the same whatever this is: control characters and escape sequences
/// are removed whole, and any whitespace NOT kept becomes a single space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Keep {
    /// One line on a screen: tab, CR and newline all become a space.
    #[default]
    Nothing,
    /// Many lines on a screen: a newline is a line break; a tab would jump to the next tab
    /// stop and a CR back over the line, so both still become a space.
    Newlines,
    /// Text as data, not as a rendering: newlines AND tabs are content and stay as themselves.
    /// CR does not — it is cursor motion, never text (see the module docs).
    NewlinesAndTabs,
}

impl Keep {
    fn newlines(self) -> bool {
        !matches!(self, Keep::Nothing)
    }
    fn tabs(self) -> bool {
        matches!(self, Keep::NewlinesAndTabs)
    }
}

/// Streaming sanitizer: feed chars one at a time, get back what may be shown.
/// C0 and C1 control characters and ESC-initiated sequences (CSI, OSC, DCS/SOS/PM/APC and
/// two-byte escapes) are dropped. Whitespace `keep` does not name becomes a space; a kept
/// newline stays a line break (and ends any unterminated sequence).
#[derive(Debug, Clone, Default)]
pub struct Cleaner {
    state: State,
    keep: Keep,
}

impl Cleaner {
    pub fn new(keep: Keep) -> Cleaner {
        Cleaner { state: State::Text, keep }
    }

    /// The char to show for `c`, if any.
    pub fn push(&mut self, c: char) -> Option<char> {
        let u = c as u32;
        if self.keep.newlines() && c == '\n' {
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
            // a tab outside a sequence, where `keep` says a tab is content
            '\t' if self.keep.tabs() => return Some('\t'),
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
    clean(s, Keep::Nothing)
}

/// Like `sanitize`, but line breaks are kept (multi-line output: descriptions, whole reports).
pub fn sanitize_lines(s: &str) -> String {
    clean(s, Keep::Newlines)
}

/// `s` inside a JSON string: the same cleaner the screen uses, with the same rules about what
/// is removed — escape sequences and control characters, whole — but tabs kept, because JSON
/// is data and a tab is text there (`Keep::NewlinesAndTabs`). Everything else is left for
/// serde to escape as usual.
///
/// JSON already escapes `"`/`\` and C0 controls; it passes DEL (U+007F) and C1 (U+0080–U+009F)
/// through raw. In UTF-8 those C1 bytes can act on a terminal (`U+009B` is a CSI), so an agent
/// that prints `--json` output would run them — the screen paths never do. Escape-sequence
/// state does not survive into `errors`/`warnings` entries: they carry whole texts, never
/// fragments that could split a sequence.
///
/// Keeping tabs is what makes `tb export --json` → `tb edit --from` give a description back
/// unchanged (`tests/export.rs::an_export_imports_straight_back`): a tab in a description is
/// text somebody wrote, and serde carries it as `\t`, never as a byte a terminal could act on.
pub fn sanitize_json(s: &str) -> String {
    clean(s, Keep::NewlinesAndTabs)
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

fn clean(s: &str, keep: Keep) -> String {
    if !s.chars().any(needs_cleaning) {
        return s.to_string();
    }
    let mut c = Cleaner::new(keep);
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
        let mut c = Cleaner::new(Keep::Nothing);
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
    fn json_output_is_cleaned_like_the_screen() {
        // DEL and every C1 go; emoji, CJK and accents stay. A stray C1 opener eats what
        // follows, exactly as on screen (`stray \u{85}\u{8d}c1` above shows the shape).
        assert_eq!(sanitize_json("del\x7f é✅審査"), "del é✅審査");
        // whole sequences removed, as on screen
        assert_eq!(sanitize_json("a\x1b[31mred\x1b[0mb"), "aredb");
        assert_eq!(sanitize_json("x\x1b]0;title\x07y"), "xy");
        // serde does the quoting; the cleaned text needs no escapes of its own
        let v = serde_json::to_string(&sanitize_json("quote\" back\\slash\n")).unwrap();
        assert_eq!(v, r#""quote\" back\\slash\n""#);
    }

    /// The ONE place the JSON view and the screen differ, pinned in both directions: a tab is
    /// text in JSON and a jump to the next tab stop on a screen. Everything else — what is
    /// removed, and CR — is identical, because it is the same cleaner.
    #[test]
    fn json_keeps_tabs_the_screen_folds_them() {
        // newline kept by both; tab kept ONLY by the JSON view; CR folded by both
        assert_eq!(sanitize_json("one\ntwo\tthree\rfour"), "one\ntwo\tthree four");
        assert_eq!(sanitize_lines("one\ntwo\tthree\rfour"), "one\ntwo three four");
        assert_eq!(sanitize("one\ntwo\tthree\rfour"), "one two three four");
        // a tab is the only difference: with none in the text the two agree exactly
        for s in ["plain", "a\x1b[31mred\x1b[0mb", "del\x7f é✅審査", "one\ntwo\rthree", "\u{9b}2Jgone"] {
            assert_eq!(sanitize_json(s), sanitize_lines(s), "no tab in {s:?}: the two views agree");
        }
        // a tab INSIDE a sequence is still part of the sequence and goes with it
        assert_eq!(sanitize_json("a\x1b]0;ti\ttle\x07b"), "ab");
        assert_eq!(sanitize_json("a\x1b[3\t1mb"), "a\t1mb", "a tab breaks a CSI, as any non-parameter byte does");
        // serde writes a kept tab as \t: no byte a terminal could act on leaves the process
        assert_eq!(serde_json::to_string(&sanitize_json("a\tb")).unwrap(), r#""a\tb""#);
    }

    /// The exact set, one code point at a time — the list docs/JSON.md states.
    #[test]
    fn json_strips_exactly_the_controls_and_keeps_the_text() {
        for u in (0x00..0x20u32).chain([0x7f]).chain(0x80..0xa0) {
            let c = char::from_u32(u).unwrap();
            let got = sanitize_json(&format!("A{c}B"));
            let want = match c {
                '\n' => "A\nB",
                '\t' => "A\tB",
                '\r' => "A B",
                // a sequence opener swallows what follows it
                '\x1b' | '\u{90}' | '\u{98}' | '\u{9b}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => "A",
                _ => "AB",
            };
            assert_eq!(got, want, "U+{u:04X}");
        }
        // nothing above U+009F is touched, including the whitespace that looks like a control
        for c in ['\u{a0}', '\u{2028}', '\u{2029}', '✅', '審', 'é'] {
            assert_eq!(sanitize_json(&format!("A{c}B")), format!("A{c}B"), "{c:?}");
        }
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
