//! The `TB_NOW` clock override (legacy `TTYBOARD_NOW`): an in-range value pins the clock,
//! anything else — not an integer, or outside 946684800–4102444800 (2000, the last second it accepts is 4102444799) — is refused
//! before anything is written, in plain mode and under `--json`.
//!
//! The suite pins the clock by setting `TB_NOW` in the environment of the spawned binary
//! (`tests/common::pin_clock`), so the check lives in the release binary: no cfg gate.
use std::process::{Command, Output};

const MIN: &str = "946684800"; // 2000-01-01T00:00:00Z — keep in step with store::TB_NOW_MIN
const MAX: &str = "4102444800"; // the first refused second, one past the window — keep in step with store::TB_NOW_MAX
const LAST: &str = "4102444799"; // the last second the window accepts — MAX - 1, the other edge

struct Board {
    dir: tempfile::TempDir,
    db: std::path::PathBuf,
}

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("board.db");
        Board { dir, db }
    }

    fn run(&self, now: Option<&str>, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args)
            .env("TB_DB", &self.db)
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .env_remove("TB_NOW")
            .env_remove("TTYBOARD_NOW");
        match now {
            Some(v) => c.env("TB_NOW", v),
            None => &mut c,
        }
        .output()
        .unwrap()
    }

    /// The board as `id|column|created_at` per card, for before/after comparisons.
    fn dump(&self) -> String {
        if !self.db.exists() {
            return String::new();
        }
        let s = terminal_board::store::Store::open(&self.db).unwrap();
        s.list()
            .unwrap()
            .iter()
            .map(|c| format!("{}|{}|{}", c.id, c.column, c.created_at))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn card_count(&self) -> usize {
        if !self.db.exists() {
            return 0;
        }
        terminal_board::store::Store::open(&self.db).unwrap().list().unwrap().len()
    }
}

#[test]
fn an_in_range_pin_is_used() {
    let b = Board::new();
    let out = String::from_utf8(b.run(Some("1789777000"), &["add", "widgets: gh#7 fix it", "--json"]).stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["card"]["created_at"], 1_789_777_000);
    // the same pin in a second command reads back as fact (show --json is the card itself)
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8(b.run(Some("1789777000"), &["show", "1", "--json"]).stdout).unwrap()).unwrap();
    assert_eq!(v["created_at"], 1_789_777_000);
    // BOTH edges of the window, pinned against the strings tb ships: MIN and LAST are the
    // first and last seconds it accepts, and MAX — the one every message calls "one past the
    // last accepted" — is refused. (The upper bound is exclusive; see store::TB_NOW_MAX.)
    for edge in [MIN, LAST] {
        let e = Board::new();
        let o = e.run(Some(edge), &["add", "edge"]);
        assert!(o.status.success(), "edge {edge} refused: {}", String::from_utf8_lossy(&o.stderr));
        let s = terminal_board::store::Store::open(&e.db).unwrap();
        assert_eq!(s.list().unwrap()[0].created_at, edge.parse::<i64>().unwrap(), "edge {edge} pinned");
    }
    // the legacy name still pins
    let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
    c.args(["add", "legacy"])
        .env("TB_DB", &b.db)
        .env("TB_AS", "tester")
        .env("TB_NO_HERDR", "1")
        .env_remove("TB_NOW")
        .env("TTYBOARD_NOW", "1789777000")
        .output()
        .unwrap();
    let s = terminal_board::store::Store::open(&b.db).unwrap();
    assert_eq!(s.list().unwrap().iter().map(|c| c.created_at).min(), Some(1_789_777_000), "legacy TTYBOARD_NOW pins too");
}

#[test]
fn a_bad_pin_is_refused_and_writes_nothing() {
    let b = Board::new();
    seed_one(&b, None);
    let before = b.dump();
    let count = b.card_count();
    // one of every refusal class: negative, zero, past the window, absurdly large,
    // not a number, and surrounded by blank space (never silently trimmed into a pin).
    // ("", the empty pin, is the real-clock case — the next test.)
    // `MAX` is in the list on purpose: every shipped string (the refusal's own hint,
    // README.md, docs/JSON.md, docs/AGENTS.md, CHANGELOG.md) calls it "one past the last
    // accepted", so the window must be exclusive at the top. `LAST` (MAX - 1) is the last
    // value accepted, pinned by `an_in_range_pin_is_used`.
    for bad in ["0", "-1", "9223372036854775807", "1e9999", "abc", " 12 ", "946684799", MAX, "4102444801"] {
        let o = b.run(Some(bad), &["add", "must not appear", "--as", "tester"]);
        assert!(!o.status.success(), "TB_NOW={bad:?} was accepted: {}", String::from_utf8_lossy(&o.stdout));
        let err = String::from_utf8_lossy(&o.stderr).to_string();
        assert_eq!(err.trim().lines().count(), 1, "one line: {err}");
        assert!(err.contains("TB_NOW"), "names the variable: {err}");
        assert!(err.contains(MIN) && err.contains(MAX), "names the range: {err}");
        assert_eq!(b.card_count(), count, "TB_NOW={bad:?} wrote nothing");
        assert_eq!(b.dump(), before, "TB_NOW={bad:?} changed the board");
    }
}

