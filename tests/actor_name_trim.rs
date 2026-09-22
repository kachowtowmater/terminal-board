//! #103: `--as "anna "` (or any actor name with leading/trailing space) must behave exactly
//! like `--as anna` everywhere tb compares an actor against a stored name. The name is
//! trimmed once, in `resolve_actor` — so every comparison site downstream (the card holder,
//! the `done-by` list, the self-approval guard, and the reviewer-claim guard) sees the same
//! clean value without needing its own trim. These tests exercise each site through the real
//! binary, with a padded `--as` on one side of the comparison and a clean name on the other.
use std::path::PathBuf;
use std::process::{Command, Output};

struct Board {
    _dir: tempfile::TempDir,
    db: PathBuf,
}

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("b.db");
        Board { _dir: dir, db }
    }

    /// `actor` is passed as `--as`, padded or not — never through `$TB_AS`, so this exercises
    /// exactly the flag the bug report used.
    fn run(&self, actor: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .args(["--as", actor])
            .env("TB_DB", &self.db)
            .env("TB_NO_HERDR", "1")
            .env_remove("TB_AS")
            .env_remove("HERDR_AGENT_NAME")
            .output()
            .unwrap()
    }

    fn ok(&self, actor: &str, args: &[&str]) -> String {
        let o = self.run(actor, args);
        assert!(o.status.success(), "[{actor}] {args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, actor: &str, args: &[&str]) -> serde_json::Value {
        serde_json::from_str(&self.ok(actor, args)).unwrap()
    }
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

/// A padded `--as` is stored trimmed, not literally: the event carries the clean name.
#[test]
fn a_padded_actor_is_recorded_trimmed() {
    let b = Board::new();
    b.ok("  anna  ", &["add", "docs: x"]);
    let v = b.json("anna", &["show", "1", "--json"]);
    assert_eq!(v["events"][0]["actor"], "anna", "the stored actor keeps no padding: {v}");
}

/// Holder guard (`src/store.rs`, the DOING-card ownership check): the same person, asking
/// with a padded name, is not treated as a stranger to their own card.
#[test]
fn the_holder_guard_matches_a_padded_name() {
    let b = Board::new();
    b.ok("anna", &["add", "docs: y"]);
    b.ok("anna", &["take", "1"]);
    let o = b.run(" anna ", &["move", "1", "review"]);
    assert!(o.status.success(), "the card's own holder, asking with a padded name, was refused: {}", stderr(&o));

    // a genuine stranger is still refused — the fix does not loosen the guard itself
    b.ok("bob", &["add", "docs: y2"]);
    b.ok("bob", &["take", "2"]);
    let o = b.run(" carol ", &["move", "2", "review"]);
    assert!(!o.status.success(), "a stranger with a padded name was let through the holder guard");
    assert!(stderr(&o).contains("is held by bob"), "{}", stderr(&o));
}

/// `done-by` (`src/store/closing.rs::may_close`): a name on the list, asked for with padding,
/// still closes the card.
#[test]
fn the_done_by_list_matches_a_padded_name() {
    let b = Board::new();
    b.ok("bob", &["add", "docs: z"]);
    b.ok("bob", &["take", "1"]);
    b.ok("bob", &["done", "1"]);
    b.ok("alice", &["config", "done-by", "anna"]);
    let o = b.run(" anna ", &["done", "1"]);
    assert!(o.status.success(), "a done-by name with padding was refused: {}", stderr(&o));
    let column = b.json("anna", &["show", "1", "--json"])["column"].clone();
    assert_eq!(column, "done");
}

/// Self-approval guard (`src/store.rs`, REVIEW -> DONE): the author cannot dodge the rule by
/// padding their own name either — the guard still recognizes them.
#[test]
fn the_self_approval_guard_still_catches_a_padded_name() {
    let b = Board::new();
    b.ok("carol", &["add", "docs: w"]);
    b.ok("carol", &["take", "1"]);
    b.ok("carol", &["done", "1"]);
    let o = b.run(" carol ", &["done", "1"]);
    assert!(!o.status.success(), "the author, padded, was let through their own approval");
    assert!(stderr(&o).contains("you did this work"), "{}", stderr(&o));
}

/// Reviewer-claim guard (`Store::next_review`): a reviewer cannot claim their own authored
/// card by asking with a padded name either.
#[test]
fn next_review_still_skips_the_authors_own_card_for_a_padded_name() {
    let b = Board::new();
    b.ok("dave", &["add", "docs: v"]);
    b.ok("dave", &["take", "1"]);
    b.ok("dave", &["done", "1"]);
    let o = b.run(" dave ", &["next", "--review"]);
    assert!(!o.status.success(), "the author, padded, was offered their own card to review");
    assert!(stderr(&o).contains("your own work"), "{}", stderr(&o));
}

/// The empty-after-trim name stays refused: this fix must not make `--as " "` legal.
#[test]
fn a_whitespace_only_actor_is_still_refused() {
    let b = Board::new();
    let o = b.run(" ", &["add", "docs: refused"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--as is empty"), "{}", stderr(&o));
}
