//! Text that arrives from a file or from standard input instead of a command-line string:
//! `tb add … --desc-file PATH|-`, `tb edit ID --desc-file PATH|-`, `tb note ID --file PATH|-`.
//!
//! A shell eats backticks, `$`, quotes and backslashes in a long string; a file has no such
//! problem. The bytes are never re-interpreted here: they must be UTF-8 text, a leading
//! byte-order mark (an encoding signature, not text) is dropped, and everything else reaches
//! the store exactly as it was written. Control characters are NOT removed on the way in —
//! the store keeps text raw, as it does for `--desc`, and `text::sanitize*` still cleans every
//! screen path on the way out.
//!
//! `-` means standard input. It is refused when standard input is a terminal: agents have
//! none, and a command that waits for typing never returns.

use crate::store::BoardError;
use std::io::{IsTerminal, Read};
use std::path::Path;

/// The most text one file or pipe may carry: 256 KiB — twice the longest single argument
/// Linux accepts, so nothing `--desc "…"` could ever carry is refused here.
pub const MAX_TEXT_BYTES: usize = 256 * 1024;

/// The most one JSON document of cards may carry (`tb import`, `tb edit --from`): 4 MiB.
pub const MAX_DOC_BYTES: usize = 4 * 1024 * 1024;

/// What to do about too much text: one card's text, or a whole document of cards.
fn shorten(max: usize) -> &'static str {
    if max == MAX_TEXT_BYTES {
        "shorten it, or keep the long text in a file and name its path on the card"
    } else {
        "split it into smaller files and run them one after another"
    }
}

fn limit(max: usize) -> String {
    format!("{max} bytes ({} KiB)", max / 1024)
}

/// The text in `source` (a path, or `-` for standard input). `usage` is the command up to and
/// including the flag — `note 3 --file` — so every refusal can name the exact next command.
pub fn read(source: &Path, usage: &str) -> Result<String, BoardError> {
    read_up_to(source, usage, MAX_TEXT_BYTES)
}

/// `read` with another size limit (a document of many cards is bigger than one card's text).
/// Every other rule is the same: UTF-8, no NUL, not empty, a terminal on `-` is refused.
pub fn read_up_to(source: &Path, usage: &str, max: usize) -> Result<String, BoardError> {
    if source.as_os_str() == "-" {
        let stdin = std::io::stdin();
        let tty = stdin.is_terminal();
        return from_stdin(tty, &mut stdin.lock(), usage, max);
    }
    from_file(source, usage, max)
}

/// Standard input, unless it is a terminal (checked before a single byte is read).
fn from_stdin(is_terminal: bool, input: &mut dyn Read, usage: &str, max: usize) -> Result<String, BoardError> {
    if is_terminal {
        return Err(terminal_refusal(usage));
    }
    let mut bytes = Vec::new();
    // never an unbounded read: one byte past the limit is enough to know it is too much
    input
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| BoardError(format!("could not read standard input: {e} — try a file instead: 'tb {usage} PATH'")))?;
    if bytes.len() > max {
        return Err(BoardError(format!(
            "standard input is over the limit of {} for text from a file — {}",
            limit(max),
            shorten(max)
        )));
    }
    text_of(bytes, "standard input", usage)
}

fn terminal_refusal(usage: &str) -> BoardError {
    BoardError(format!(
        "'-' reads piped text, but standard input is a terminal — pipe or redirect it: 'tb {usage} - < FILE', or pass the path: 'tb {usage} FILE'"
    ))
}

fn from_file(path: &Path, usage: &str, max: usize) -> Result<String, BoardError> {
    let shown = format!("'{}'", path.display());
    let again = format!("'tb {usage} PATH', or pipe the text: 'tb {usage} -'");
    let refuse = |e: &std::io::Error| match e.kind() {
        std::io::ErrorKind::NotFound => {
            BoardError(format!("no file {shown} — check the path (it is relative to where tb runs): {again}"))
        }
        std::io::ErrorKind::PermissionDenied => {
            BoardError(format!("cannot read {shown}: permission denied — make it readable, then {again}"))
        }
        _ => BoardError(format!("cannot read {shown}: {e} — check the path: {again}")),
    };
    let mut file = std::fs::File::open(path).map_err(|e| refuse(&e))?;
    let meta = file.metadata().map_err(|e| refuse(&e))?;
    if meta.is_dir() {
        return Err(BoardError(format!("{shown} is a directory, not a text file — name a file: {again}")));
    }
    // a path that IS the terminal (/dev/tty, /dev/stdin in a shell) would wait for typing too
    if file.is_terminal() {
        return Err(terminal_refusal(usage));
    }
    let too_big = |size: String| {
        BoardError(format!(
            "{shown} is {size}; text from a file is limited to {} — {}",
            limit(max),
            shorten(max).replace("in a file", "in the file")
        ))
    };
    if meta.is_file() && meta.len() > max as u64 {
        return Err(too_big(format!("{} bytes", meta.len())));
    }
    // the size above can be wrong (a pipe, a device, a file still growing): read bounded anyway
    let mut bytes = Vec::new();
    (&mut file).take(max as u64 + 1).read_to_end(&mut bytes).map_err(|e| refuse(&e))?;
    if bytes.len() > max {
        return Err(too_big(format!("over {max} bytes")));
    }
    text_of(bytes, &shown, usage)
}

