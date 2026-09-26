//! #209: a session launched against ONE card (`TB_CARD=<board>#<id>` — what
//! `tb-agent-start --card` exports) may only WRITE its own card: every card write on any
//! other card — including a new card (`add`) and any card on another board — is refused
//! with JSON code `card_bound`, naming the card this session is bound to. `note` stays
//! allowed on ANY card (a builder reports what it finds on other cards), reads are never
//! touched, and the exemptions are the ones the guard shares with the `TB_AS` pin (#191):
//! a `TB_DB` fixture (a test), a person (no `TB_CARD`), and `TB_ROLE` lead/orchestrator.
//!
//! Like `as_pin.rs`, the refusal cases run on a REAL board path (a temp `HOME`, no
//! `TB_DB`) so they cover exactly what the gate covers; because the check sits in
//! `main::run` after the board resolves, every shell-fed form of a command goes through
//! it too.
use std::path::PathBuf;
use std::process::{Command, Output};

/// A builder's own session exports `TB_*` values that would leak INTO the tested process
/// through `Command`'s inherited environment (the suite's `TB_DB` fixture, this session's
/// `TB_CARD`) and change what the guard sees — every child here starts from a clean slate,
/// then adds exactly what its role in the test names.
fn scrub(c: &mut Command) {
    for k in ["TB_CARD", "TB_AS", "TB_HARNESS", "TB_MODEL", "TB_ROLE", "TB_SESSION", "TB_DB", "TB_BOARD", "TB_HOST", "TB_NO_HERDR", "TB_READONLY", "TTYBOARD_CARD", "TTYBOARD_AS", "TTYBOARD_DB"] {
        c.env_remove(k);
    }
}

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
    fn fixture_db() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("b.db");
        Board { _dir: dir, db: Some(db) }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).env("USER", "person").env("TZ", "UTC");
        match &self.db {
            Some(db) => c.env("TB_DB", db),
            None => c.env("HOME", self._dir.path()),
        };
        scrub(&mut c);
        c.output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        o
    }

    /// The card-bound worker session: pinned exactly as `tb-agent-start --card` does (the
    /// identity fields tb records alongside it included).
    fn as_bound(&self, bound: &str, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args)
            .env("TB_CARD", bound)
            .env("TB_AS", "b-1")
            .env("TB_HARNESS", "omp")
            .env("TB_MODEL", "g")
            .env("TB_ROLE", "coder")
            .env("TB_SESSION", "omp-b-1-x")
            .env("TB_NO_HERDR", "1")
            .env("TZ", "UTC");
        match &self.db {
            Some(db) => c.env("TB_DB", db),
            None => c.env("HOME", self._dir.path()),
        };
        scrub(&mut c);
        c.output().unwrap()
    }

    /// The same session with the role swapped — how a lead/orchestrator pane runs.
    fn with_role(&self, bound: &str, role: &str, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args)
            .env("TB_CARD", bound)
            .env("TB_AS", "lead-p")
            .env("TB_HARNESS", "omp")
            .env("TB_MODEL", "g")
            .env("TB_ROLE", role)
            .env("TB_SESSION", "omp-lead-x")
            .env("TB_NO_HERDR", "1")
            .env("TZ", "UTC");
        match &self.db {
            Some(db) => c.env("TB_DB", db),
            None => c.env("HOME", self._dir.path()),
        };
        scrub(&mut c);
        c.output().unwrap()
    }

    /// A plain person: no `TB_CARD`, no `TB_AS`, no role.
    fn as_person(&self, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).env_remove("TB_CARD").env("USER", "person").env("TZ", "UTC");
        match &self.db {
            Some(db) => c.env("TB_DB", db),
            None => c.env("HOME", self._dir.path()),
        };
        scrub(&mut c);
        c.output().unwrap()
    }

    fn json(v: &[u8]) -> serde_json::Value {
        serde_json::from_slice(v).unwrap()
    }

    /// The card count on the named board, read as a person.
    fn count(&self, board: &str) -> usize {
        let n = self.ok(&[board, "list", "--json", "--as", "charles"]);
        Board::json(&n.stdout).as_array().unwrap().len()
    }
}

