//! The `TB_NOW` clock override (legacy `TTYBOARD_NOW`): an in-range value pins the clock,
//! anything else — not an integer, or outside 946684800–4102444800 (2000–2100) — is refused
//! before anything is written, in plain mode and under `--json`.
//!
//! The suite pins the clock by setting `TB_NOW` in the environment of the spawned binary
//! (`tests/common::pin_clock`), so the check lives in the release binary: no cfg gate.
use std::process::{Command, Output};

const MIN: &str = "946684800"; // 2000-01-01T00:00:00Z — keep in step with store::TB_NOW_MIN
const MAX: &str = "4102444800"; // 2100-01-01T00:00:00Z — keep in step with store::TB_NOW_MAX

struct Board {
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
}

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("board.db");
        Board { _dir: dir, db }
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
    // the edges of the window are accepted too (first and last second of 2000–2100)
    for edge in [MIN, MAX] {
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
    for bad in ["0", "-1", "9223372036854775807", "1e9999", "abc", " 12 ", "946684799", "4102444801"] {
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

/// Seeds exactly one card; a helper for the write-refusal tests.
fn seed_one(b: &Board, now: Option<&str>) {
    let o = b.run(now, &["add", "widgets: one card", "--as", "tester"]);
    assert!(o.status.success(), "seed failed: {}", String::from_utf8_lossy(&o.stderr));
}
