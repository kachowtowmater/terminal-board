//! `tb config rules "TEXT"` / `--file PATH` / `--off`: a board's own conventions. D9 (per-board
//! house rules, client items) — printed by `tb guide`, and shown once to each agent, the first
//! `tb next` since the text was last set or changed.
use std::io::Write;
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
        self.as_who("alice", args)
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

    fn json_as(&self, who: &str, args: &[&str]) -> serde_json::Value {
        let out = self.ok_as(who, args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    fn refused(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(1), "{args:?} should be refused: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stderr).trim().to_string()
    }
}

#[test]
fn setting_reading_and_clearing_rules() {
    let b = Board::new();
    assert_eq!(b.ok(&["config", "rules"]).trim(), "rules is off — this board has no house rules set: 'tb config rules \"TEXT\"' or 'tb config rules --file PATH'");
    let v = b.json(&["config", "rules", "branch names: fix/<issue>", "--json"]);
    assert_eq!(v["config"]["value"], "branch names: fix/<issue>");
    assert_eq!(b.ok(&["config", "rules"]).trim_end(), "branch names: fix/<issue>");
    assert_eq!(b.json(&["config", "rules", "--json"])["config"]["value"], "branch names: fix/<issue>");
    // it shows up in the aggregate listing too, only once set
    assert!(b.ok(&["config"]).contains("rules"));
    let cleared = b.json(&["config", "rules", "--off", "--json"]);
    assert_eq!(cleared["config"]["value"], serde_json::Value::Null);
    assert_eq!(b.json(&["config", "rules", "--json"])["config"]["value"], serde_json::Value::Null);
    assert!(!b.ok(&["config"]).contains("rules"), "an unset board that never had rules lists nothing new");
}

#[test]
fn blank_text_is_refused_use_off_instead() {
    let b = Board::new();
    let e = b.refused(&["config", "rules", "   "]);
    assert!(e.contains("rules text is empty"), "{e}");
    assert!(e.contains("--off"), "{e}");
}

#[test]
fn rules_from_a_file_arrive_byte_for_byte() {
    let b = Board::new();
    let mut f = tempfile::NamedTempFile::new().unwrap();
    write!(f, "  house rules:\n  - note before you stop\n").unwrap();
    b.ok(&["config", "rules", "--file", f.path().to_str().unwrap()]);
    // set_rules trims the OUTER blank space, same as every other text setting; the inner
    // indentation and line break survive, and the reader adds exactly one trailing newline
    assert_eq!(b.ok(&["config", "rules"]), "house rules:\n  - note before you stop\n");
}

#[test]
fn tb_guide_prints_the_rules_only_once_set() {
    let b = Board::new();
    let unset = b.ok(&["guide"]);
    assert!(!unset.contains("This board's rules"), "a board that sets nothing renders exactly as before");
    b.ok(&["config", "rules", "ping the on-call before a force-push"]);
    let set = b.ok(&["guide"]);
    assert!(set.contains("## This board's rules"), "{set}");
    assert!(set.contains("ping the on-call before a force-push"), "{set}");
    // nothing about the manual above it changed
    assert!(set.starts_with(&unset), "the manual text itself is untouched, only appended to");
}

#[test]
fn tb_next_shows_the_rules_once_per_agent_then_stops() {
    let b = Board::new();
    b.ok(&["config", "wip", "9"]);
    b.ok(&["add", "docs: card one"]);
    b.ok(&["add", "docs: card two"]);
    b.ok(&["config", "rules", "read the brief before you start"]);

    // alice's first `tb next`: the banner shows
    let first = b.ok_as("alice", &["next"]);
    assert!(first.contains("this board's rules:\nread the brief before you start"), "{first}");

    // alice's SECOND `tb next`: no banner this time
    let second = b.ok_as("alice", &["next"]);
    assert!(!second.contains("this board's rules"), "{second}");

    // bob's first `tb next` (a fresh actor on the same board) shows it too — per agent
    b.ok(&["add", "docs: card three"]);
    let bob_first = b.ok_as("bob", &["next"]);
    assert!(bob_first.contains("this board's rules:\nread the brief before you start"), "{bob_first}");
}

#[test]
fn tb_next_json_carries_the_banner_as_an_additive_field_only_the_first_time() {
    let b = Board::new();
    b.ok(&["config", "wip", "9"]);
    b.ok(&["add", "docs: card one"]);
    b.ok(&["add", "docs: card two"]);
    b.ok(&["config", "rules", "read the brief before you start"]);

    let first = b.json_as("carol", &["next", "--json"]);
    assert_eq!(first["rules"], "read the brief before you start");
    assert_eq!(first["card"]["id"], 1);

    let second = b.json_as("carol", &["next", "--json"]);
    assert!(second.get("rules").is_none(), "no 'rules' field the second time: {second}");
    assert_eq!(second["card"]["id"], 2);
}

#[test]
fn a_board_with_no_rules_adds_no_json_field_and_no_banner() {
    let b = Board::new();
    b.ok(&["add", "docs: card one"]);
    let v = b.json_as("alice", &["next", "--json"]);
    assert!(v.get("rules").is_none(), "{v}");
    let plain = b.ok_as("bob", &["show", "1"]);
    assert!(!plain.contains("this board's rules"), "{plain}");
}

#[test]
fn tb_take_never_shows_the_banner_only_tb_next_does() {
    let b = Board::new();
    b.ok(&["add", "docs: card one"]);
    b.ok(&["config", "rules", "read the brief before you start"]);
    let out = b.ok_as("dave", &["take", "1"]);
    assert!(!out.contains("this board's rules"), "{out}");
    // dave's first `tb next` (on a second card) still shows it — `take` did not mark it seen
    b.ok(&["add", "docs: card two"]);
    let next_out = b.ok_as("dave", &["next"]);
    assert!(next_out.contains("this board's rules"), "{next_out}");
}

#[test]
fn editing_the_rules_shows_the_new_text_again_even_to_someone_who_saw_the_old_wording() {
    let b = Board::new();
    b.ok(&["config", "wip", "9"]);
    b.ok(&["add", "docs: card one"]);
    b.ok(&["add", "docs: card two"]);
    b.ok(&["config", "rules", "old wording"]);
    let first = b.ok_as("alice", &["next"]);
    assert!(first.contains("old wording"), "{first}");
    let repeat = b.ok_as("alice", &["next"]);
    assert!(!repeat.contains("this board's rules"), "already seen: {repeat}");

    b.ok(&["config", "rules", "new wording"]);
    b.ok(&["add", "docs: card three"]);
    let after_change = b.ok_as("alice", &["next"]);
    assert!(after_change.contains("this board's rules:\nnew wording"), "{after_change}");
}

/// Either form of `tb next` counts as "seen" for the other: an agent whose first contact with
/// the board is `tb next --review` is not shown the same text again on a later plain `tb next`.
#[test]
fn next_review_and_plain_next_share_the_same_seen_mark() {
    let b = Board::new();
    b.ok(&["config", "wip", "9"]);
    b.ok(&["add", "docs: a card to review"]);
    b.ok_as("bob", &["take", "1"]);
    b.ok_as("bob", &["done", "1"]); // -> review
    b.ok(&["config", "rules", "check done criteria before approving"]);
    b.ok(&["add", "docs: card two"]);

    let review_next = b.ok_as("alice", &["next", "--review"]);
    assert!(review_next.contains("check done criteria before approving"), "{review_next}");
    let plain_next = b.ok_as("alice", &["next"]);
    assert!(!plain_next.contains("this board's rules"), "{plain_next}");
}
