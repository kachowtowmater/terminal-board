//! `tb assign ID NAME`: hand a specific TODO card to NAME, without the caller (`--as`) becoming
//! its owner — `take` run on someone else's behalf. D5 (per-board house rules, client items).
//!
//! It reaches DOING through the SAME transition and WIP check `take`/`next`/`move` share, and
//! only from TODO — the same restriction `take` itself enforces (and, like `take`, there is no
//! `--force` to pull a card away from whoever already holds it).
use std::path::PathBuf;
use std::process::{Command, Output};

struct Board {
    _dir: tempfile::TempDir,
    db: PathBuf,
}

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("board.db");
        Board { _dir: dir, db }
    }

    fn as_who(&self, who: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("TB_DB", &self.db)
            .env("TB_AS", who)
            .env("TB_NO_HERDR", "1")
            .env("TZ", "UTC")
            .env_remove("TB_BOARD")
            .env_remove("HERDR_AGENT_NAME")
            .output()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.as_who("orchestrator", args)
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn ok_as(&self, who: &str, args: &[&str]) -> String {
        let o = self.as_who(who, args);
        assert!(o.status.success(), "[{who}] {args:?} failed: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let out = self.ok(args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    fn refused(&self, args: &[&str]) -> String {
        self.refused_as("orchestrator", args)
    }

    fn refused_as(&self, who: &str, args: &[&str]) -> String {
        let o = self.as_who(who, args);
        assert_eq!(o.status.code(), Some(1), "[{who}] {args:?} should be refused: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stderr).trim().to_string()
    }
}

#[test]
fn assign_hands_a_todo_card_to_name_without_the_caller_becoming_owner() {
    let b = Board::new();
    b.ok(&["add", "ops: rotate the keys"]);
    let v = b.json(&["assign", "1", "carol", "--json"]);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["card"]["id"], 1);
    assert_eq!(v["card"]["column"], "doing");
    assert_eq!(v["card"]["owner"], "carol", "the NAMED person holds it, not --as");

    // the event log keeps the two facts apart: who assigned it vs. who now holds it
    let show = b.json(&["show", "1", "--json"]);
    let events = show["events"].as_array().unwrap();
    let assigned = events.iter().find(|e| e["kind"] == "assigned").expect("an 'assigned' event");
    assert_eq!(assigned["actor"], "orchestrator", "actor is WHO ASSIGNED it");
    assert_eq!(assigned["text"], "assigned to carol");
    assert_eq!(show["owner"], "carol", "owner is who now HOLDS it");
}

#[test]
fn assign_only_reaches_a_card_in_todo_same_as_take() {
    let b = Board::new();
    b.ok(&["add", "ops: rotate the keys"]);
    b.ok(&["assign", "1", "carol"]);
    // card 1 is now in doing, held by carol — assigning it again is refused, exactly like
    // `take 1` would be (no --force exists on either to pull it away from its holder)
    let take_err = b.refused(&["take", "1"]);
    let assign_err = b.refused(&["assign", "1", "dave"]);
    assert!(take_err.contains("not todo"), "{take_err}");
    assert_eq!(take_err, assign_err, "assign reuses take's own refusal text for a non-todo card, byte for byte");
    // carol still holds it — assign never stole it out from under her
    assert_eq!(b.json(&["show", "1", "--json"])["owner"], "carol");
}

#[test]
fn assign_respects_the_board_wide_wip_limit() {
    let b = Board::new();
    b.ok(&["config", "wip", "1"]);
    b.ok(&["add", "ops: card one"]);
    b.ok(&["add", "ops: card two"]);
    b.ok(&["assign", "1", "carol"]); // fills the one slot
    let e = b.refused(&["assign", "2", "dave"]);
    assert!(e.contains("doing is full (1/1"), "{e}");
    // card 2 is untouched: still todo, unowned
    let c2 = b.json(&["show", "2", "--json"]);
    assert_eq!(c2["column"], "todo");
    assert_eq!(c2["owner"], serde_json::Value::Null);
}

#[test]
fn an_empty_name_is_refused_before_anything_changes() {
    let b = Board::new();
    b.ok(&["add", "ops: card one"]);
    let e = b.refused(&["assign", "1", " "]);
    assert!(e.contains("name is empty"), "{e}");
    assert_eq!(b.json(&["show", "1", "--json"])["column"], "todo", "nothing moved");
}

#[test]
fn assign_trims_the_name() {
    let b = Board::new();
    b.ok(&["add", "ops: card one"]);
    b.ok(&["assign", "1", "  carol  "]);
    assert_eq!(b.json(&["show", "1", "--json"])["owner"], "carol");
}

#[test]
fn assigning_a_card_that_does_not_exist_is_the_same_no_card_error_as_every_other_command() {
    let b = Board::new();
    // the board must already exist for both calls, or the FIRST one alone also prints a
    // one-time "created board" line — add a card first so neither refusal carries it
    b.ok(&["add", "ops: card one"]);
    let assign_err = b.refused(&["assign", "9", "carol"]);
    let take_err = b.refused(&["take", "9"]);
    assert_eq!(assign_err, take_err, "same 'no card #9' refusal shape as every other command");
}

/// Someone else runs `tb assign`, then the person it named continues the work as usual — this
/// is the whole point of the command: an orchestrator directs without ever holding the card.
#[test]
fn the_named_person_then_works_the_card_normally() {
    let b = Board::new();
    b.ok(&["add", "ops: rotate the keys"]);
    b.ok(&["assign", "1", "carol"]);
    b.ok_as("carol", &["note", "1", "started"]);
    b.ok_as("carol", &["done", "1"]);
    assert_eq!(b.json(&["show", "1", "--json"])["column"], "review");
}
