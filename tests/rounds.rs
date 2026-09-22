//! A11: `config max-rounds N` marks a card sent back more than N times **escalate** — derived,
//! never stored — and `tb next` / `tb next --review`'s automatic pick skips it, so a loop
//! between a worker and a reviewer cannot run forever unnoticed. It is never hidden: `tb
//! list`, `tb board` and `tb show` show it exactly as before, and explicit commands
//! (`tb take ID`, `tb move`, `tb done`) still act on it directly.
use std::path::PathBuf;
use std::process::{Command, Output};

const NOW: i64 = 1_790_856_000; // 2026-10-01T12:00:00Z

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
            .env("TB_NOW", NOW.to_string())
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
        assert!(o.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn ok_as(&self, who: &str, args: &[&str]) -> String {
        let o = self.as_who(who, args);
        assert!(o.status.success(), "[{who}] {args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let out = self.ok(args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    fn refused(&self, who: &str, args: &[&str]) -> String {
        let o = self.as_who(who, args);
        assert_eq!(o.status.code(), Some(1), "[{who}] {args:?} should be refused: {}", String::from_utf8_lossy(&o.stdout));
        String::from_utf8_lossy(&o.stderr).trim().to_string()
    }

    fn column(&self, id: i64) -> String {
        self.json(&["show", &id.to_string(), "--json"])["column"].as_str().unwrap().to_string()
    }

    fn escalate(&self, id: i64) -> bool {
        self.json(&["show", &id.to_string(), "--json"])["escalate"].as_bool().unwrap()
    }

    /// Send card `id` back `n` times, alternating worker/reviewer, each round noted so the
    /// send-back reason is never empty. Ends the card in REVIEW, round `n + 1`.
    fn cycle(&self, id: i64, worker: &str, reviewer: &str, n: i64) {
        for i in 0..n {
            self.ok_as(reviewer, &["move", &id.to_string(), "doing", &format!("round {i}: try again")]);
            self.ok_as(worker, &["done", &id.to_string()]);
        }
    }
}

// ------------------------------------------------------------------------ config round trip

#[test]
fn max_rounds_uncapped_by_default_then_validated_and_clearable() {
    let b = Board::new();
    assert_eq!(b.ok(&["config", "max-rounds"]).trim(), "off — no card is ever marked escalate");
    assert!(!b.ok(&["config"]).contains("max-rounds"), "unlisted while unset");

    assert_eq!(b.ok(&["config", "max-rounds", "5"]).trim(), "max-rounds is now 5 — a card sent back more times than that is marked escalate and skipped by 'tb next' / 'tb next --review' (still visible, still workable directly)");
    assert_eq!(b.ok(&["config", "max-rounds"]).trim(), "5");
    assert!(b.ok(&["config"]).contains(&format!("{:<13} {}", "max-rounds", "5")));
    assert_eq!(b.json(&["config", "max-rounds", "--json"])["config"]["value"], serde_json::json!(5));

    // a leading "-1" is clap's own flag-vs-value ambiguity (the same as every numeric config
    // value in this CLI, e.g. `tb config wip -1`), not this validation — not tested here
    for bad in ["0", "1000", "many"] {
        let e = b.refused("alice", &["config", "max-rounds", bad]);
        assert!(e.contains("max-rounds must be") || e.contains("must be a number"), "{bad}: {e}");
    }

    assert_eq!(b.ok(&["config", "max-rounds", "--off"]).trim(), "max-rounds is off — no card is ever marked escalate");
    assert!(!b.ok(&["config"]).contains("max-rounds"), "cleared = unlisted again");
}

// -------------------------------------------------------------------------------- escalate

#[test]
fn escalate_appears_only_after_more_send_backs_than_the_cap_allows() {
    let b = Board::new();
    b.ok(&["config", "max-rounds", "2"]);
    b.ok(&["add", "widgets: fix it"]);
    b.ok_as("bob", &["take", "1"]);
    b.ok_as("bob", &["done", "1"]); // review, round 1
    assert!(!b.escalate(1));
    b.cycle(1, "bob", "carol", 2); // 2 send-backs: round 3, exactly the cap — not escalate
    assert_eq!(b.json(&["show", "1", "--json"])["round"], 3);
    assert!(!b.escalate(1), "sent back exactly N times is not yet 'more than N'");
    b.ok_as("carol", &["move", "1", "doing", "one more pass"]);
    b.ok_as("bob", &["done", "1"]); // round 4: the 3rd send-back — now escalate
    assert_eq!(b.json(&["show", "1", "--json"])["round"], 4);
    assert!(b.escalate(1));
    // still fully visible: tb list, tb board and tb show all show it
    assert_eq!(b.json(&["list", "--json"]).as_array().unwrap().len(), 1);
    assert_eq!(b.json(&["board", "--json"])["columns"]["review"].as_array().unwrap().len(), 1);
}

#[test]
fn escalate_clears_once_the_card_reaches_done() {
    let b = Board::new();
    b.ok(&["config", "max-rounds", "1"]);
    b.ok(&["add", "widgets: fix it"]);
    b.ok_as("bob", &["take", "1"]);
    b.ok_as("bob", &["done", "1"]);
    b.cycle(1, "bob", "carol", 2); // 2 send-backs > cap of 1
    assert!(b.escalate(1));
    b.ok_as("carol", &["done", "1"]); // still reachable directly, --force not needed for the close itself
    assert_eq!(b.column(1), "done");
    assert!(!b.escalate(1), "a finished card is never escalate, whatever its history");
}

#[test]
fn a_board_that_never_sets_max_rounds_never_escalates() {
    let b = Board::new();
    b.ok(&["add", "widgets: fix it"]);
    b.ok_as("bob", &["take", "1"]);
    b.ok_as("bob", &["done", "1"]);
    b.cycle(1, "bob", "carol", 50);
    assert!(!b.escalate(1), "uncapped: never escalate, whatever the round");
    assert_eq!(b.json(&["show", "1", "--json"])["round"], 51);
}

// --------------------------------------------------------------------- tb next / tb next --review

#[test]
fn tb_next_skips_an_escalated_todo_card_but_still_hands_it_to_take() {
    let b = Board::new();
    b.ok(&["config", "max-rounds", "1"]);
    b.ok(&["add", "widgets: fix it"]);
    b.ok(&["add", "ops: rotate tokens"]);
    b.ok_as("bob", &["take", "1"]);
    b.ok_as("bob", &["done", "1"]);
    b.cycle(1, "bob", "carol", 2); // escalate
    assert!(b.escalate(1));
    b.ok_as("bob", &["drop", "1"]); // back to TODO, unowned, still escalate
    assert_eq!(b.column(1), "todo");
    assert!(b.escalate(1));
    // the automatic pick skips it and hands out the other card instead
    assert_eq!(b.json(&["next", "--json", "--as", "dave"])["card"]["id"], 2);
    // it is still explicitly workable: tb take names it directly
    b.ok_as("erin", &["take", "1"]);
    assert_eq!(b.column(1), "doing");
}

#[test]
fn tb_next_says_no_todo_cards_when_only_an_escalated_one_remains() {
    let b = Board::new();
    b.ok(&["config", "max-rounds", "1"]);
    b.ok(&["add", "widgets: fix it"]);
    b.ok_as("bob", &["take", "1"]);
    b.ok_as("bob", &["done", "1"]);
    b.cycle(1, "bob", "carol", 2);
    b.ok_as("bob", &["drop", "1"]);
    assert!(b.escalate(1));
    let e = b.refused("dave", &["next"]);
    assert!(e.contains("no todo cards"), "{e}");
}

#[test]
fn tb_next_review_skips_an_escalated_review_card() {
    let b = Board::new();
    b.ok(&["config", "max-rounds", "1"]);
    b.ok(&["add", "widgets: fix it"]);
    b.ok(&["add", "ops: rotate tokens"]);
    b.ok_as("bob", &["take", "1"]);
    b.ok_as("bob", &["done", "1"]);
    b.cycle(1, "bob", "carol", 2); // escalate, card 1 ends in review
    assert_eq!(b.column(1), "review");
    assert!(b.escalate(1));
    b.ok_as("erin", &["take", "2"]);
    b.ok_as("erin", &["done", "2"]); // card 2 in review too, not escalate
    // the automatic reviewer pick skips card 1 and claims card 2
    assert_eq!(b.json(&["next", "--review", "--json", "--as", "dave"])["card"]["id"], 2);
    // card 1 is still directly workable: a reviewer can act on it by name
    b.ok_as("dave", &["done", "1"]);
    assert_eq!(b.column(1), "done");
}

#[test]
fn tb_next_review_says_none_waiting_when_only_an_escalated_one_remains() {
    let b = Board::new();
    b.ok(&["config", "max-rounds", "1"]);
    b.ok(&["add", "widgets: fix it"]);
    b.ok_as("bob", &["take", "1"]);
    b.ok_as("bob", &["done", "1"]);
    b.cycle(1, "bob", "carol", 2);
    assert!(b.escalate(1));
    let e = b.refused("dave", &["next", "--review"]);
    assert!(e.contains("no review cards waiting"), "{e}");
}

// -------------------------------------------------------------------------- sort due, blocked

#[test]
fn tb_next_skips_an_escalated_card_even_when_it_is_the_nearest_due() {
    let b = Board::new();
    b.ok(&["config", "sort", "due"]);
    b.ok(&["config", "max-rounds", "1"]);
    b.ok(&["add", "permits: the nearest", "--due", "2026-10-02"]);
    b.ok(&["add", "tax: later", "--due", "2026-11-30"]);
    b.ok_as("bob", &["take", "1"]);
    b.ok_as("bob", &["done", "1"]);
    b.cycle(1, "bob", "carol", 2);
    b.ok_as("bob", &["drop", "1"]);
    assert!(b.escalate(1));
    assert_eq!(b.json(&["board", "--json"])["columns"]["todo"][0]["id"], 1, "still first on the board by due date");
    assert_eq!(b.json(&["next", "--json", "--as", "erin"])["card"]["id"], 2, "but tb next skips the escalated one");
}

#[test]
fn escalate_and_blocked_compose_a_card_needs_neither_to_be_skipped() {
    let b = Board::new();
    b.ok(&["config", "max-rounds", "1"]);
    b.ok(&["add", "widgets: fix it"]);
    b.ok(&["add", "ops: rotate tokens"]);
    b.ok_as("bob", &["take", "1"]);
    b.ok_as("bob", &["done", "1"]);
    b.cycle(1, "bob", "carol", 2);
    b.ok_as("bob", &["drop", "1"]);
    b.ok(&["block", "2", "waiting on something"]);
    // one card escalated, the other blocked: nothing left for the automatic pick
    let e = b.refused("dave", &["next"]);
    assert!(e.contains("no todo cards"), "{e}");
    b.ok(&["block", "2", "--clear"]);
    assert_eq!(b.json(&["next", "--json", "--as", "dave"])["card"]["id"], 2, "unblocked and not escalated: now pickable");
}

// -------------------------------------------------------------------------------- documented

#[test]
fn max_rounds_and_escalate_are_documented() {
    let cases: [(&str, &[&str]); 4] = [
        ("README.md", &["max-rounds", "escalate"]),
        ("docs/AGENTS.md", &["max-rounds", "escalate"]),
        ("docs/JSON.md", &["escalate"]),
        ("docs/SCHEMA.md", &["max-rounds"]),
    ];
    for (doc, words) in cases {
        let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(doc)).unwrap();
        for w in words {
            assert!(text.contains(*w), "{doc} never mentions {w}");
        }
    }
}

/// A board that sets nothing renders and behaves exactly as before: unlisted setting, no
/// `escalate` surprises, same `tb config` output shape as every other card's baseline.
#[test]
fn a_board_that_sets_nothing_is_unchanged() {
    let b = Board::new();
    b.ok(&["add", "widgets: fix it"]);
    assert_eq!(b.json(&["show", "1", "--json"])["escalate"], serde_json::json!(false));
    assert_eq!(
        b.ok(&["config"]),
        "wip           3\ntheme         dark\nlayout        auto\ngithub        off\ngithub-panel  shown\nagents-panel  shown\n"
    );
    assert_eq!(b.json(&["board", "--json"])["v"], 1);
}