#[test]
fn an_empty_or_unset_pin_is_the_real_clock() {
    let b = Board::new();
    let o = b.run(None, &["add", "real clock"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let s = terminal_board::store::Store::open(&b.db).unwrap();
    let t = s.list().unwrap()[0].created_at;
    let real = chrono::Utc::now().timestamp();
    assert!((t - real).abs() < 300, "created_at {t} is not the real clock {real}");
    // empty means the same
    let e = Board::new();
    let o = e.run(Some(""), &["add", "real clock too"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let s = terminal_board::store::Store::open(&e.db).unwrap();
    let t = s.list().unwrap()[0].created_at;
    assert!((t - chrono::Utc::now().timestamp()).abs() < 300, "empty pin must be the real clock, got {t}");
}

#[test]
fn the_refusal_is_documented_json() {
    let b = Board::new();
    seed_one(&b, None);
    let before = b.dump();
    let o = b.run(Some("abc"), &["add", "never", "--json"]);
    assert!(!o.status.success());
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("stdout is not JSON: {e}: {out}"));
    assert_eq!(v["ok"], false);
    let err = v["error"].as_str().unwrap_or_else(|| panic!("{v}"));
    assert!(err.contains("TB_NOW") && err.contains("abc"), "error names the variable and value: {v}");
    let hint = v["hint"].as_str().unwrap_or_else(|| panic!("{v}"));
    assert!(hint.contains(MIN) && hint.contains(MAX), "hint carries the range: {v}");
    // --json goes to stdout, nothing on stderr, and the board is untouched
    assert!(String::from_utf8_lossy(&o.stderr).trim().is_empty(), "no stderr under --json: {:?}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(b.dump(), before);
}

/// The refusal is ahead of EVERY command, including the ones that return before the command
/// match in `main`'s `run()`: `tb setup`, and the bulk `import` / `edit --from` reader. Each
/// of those reaches a write, so a gate placed after them refuses nothing that matters.
#[test]
fn the_early_return_paths_are_refused_too() {
    let b = Board::new();
    seed_one(&b, Some("1789777000"));
    let before = b.dump();

    // `tb setup` returns from run() long before the command match
    let o = b.run(Some("0"), &["setup", "--dry-run", "--yes"]);
    assert!(!o.status.success(), "setup accepted TB_NOW=0: {}", String::from_utf8_lossy(&o.stdout));
    let err = String::from_utf8_lossy(&o.stderr).to_string();
    assert!(err.contains("TB_NOW") && err.contains(MIN), "setup: names the variable and the range: {err}");
    assert_eq!(b.dump(), before, "setup changed the board");

    // the bulk path (`tb import FILE`) returns before it too — and it writes cards
    let file = b.dir.path().join("cards.json");
    std::fs::write(&file, r#"[{"title": "docs: from a file"}]"#).unwrap();
    let path = file.to_str().unwrap();
    let o = b.run(Some("0"), &["import", path]);
    assert!(!o.status.success(), "import accepted TB_NOW=0: {}", String::from_utf8_lossy(&o.stdout));
    let err = String::from_utf8_lossy(&o.stderr).to_string();
    assert!(err.contains("TB_NOW") && err.contains(MIN), "import: names the variable and the range: {err}");
    assert_eq!(b.dump(), before, "import wrote with TB_NOW=0");

    // and that same file really does write, so the refusal above is what stopped it
    let o = b.run(Some("1789777001"), &["import", path]);
    assert!(o.status.success(), "import with a good pin failed: {}", String::from_utf8_lossy(&o.stderr));
    assert_ne!(b.dump(), before, "the import path writes when the pin is in range");
}

/// The product's primary UI is the full-screen board, which `main`'s `run()` reaches by its
/// own early return — only when stdout is a terminal, so this drives the real binary through
/// a pty. A gate placed after that return refuses nothing here: the board opens and every
/// move writes `column_since` and an event at the absurd timestamp.
#[cfg(unix)]
#[test]
fn the_full_screen_board_is_refused_too() {
    use std::io::{Read, Write};
    use std::os::unix::io::{AsRawFd, FromRawFd};

    let b = Board::new();
    seed_one(&b, Some("1789777000"));
    let before = b.dump();

    let (master, slave_path) = pty::open();
    let slave = std::fs::OpenOptions::new().read(true).write(true).open(&slave_path).unwrap();
    let sfd = slave.as_raw_fd();
    let dup = |fd: i32| unsafe { std::process::Stdio::from_raw_fd(libc::dup(fd)) };
    let mut child = Command::new(env!("CARGO_BIN_EXE_tb"))
        .env("TB_DB", &b.db)
        .env("TB_AS", "tester")
        .env("TB_NO_HERDR", "1")
        .env("TB_NOW", "0")
        .env("TERM", "xterm-256color")
        .env_remove("TTYBOARD_NOW")
        .stdin(dup(sfd))
        .stdout(dup(sfd))
        .stderr(dup(sfd))
        .spawn()
        .unwrap();
    drop(slave); // only the child holds the slave now, so the master reads EOF when it exits

    // if the board DID open, these move the focused card twice and quit — the exact keys that
    // wrote column_since=0 and two events at ts=0 before the gate moved
    let mut m = unsafe { std::fs::File::from_raw_fd(master) };
    let _ = m.write_all(b"\x1b[1;2C\x1b[1;2Cq\r");
    let _ = m.flush();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        while let Ok(n) = m.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&out).to_string()
    });

    // never hang the suite on a board that opened and is waiting for a key
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        match child.try_wait().unwrap() {
            Some(s) => break Some(s),
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    };
    let out = reader.join().unwrap();
    let status = status.unwrap_or_else(|| panic!("the full-screen board did not exit in 30s with TB_NOW=0:\n{out}"));

    assert!(!status.success(), "the full-screen board accepted TB_NOW=0 (exit {status:?}):\n{out}");
    assert!(out.contains("not a plausible unix second"), "the refusal is not on the terminal:\n{out}");
    assert!(out.contains("TB_NOW") && out.contains(MIN), "the refusal names the variable and the range:\n{out}");
    assert_eq!(b.dump(), before, "the full-screen board wrote with TB_NOW=0");
    // nothing was moved and no event was stamped at 0
    let s = terminal_board::store::Store::open(&b.db).unwrap();
    for c in s.list().unwrap() {
        assert_ne!(c.column_since, 0, "card #{} kept column_since=0", c.id);
        assert_eq!(c.column, "todo", "card #{} was moved by the board that should not have opened", c.id);
    }
}

/// A pty, so tb believes stdout is a terminal and takes its full-screen branch. `posix_openpt`
/// and friends are in libc itself on both Linux and macOS: no extra crate, no `script(1)`.
#[cfg(unix)]
mod pty {
    use std::path::PathBuf;

    pub fn open() -> (i32, PathBuf) {
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(master >= 0, "posix_openpt: {}", std::io::Error::last_os_error());
            assert_eq!(libc::grantpt(master), 0, "grantpt: {}", std::io::Error::last_os_error());
            assert_eq!(libc::unlockpt(master), 0, "unlockpt: {}", std::io::Error::last_os_error());
            // a 0x0 terminal is not a terminal the board can draw on
            let size = libc::winsize { ws_row: 40, ws_col: 120, ws_xpixel: 0, ws_ypixel: 0 };
            assert_eq!(libc::ioctl(master, libc::TIOCSWINSZ, &size), 0, "TIOCSWINSZ: {}", std::io::Error::last_os_error());
            let name = libc::ptsname(master);
            assert!(!name.is_null(), "ptsname: {}", std::io::Error::last_os_error());
            (master, PathBuf::from(std::ffi::CStr::from_ptr(name).to_str().unwrap()))
        }
    }
}

/// Seeds exactly one card; a helper for the write-refusal tests.
fn seed_one(b: &Board, now: Option<&str>) {
    let o = b.run(now, &["add", "widgets: one card", "--as", "tester"]);
    assert!(o.status.success(), "seed failed: {}", String::from_utf8_lossy(&o.stderr));
}
