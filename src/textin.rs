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

use crate::store::{BoardError, Code};
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
        if !tty {
            // #79: a pipe nobody ever closes (some agent harnesses, `ssh host tb …` without
            // `-n`) otherwise hangs here forever. Opt in only — see `stdin_timeout`.
            if let Some(timeout) = stdin_timeout() {
                wait_for_first_byte(timeout, usage)?;
            }
        }
        return from_stdin(tty, &mut stdin.lock(), usage, max);
    }
    from_file(source, usage, max)
}

/// `TB_STDIN_TIMEOUT`: whole seconds to wait for `-`'s FIRST byte before refusing. Unset or
/// `0` = wait forever, exactly today's behaviour — the default stays unbounded on purpose.
///
/// #79's decision: a hang is bad, but a wrong timeout that truncates a producer that is
/// merely slow to start is bad too (an LLM piping its output may buffer 60s before writing
/// anything), and tb cannot tell the two apart by looking at the pipe. Rather than guess a
/// default, this is opt-in: a harness that wants a bound (an unattended agent loop, where a
/// hang is the worse failure) sets it; nothing after the first byte is ever timed, so a slow
/// starter that does eventually write is never cut off once it has begun. Leniently parsed —
/// this only changes what tb WAITS for, never what it writes (docs/AGENTS.md's env var rule).
fn stdin_timeout() -> Option<std::time::Duration> {
    let secs: u64 = crate::env("STDIN_TIMEOUT")?.trim().parse().ok()?;
    (secs > 0).then_some(std::time::Duration::from_secs(secs))
}

/// Is `fd` readable (or at EOF/HUP) within `timeout`? `Ok(false)` is a real timeout —
/// nothing arrived and nothing closed. `Err` only for a genuine `poll` failure, which is not
/// evidence of a hang — the caller falls through to the real read and lets THAT fail on its
/// own terms if something is genuinely wrong. Never consumes a byte.
#[cfg(unix)]
fn poll_readable(fd: std::os::unix::io::RawFd, timeout: std::time::Duration) -> std::io::Result<bool> {
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    let ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    let r = unsafe { libc::poll(&mut pfd, 1, ms) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(r > 0)
}

/// Waits up to `timeout` for standard input to have a byte ready; refuses if nothing happens
/// in time.
#[cfg(unix)]
fn wait_for_first_byte(timeout: std::time::Duration, usage: &str) -> Result<(), BoardError> {
    use std::os::unix::io::AsRawFd;
    let fd = std::io::stdin().as_raw_fd();
    match poll_readable(fd, timeout) {
        Ok(true) | Err(_) => Ok(()),
        Ok(false) => Err(BoardError(format!(
            "'-' waited {}s for a first byte on standard input (TB_STDIN_TIMEOUT) and nothing arrived — \
             a pipe nobody writes to or closes would otherwise hang tb forever: check what is supposed \
             to feed it, or unset TB_STDIN_TIMEOUT to wait as long as it takes: 'tb {usage} -'",
            timeout.as_secs()
        ), Code::IoError)),
    }
}

/// No `poll` on this platform: `TB_STDIN_TIMEOUT` is a no-op, same as leaving it unset.
#[cfg(not(unix))]
fn wait_for_first_byte(_timeout: std::time::Duration, _usage: &str) -> Result<(), BoardError> {
    Ok(())
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
        .map_err(|e| BoardError(format!("could not read standard input: {e} — try a file instead: 'tb {usage} PATH'"), Code::IoError))?;
    if bytes.len() > max {
        return Err(BoardError(format!(
            "standard input is over the limit of {} for text from a file — {}",
            limit(max),
            shorten(max)
        ), Code::IoError));
    }
    text_of(bytes, "standard input", usage)
}

fn terminal_refusal(usage: &str) -> BoardError {
    BoardError(format!(
        "'-' reads piped text, but standard input is a terminal — pipe or redirect it: 'tb {usage} - < FILE', or pass the path: 'tb {usage} FILE'"
    ), Code::IoError)
}

fn from_file(path: &Path, usage: &str, max: usize) -> Result<String, BoardError> {
    let shown = format!("'{}'", path.display());
    let again = format!("'tb {usage} PATH', or pipe the text: 'tb {usage} -'");
    let refuse = |e: &std::io::Error| match e.kind() {
        std::io::ErrorKind::NotFound => {
            BoardError(format!("no file {shown} — check the path (it is relative to where tb runs): {again}"), Code::IoError)
        }
        std::io::ErrorKind::PermissionDenied => {
            BoardError(format!("cannot read {shown}: permission denied — make it readable, then {again}"), Code::IoError)
        }
        _ => BoardError(format!("cannot read {shown}: {e} — check the path: {again}"), Code::IoError),
    };
    // #79: `File::open` on a FIFO already does exactly the right thing by default — it waits
    // for a writer to attach, however long that takes, same as `-` waits for a producer. The
    // only gap is a FIFO nobody is EVER feeding, with no way to bound the wait; `open_fifo`
    // covers that the same way `-` is covered (TB_STDIN_TIMEOUT), never by refusing every
    // FIFO outright regardless of whether it has a writer.
    let mut file = if is_fifo(path) { open_fifo(path, &shown, usage, &refuse)? } else { std::fs::File::open(path).map_err(|e| refuse(&e))? };
    let meta = file.metadata().map_err(|e| refuse(&e))?;
    if meta.is_dir() {
        return Err(BoardError(format!("{shown} is a directory, not a text file — name a file: {again}"), Code::IoError));
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
        ), Code::IoError)
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

/// Is `path` a named pipe? `stat`, unlike `open`, never blocks on one — safe to check before
/// `File::open` would.
#[cfg(unix)]
fn is_fifo(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path).map(|m| m.file_type().is_fifo()).unwrap_or(false)
}