/// Every write on another card is refused with `card_bound`, naming the bound card, and
/// nothing is written.
#[test]
fn a_bound_session_cannot_write_another_card() {
    let b = Board::real();
    b.ok(&["p", "add", "c1", "--as", "charles"]);
    b.ok(&["p", "add", "c2", "--as", "charles"]);
    for args in [
        vec!["p", "take", "2", "--as", "b-1", "--json"],
        vec!["p", "edit", "2", "--title", "forged", "--as", "b-1", "--json"],
        vec!["p", "move", "2", "doing", "--as", "b-1", "--json"],
        vec!["p", "done", "2", "--as", "b-1", "--json"],
        vec!["p", "block", "2", "stuck", "--as", "b-1", "--json"],
        vec!["p", "drop", "2", "--as", "b-1", "--json"],
        vec!["p", "check", "2", "1", "--as", "b-1", "--json"],
        vec!["p", "rm", "2", "--force", "--as", "b-1", "--json"],
        vec!["p", "prio", "2", "top", "--as", "b-1", "--json"],
        vec!["p", "assign", "2", "b-1", "--as", "b-1", "--json"],
    ] {
        let o = b.as_bound("p#1", &args);
        assert!(!o.status.success(), "{args:?} must be refused");
        let v = Board::json(&o.stdout);
        assert_eq!(v["ok"], false, "{args:?}: {v}");
        assert_eq!(v["code"], "card_bound", "{args:?}: {v}");
        let err = v["error"].as_str().unwrap();
        assert!(err.contains("p#1"), "the refusal names the bound card: {err}");
        assert!(err.contains("p#2"), "the refusal names the asked-for card: {err}");
    }
    assert_eq!(b.count("p"), 2, "nothing was written");
}

/// A bound session works its own card: `take` (the very first move) succeeds.
#[test]
fn a_bound_session_writes_its_own_card() {
    let b = Board::real();
    b.ok(&["p", "add", "mine", "--as", "charles"]);
    b.ok(&["p", "add", "other", "--as", "charles"]);
    let o = b.as_bound("p#1", &["p", "take", "1", "--as", "b-1", "--json"]);
    assert!(o.status.success(), "its own card works: {}", String::from_utf8_lossy(&o.stderr));
}

/// A new card (`add`) is always refused: the session is bound to a card that exists.
#[test]
fn a_bound_session_cannot_add_a_card() {
    let b = Board::real();
    b.ok(&["p", "add", "c1", "--as", "charles"]);
    let o = b.as_bound("p#1", &["p", "add", "smuggled", "--as", "b-1", "--json"]);
    assert!(!o.status.success(), "the add must be refused");
    let v = Board::json(&o.stdout);
    assert_eq!(v["code"], "card_bound", "{v}");
    let err = v["error"].as_str().unwrap();
    assert!(err.contains("p#1"), "the refusal names the bound card: {err}");
    assert_eq!(b.count("p"), 1, "no card was created");
}

/// A write on a card of ANOTHER board is refused too: `TB_CARD=p#1` does not own `q#1`,
/// even though the id matches.
#[test]
fn a_bound_session_cannot_write_on_another_board() {
    let b = Board::real();
    b.ok(&["p", "add", "c1", "--as", "charles"]);
    b.ok(&["q", "add", "q1", "--as", "charles"]);
    let o = b.as_bound("p#1", &["q", "take", "1", "--as", "b-1", "--json"]);
    assert!(!o.status.success(), "the cross-board take must be refused");
    let v = Board::json(&o.stdout);
    assert_eq!(v["code"], "card_bound", "{v}");
    let err = v["error"].as_str().unwrap();
    assert!(err.contains("p#1"), "the refusal names the bound card: {err}");
}