/// `bytes` as text: UTF-8, no NUL, not blank; a leading byte-order mark is dropped.
fn text_of(bytes: Vec<u8>, shown: &str, usage: &str) -> Result<String, BoardError> {
    if let Some(at) = bytes.iter().position(|b| *b == 0) {
        return Err(BoardError(format!(
            "{shown} holds a NUL byte (offset {at}), so it is not a text file — tb stores UTF-8 text; convert it first (e.g. iconv -f utf-16 -t utf-8), then 'tb {usage} PATH'"
        )));
    }
    let text = String::from_utf8(bytes).map_err(|e| {
        BoardError(format!(
            "{shown} is not UTF-8 text (bad byte at offset {}) — tb stores UTF-8 text; convert it first (e.g. iconv -f latin1 -t utf-8), then 'tb {usage} PATH'",
            e.utf8_error().valid_up_to()
        ))
    })?;
    let text = match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => text,
    };
    if text.trim().is_empty() {
        // an empty pipe is what a forgotten `<` or `|` looks like to an agent whose standard
        // input is /dev/null: never let that silently blank a description
        return Err(BoardError(format!(
            "{shown} is empty — nothing to store; check what writes it, then 'tb {usage} PATH' or 'tb {usage} -' with the text piped in"
        )));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader that must never be touched.
    struct Untouchable;
    impl Read for Untouchable {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("standard input was read although it is a terminal");
        }
    }

    #[test]
    fn a_terminal_on_stdin_is_refused_before_any_read() {
        let e = from_stdin(true, &mut Untouchable, "note 3 --file", MAX_TEXT_BYTES).unwrap_err().0;
        assert!(e.starts_with("'-' reads piped text, but standard input is a terminal — "), "{e}");
        assert!(e.contains("'tb note 3 --file - < FILE'") && e.contains("'tb note 3 --file FILE'"), "{e}");
    }

    #[test]
    fn piped_text_is_kept_byte_for_byte() {
        let nasty = "a `b` $c \"d\" 'e' \\f\tg\r\nh\n\n  i";
        let got = from_stdin(false, &mut nasty.as_bytes(), "note 3 --file", MAX_TEXT_BYTES).unwrap();
        assert_eq!(got, nasty);
    }

    #[test]
    fn the_limit_is_exact_and_the_read_is_bounded() {
        let at = vec![b'x'; MAX_TEXT_BYTES];
        assert_eq!(from_stdin(false, &mut at.as_slice(), "note 3 --file", MAX_TEXT_BYTES).unwrap().len(), MAX_TEXT_BYTES);
        // an endless producer: the read must stop on its own, one byte past the limit
        let e = from_stdin(false, &mut std::io::repeat(b'x'), "note 3 --file", MAX_TEXT_BYTES).unwrap_err().0;
        assert!(e.contains("over the limit of 262144 bytes (256 KiB)"), "{e}");
    }

    #[test]
    fn not_text_is_refused_and_a_byte_order_mark_is_dropped() {
        let e = text_of(vec![b'o', b'k', 0xff, b'!'], "'f'", "note 3 --file").unwrap_err().0;
        assert!(e.contains("not UTF-8 text (bad byte at offset 2)") && e.contains("iconv"), "{e}");
        let e = text_of(vec![b'a', 0, b'b'], "'f'", "note 3 --file").unwrap_err().0;
        assert!(e.contains("NUL byte (offset 1)"), "{e}");
        let e = text_of(b" \n\t\n".to_vec(), "'f'", "note 3 --file").unwrap_err().0;
        assert!(e.starts_with("'f' is empty — "), "{e}");
        assert_eq!(text_of("\u{feff}héllo\n".as_bytes().to_vec(), "'f'", "x").unwrap(), "héllo\n");
        // only a LEADING mark is an encoding signature; one inside the text is text
        assert_eq!(text_of("a\u{feff}b".as_bytes().to_vec(), "'f'", "x").unwrap(), "a\u{feff}b");
    }
}
