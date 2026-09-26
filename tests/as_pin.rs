//! #191: a session launched with `TB_AS` pinned (tb-agent-start exports it so every `tb`
//! line lands under the launched name) cannot act under a DIFFERENT `--as` on a real board —
//! refused with JSON code `as_mismatch`, naming the launched identity, before anything is
//! opened or written. `TB_DB` (a test/fixture file) keeps free naming so the suite is
//! unaffected, and a person (no `TB_AS`) is never touched.
//!
//! The refusal cases run on a REAL board path (a temp `HOME`, no `TB_DB`) so they cover
//! exactly what the gate covers; every fixture case below passes a real `--as`, so none
//! would be affected by the guard even if it ran there.
use std::path::PathBuf;
use std::process::{Command, Output};

struct Board {
    _dir: tempfile::TempDir,
    db: Option<PathBuf>,
}

impl Board {
    /// A real board path: `HOME` pinned to a temp dir, `TB_DB` unset, `USER` fixed so the
    /// fallback chain resolves the same way every run.
    fn real() -> Board {
        let dir = tempfile::tempdir().unwrap();
        Board { _dir: dir, db: None }
    }

    /// A `TB_DB` fixture: the shape every other test in the suite uses.
    fn pinned_db() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("b.db");
        Board { _dir: dir, db: Some(db) }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).env("USER", "person").env("TZ", "UTC");
        match &self.db {
            Some(db) => {
                c.env("TB_DB", db);
            }
            None => {
                c.env("HOME", self._dir.path());
            }
        }
        c.output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        o
    }

    /// The agent session: `TB_AS` pinned exactly as tb-agent-start does (with the identity
    /// fields tb records alongside it).
    fn as_agent(&self, pinned: &str, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args)
            .env("TB_AS", pinned)
            .env("TB_HARNESS", "omp")
            .env("TB_MODEL", "g")
            .env("TB_ROLE", "coder")
            .env("TB_SESSION", "omp-b-x-1")
            .env("TB_NO_HERDR", "1")
            .env("TZ", "UTC");
        match &self.db {
            Some(db) => {
                c.env("TB_DB", db);
            }
            None => {
                c.env("HOME", self._dir.path());
            }
        }
        c.output().unwrap()
    }

    fn json(v: &[u8]) -> serde_json::Value {
        serde_json::from_slice(v).unwrap()
    }
}

/// The pinned session names a different `--as` on a real board: refused with `as_mismatch`,
/// the message names the LAUNCHED identity, and no card is created.
#[test]
fn a_pinned_session_cannot_act_under_another_name_on_a_real_board() {
    let b = Board::real();
    b.ok(&["pin-board", "add", "seed", "--as", "charles"]);
    let o = b.as_agent("b-x", &["pin-board", "add", "forged", "--as", "b-y", "--json"]);
    assert!(!o.status.success(), "the forged --as must be refused");
    let v = Board::json(&o.stdout);
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["code"], "as_mismatch", "{v}");
    let err = v["error"].as_str().unwrap();
    assert!(err.contains("b-x"), "the refusal names the launched identity: {err}");
    assert!(err.contains("b-y"), "the refusal names the asked-for name: {err}");
    let n = b.ok(&["pin-board", "list", "--json", "--as", "charles"]);
    assert_eq!(Board::json(&n.stdout).as_array().unwrap().len(), 1, "nothing was written");
}

/// The same session acting as its own pinned name works on the real board — every spelling
/// the comparison accepts: exact, padded, different case.
#[test]
fn a_pinned_session_acts_as_its_own_name() {
    for (pinned, asked) in [("b-x", "b-x"), ("b-x", "  b-x  "), ("b-x", "B-X")] {
        let b = Board::real();
        let o = b.as_agent(pinned, &["pin-board", "add", "own", "--as", asked, "--json"]);
        assert!(o.status.success(), "pinned={pinned:?} asked={asked:?}: {}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(
            Board::json(&o.stdout)["card"]["events"][0]["actor"],
            "b-x",
            "the event carries the pinned name"
        );
    }
}

/// With `TB_AS` unset — a person — any `--as` still works on a real board.
#[test]
fn a_person_names_themselves_freely_on_a_real_board() {
    let b = Board::real();
    b.ok(&["pin-board", "add", "seed", "--as", "charles"]);
    b.ok(&["pin-board", "add", "mine", "--as", "whoever"]);
    let n = b.ok(&["pin-board", "list", "--json", "--as", "charles"]);
    assert_eq!(Board::json(&n.stdout).as_array().unwrap().len(), 2);
}

/// The identity chain behind `--as`: when the flag is absent, `TB_AS` still wins on the real
/// board (the launched name is used with no refusal) — the pin does not break the fallback.
#[test]
fn a_pinned_session_without_a_flag_uses_its_launched_name() {
    let b = Board::real();
    let o = b.as_agent("b-x", &["pin-board", "add", "own", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        Board::json(&o.stdout)["card"]["events"][0]["actor"],
        "b-x",
        "the pinned name is used with no flag"
    );
}

/// An empty/whitespace `TB_AS` is not a pin: nothing is ever refused on that account.
#[test]
fn an_empty_tb_as_is_not_a_pin() {
    for v in ["", "   "] {
        let b = Board::real();
        b.ok(&["pin-board", "add", "seed", "--as", "charles"]);
        let o = Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(["pin-board", "add", "other", "--as", "someone-else"])
            .env("TB_AS", v)
            .env("HOME", b._dir.path())
            .env("TZ", "UTC")
            .output()
            .unwrap();
        assert!(o.status.success(), "TB_AS={v:?}: {}", String::from_utf8_lossy(&o.stderr));
    }
}

/// Under `TB_DB` — a test or fixture DB — any `--as` still works, pinned or not.
#[test]
fn a_tb_db_fixture_keeps_free_naming() {
    let b = Board::pinned_db();
    let o = b.as_agent("b-x", &["add", "fixture", "--as", "b-y", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        Board::json(&o.stdout)["card"]["events"][0]["actor"],
        "b-y",
        "under TB_DB the asked-for name is used"
    );
}

/// The refusal fires before anything is opened: a pinned session cannot CREATE a real board
/// under another name either (the writes()-gated create path), while its own name can.
#[test]
fn the_refusal_precedes_board_creation() {
    let b = Board::real();
    let o = b.as_agent("b-x", &["pin-board", "add", "forged", "--as", "b-y", "--json"]);
    assert!(!o.status.success());
    assert_eq!(Board::json(&o.stdout)["code"], "as_mismatch");
    assert!(
        !b._dir.path().join(".local/state/terminal-board/boards/pin-board.db").exists(),
        "no board was created"
    );
    let o = b.as_agent("b-x", &["pin-board", "add", "own"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

/// A read command is unaffected for BOTH names: the pin only gates what the session ACTS as,
/// and a session may always look at a board (here: one it cannot write under another name).
#[test]
fn a_pinned_session_can_still_read_under_a_differing_name() {
    let b = Board::real();
    b.ok(&["pin-board", "add", "seed", "--as", "charles"]);
    let o = b.as_agent("b-x", &["pin-board", "list", "--as", "b-y", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(Board::json(&o.stdout).as_array().unwrap().len(), 1);
}