/// A note on ANOTHER card stays allowed (a builder reports what it finds), and notes on
/// its own card work as ever.
#[test]
fn a_bound_session_can_note_any_card() {
    let b = Board::real();
    b.ok(&["p", "add", "c1", "--as", "charles"]);
    b.ok(&["p", "add", "c2", "--as", "charles"]);
    let o = b.as_bound("p#1", &["p", "note", "2", "found this on card 2", "--as", "b-1"]);
    assert!(o.status.success(), "a note on another card is allowed: {}", String::from_utf8_lossy(&o.stderr));
    b.as_bound("p#1", &["p", "note", "1", "progress on my card", "--as", "b-1"]);
}

/// Reads are unaffected: `list`, `show`, a bare `tb` all run.
#[test]
fn a_bound_session_still_reads() {
    let b = Board::real();
    b.ok(&["p", "add", "c1", "--as", "charles"]);
    for args in [
        vec!["p", "list", "--as", "b-1", "--json"],
        vec!["p", "show", "1", "--as", "b-1", "--json"],
    ] {
        let o = b.as_bound("p#1", &args);
        assert!(o.status.success(), "{args:?} works: {}", String::from_utf8_lossy(&o.stderr));
    }
}

/// A `TB_DB` fixture is exempt, exactly like the `TB_AS` pin: the whole test suite keeps
/// its free naming and free writing.
#[test]
fn a_tb_db_fixture_is_exempt() {
    let b = Board::fixture_db();
    b.ok(&["add", "seed", "--as", "charles"]);
    b.ok(&["add", "other", "--as", "charles"]);
    let o = b.as_bound("default#1", &["take", "1", "--as", "b-1"]);
    assert!(o.status.success(), "a fixture DB is unaffected: {}", String::from_utf8_lossy(&o.stderr));
}

/// A person (no `TB_CARD`) is never touched: any write on any card works.
#[test]
fn a_person_is_exempt() {
    let b = Board::real();
    b.ok(&["p", "add", "c1", "--as", "charles"]);
    b.ok(&["p", "add", "c2", "--as", "charles"]);
    let o = b.as_person(&["p", "take", "2", "--as", "charles"]);
    assert!(o.status.success(), "a person works any card: {}", String::from_utf8_lossy(&o.stderr));
}

/// `TB_ROLE=lead` (and `orchestrator`) are exempt: they steer every card, not one.
#[test]
fn a_lead_role_is_exempt() {
    let b = Board::real();
    b.ok(&["p", "add", "c1", "--as", "charles"]);
    b.ok(&["p", "add", "c2", "--as", "charles"]);
    // `take 2` succeeds the first time and is refused (already held) afterwards — either
    // way the CARD-BOUND guard is the one thing it must never be refused by
    for role in ["lead", "orchestrator", "LEAD"] {
        let o = b.with_role("p#1", role, &["p", "take", "2", "--json"]);
        assert!(o.status.success() || Board::json(&o.stdout)["code"] != "card_bound", "{role} steers any card: {}", String::from_utf8_lossy(&o.stderr));
    }
}

/// The refusal precedes the write: a forged write changes nothing even though it names a
/// card that exists. (The shell-fed forms — bash <<<, echo|bash, cat|bash, tee|bash,
/// xargs bash -c, `x=add; tb $x …` — all land HERE, because the check is inside tb's
/// parsed command, so a wrapper cannot smuggle a write past it.)
#[test]
fn the_refusal_precedes_card_writes() {
    let b = Board::real();
    b.ok(&["p", "add", "c1", "--as", "charles"]);
    b.ok(&["p", "add", "c2", "--as", "charles"]);
    for args in [
        vec!["p", "take", "2", "--as", "b-1"],
        vec!["p", "edit", "2", "--title", "forged", "--as", "b-1"],
        vec!["p", "add", "smuggled", "--as", "b-1"],
    ] {
        let o = b.as_bound("p#1", &args);
        assert!(!o.status.success(), "{args:?} refused");
    }
    b.as_bound("p#1", &["p", "note", "2", "a note is not a write on the card", "--as", "b-1"]);
    assert_eq!(b.count("p"), 2, "nothing was written");
}