/// No FIFO concept worth guarding here: this platform's named pipes do not block `open` the
/// same way, and `File::open` below reports whatever really goes wrong.
#[cfg(not(unix))]
fn is_fifo(_path: &Path) -> bool {
    false
}

/// Opens a FIFO for reading. Unset `TB_STDIN_TIMEOUT`: a plain blocking `open()` — that
/// already has exactly the semantics wanted, waiting for a writer to attach however long that
/// takes (a writer that attaches a moment later still works, precisely as it always has), so
/// nothing special is needed and nothing here changes that default.
///
/// A first attempt at bounding this instead opened non-blocking and then switched the fd to
/// blocking mode before reading (SENT BACK: it does not work). A FIFO's read-open with
/// `O_NONBLOCK` always succeeds at once, whether or not a writer exists, which is exactly why
/// it cannot hang — but a `read()` on it, even after switching back to blocking mode, does
/// NOT wait for a writer that has not attached yet: with zero writers CURRENTLY attached it
/// returns EOF immediately, because from the kernel's point of view a read only blocks while
/// at least one writer already holds the pipe open. Whether a writer is attached is therefore
/// only knowable by actually trying to `open()` for real — there is no non-blocking substitute.
/// So a bounded wait instead puts the ordinary blocking `open()` on its own thread and waits
/// on a channel with a deadline: on timeout the thread is simply left blocked (harmless — it
/// is reclaimed when this process exits, which happens right after refusing).
#[cfg(unix)]
fn open_fifo(path: &Path, shown: &str, usage: &str, refuse: &dyn Fn(&std::io::Error) -> BoardError) -> Result<std::fs::File, BoardError> {
    let Some(timeout) = stdin_timeout() else {
        return std::fs::File::open(path).map_err(|e| refuse(&e));
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let owned = path.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(std::fs::File::open(&owned));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(file)) => Ok(file),
        Ok(Err(e)) => Err(refuse(&e)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(BoardError(format!(
            "{shown} waited {}s for a writer to open it (TB_STDIN_TIMEOUT) and none did — check what is supposed \
             to feed it, or unset TB_STDIN_TIMEOUT to wait as long as it takes: 'tb {usage} PATH'",
            timeout.as_secs()
        ), Code::IoError)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(BoardError(format!("cannot read {shown}: the thread opening it vanished without a trace — try again"), Code::IoError))
        }
    }
}

/// Unreachable on this platform (`is_fifo` above is always `false` here), kept only so
/// `from_file`'s call site type-checks on every target, including `x86_64-pc-windows-gnu`.
#[cfg(not(unix))]
fn open_fifo(path: &Path, _shown: &str, _usage: &str, refuse: &dyn Fn(&std::io::Error) -> BoardError) -> Result<std::fs::File, BoardError> {
    std::fs::File::open(path).map_err(|e| refuse(&e))
}

/// `bytes` as text: UTF-8, no NUL, not blank; a leading byte-order mark is dropped.
fn text_of(bytes: Vec<u8>, shown: &str, usage: &str) -> Result<String, BoardError> {
    if let Some(at) = bytes.iter().position(|b| *b == 0) {
        return Err(BoardError(format!(
            "{shown} holds a NUL byte (offset {at}), so it is not a text file — tb stores UTF-8 text; convert it first (e.g. iconv -f utf-16 -t utf-8), then 'tb {usage} PATH'"
        ), Code::IoError));
    }
    let text = String::from_utf8(bytes).map_err(|e| {
        BoardError(format!(
            "{shown} is not UTF-8 text (bad byte at offset {}) — tb stores UTF-8 text; convert it first (e.g. iconv -f latin1 -t utf-8), then 'tb {usage} PATH'",
            e.utf8_error().valid_up_to()
        ), Code::IoError)
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
        ), Code::IoError));
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

    /// #79: unset, empty or `0` never applies a bound (today's behaviour, unchanged); an
    /// unparsable value is lenient rather than refused, like tb's other read-only knobs.
    #[test]
    fn stdin_timeout_is_seconds_unset_or_zero_means_forever() {
        std::env::remove_var("TB_STDIN_TIMEOUT");
        assert_eq!(stdin_timeout(), None);
        std::env::set_var("TB_STDIN_TIMEOUT", "0");
        assert_eq!(stdin_timeout(), None);
        std::env::set_var("TB_STDIN_TIMEOUT", "5");
        assert_eq!(stdin_timeout(), Some(std::time::Duration::from_secs(5)));
        std::env::set_var("TB_STDIN_TIMEOUT", "not-a-number");
        assert_eq!(stdin_timeout(), None, "unparsable is lenient, not a refusal");
        std::env::remove_var("TB_STDIN_TIMEOUT");
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
