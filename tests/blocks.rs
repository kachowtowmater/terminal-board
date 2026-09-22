//! Structured blocks: `--on` and `--until`, the derived recheck, auto-unblock on DONE, the
//! WIP discount and the waiting lane.
//!
//! The rules this pins:
//! - plain `tb block ID "text"` behaves exactly as it always did;
//! - `recheck` and the state of the card we wait on are DERIVED at read time (board `tz`,
//!   `TB_NOW`-pinnable) — never stored, never a background clock;
//! - a card blocked `--on #ID` unblocks itself when that card reaches DONE, and only then:
//!   deleting or archiving it leaves the block standing and reports `gone`, and reopening a
//!   finished card does not block anything again;
//! - a blocked card is never handed out by `tb next`, including under `sort due` where it
//!   may be the nearest-due card;
//! - `wip-counts-blocked no` frees a work slot, but never more than `wip` of them, so
//!   blocking everything cannot hand out unlimited work;
//! - the waiting lane is display only: a card is drawn in one place, and JSON, counts,
//!   `tb next` and every command are unaffected.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// 2026-10-01T12:00:00Z
const NOW: i64 = 1_790_856_000;

struct Board {
    _dir: tempfile::TempDir,
    db: PathBuf,
}

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("board.db");
        let b = Board { _dir: dir, db };
        b.ok(&["config", "tz", "UTC"]);
        b
    }

    fn at(&self, now: i64, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("TB_DB", &self.db)
            .env("TB_AS", "alice")
            .env("TB_NO_HERDR", "1")
            .env("TZ", "UTC")
            .env("TB_NOW", now.to_string())
            .env_remove("TB_BOARD")
            .env_remove("HERDR_AGENT_NAME")
            .output()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.at(NOW, args)
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let out = self.ok(args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    fn refused(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(1), "{args:?} should be refused: {}", String::from_utf8_lossy(&o.stdout));
        String::from_utf8_lossy(&o.stderr).trim().to_string()
    }

    /// The derived block fields of a card, which `show`, `list` and `board` must agree on.
    fn block_of(&self, now: i64, id: i64) -> serde_json::Value {
        let pick = |c: &serde_json::Value| {
            serde_json::json!({
                "blocked": c["blocked"], "blocked_on": c["blocked_on"],
                "blocked_until": c["blocked_until"], "recheck": c["recheck"],
                "blocked_on_state": c["blocked_on_state"],
            })
        };
        let show: serde_json::Value = serde_json::from_slice(&self.at(now, &["show", &id.to_string(), "--json"]).stdout).unwrap();
        let list: serde_json::Value = serde_json::from_slice(&self.at(now, &["list", "--json"]).stdout).unwrap();
        let listed = list.as_array().unwrap().iter().find(|c| c["id"] == id).unwrap();
        assert_eq!(pick(listed), pick(&show), "#{id}: list --json disagrees with show --json");
        let board: serde_json::Value = serde_json::from_slice(&self.at(now, &["board", "--json"]).stdout).unwrap();
        let on_board = ["todo", "doing", "review", "done"]
            .iter()
            .flat_map(|c| board["columns"][*c].as_array().unwrap().iter())
            .find(|c| c["id"] == id)
            .unwrap();
        assert_eq!(pick(on_board), pick(&show), "#{id}: board --json disagrees with show --json");
        pick(&show)
    }

    fn column(&self, id: i64) -> String {
        self.json(&["show", &id.to_string(), "--json"])["column"].as_str().unwrap().to_string()
    }
}

fn day(rfc3339: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(rfc3339).unwrap().timestamp()
}

/// Whatever else changes, a plain block is what it always was.
#[test]
fn a_plain_block_behaves_exactly_as_before() {
    let b = Board::new();
    b.ok(&["add", "permits: renewal"]);
    b.ok(&["add", "tax: return"]);
    assert_eq!(b.ok(&["block", "1", "#2"]).trim(), "#1 blocked — clear it with 'tb block 1 --clear'");
    let v = b.block_of(NOW, 1);
    assert_eq!(v["blocked"], "#2");
    assert!(v["blocked_on"].is_null() && v["blocked_until"].is_null() && v["blocked_on_state"].is_null());
    assert_eq!(v["recheck"], false);
    assert_eq!(b.ok(&["list"]), "todo    #1 renewal  [permits - 0m  x blocked by #2]\ntodo    #2 return  [tax - 0m]\n");
    assert!(b.ok(&["show", "1"]).contains("x blocked by #2\n"), "{}", b.ok(&["show", "1"]));
    // `by ` is still stripped, `--clear` still clears, and the events read as before
    b.ok(&["block", "1", "by the other side"]);
    assert_eq!(b.block_of(NOW, 1)["blocked"], "the other side");
    assert_eq!(b.ok(&["block", "1", "--clear"]).trim(), "#1 unblocked");
    assert!(b.block_of(NOW, 1)["blocked"].is_null());
    let kinds: Vec<String> = b.json(&["show", "1", "--json"])["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| format!("{}:{}", e["kind"].as_str().unwrap(), e["text"].as_str().unwrap()))
        .collect();
    assert_eq!(kinds, ["created:", "blocked:by #2", "blocked:by the other side", "unblocked:"]);
    assert!(!b.ok(&["config"]).contains("waiting-lane") && !b.ok(&["config"]).contains("wip-counts-blocked"), "nothing new is listed");
}

#[test]
fn on_and_until_are_stored_shown_and_refused_with_the_command_to_run() {
    let b = Board::new();
    b.ok(&["add", "permits: renewal"]);
    b.ok(&["add", "tax: return"]);
    let out = b.ok(&["block", "1", "waiting for the signed copy", "--on", "#2", "--until", "2026-10-09"]);
    assert_eq!(
        out.trim(),
        "#1 blocked on #2 — it unblocks itself when that card is done; look again on 2026-10-09 — clear it with 'tb block 1 --clear'"
    );
    assert_eq!(
        b.block_of(NOW, 1),
        serde_json::json!({"blocked": "waiting for the signed copy", "blocked_on": "#2", "blocked_until": "2026-10-09", "recheck": false, "blocked_on_state": "open"})
    );
    assert!(b.ok(&["show", "1"]).contains("x blocked by waiting for the signed copy - on #2 · look again 2026-10-09"), "{}", b.ok(&["show", "1"]));
    assert!(b.ok(&["list"]).contains("x blocked by waiting for the signed copy · on #2 · look again 2026-10-09"));
    // a name instead of a card: no state to report, and no auto-unblock to promise
    let out = b.ok(&["block", "2", "their counsel has it", "--on", "the other side", "--until", "2026-09-28"]);
    assert!(out.contains("#2 blocked on the other side; look again on 2026-09-28") && !out.contains("unblocks itself"), "{out}");
    let v = b.block_of(NOW, 2);
    assert_eq!((v["blocked_on"].as_str(), v["blocked_on_state"].as_str()), (Some("the other side"), None));
    assert_eq!(v["recheck"], true, "2026-09-28 has arrived on 2026-10-01");
    // the event records the whole block, so the history says what was promised
    let last = b.json(&["show", "2", "--json"])["events"].as_array().unwrap().last().unwrap().clone();
    assert_eq!((last["kind"].as_str(), last["text"].as_str()), (Some("blocked"), Some("by their counsel has it · on the other side · until 2026-09-28")));
    // refusals, each naming the command to run
    b.ok(&["add", "lease: notice"]);
    for (args, error, hint) in [
        (vec!["block", "3", "x", "--on", "#3"], "#3 cannot wait for itself", "--on #7"),
        (vec!["block", "3", "x", "--on", "#99"], "no card #99 to wait for", "--on NAME"),
        (vec!["block", "3", "x", "--on", "  "], "say who you are waiting on", "--on #7"),
        (vec!["block", "3", "x", "--until", "2026-02-30"], "not a real calendar date", "'tb block 3 \"…\" --until 2026-10-09'"),
        (vec!["block", "3", "x", "--until", "friday"], "not a date", "'tb block 3 \"…\" --until 2026-10-09'"),
        (vec!["block", "3", "x", "--until", "none"], "'--until none' says nothing", "'tb block 3 --clear'"),
    ] {
        let mut j = args.clone();
        j.push("--json");
        let o = b.run(&j);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["ok"], false, "{args:?}");
        assert!(v["error"].as_str().unwrap().contains(error), "{args:?}: {v}");
        assert!(v["hint"].as_str().unwrap().contains(hint), "{args:?}: {v}");
        assert!(b.block_of(NOW, 3)["blocked"].is_null(), "{args:?} wrote something");
    }
    // `--on` is bounded, sanitised and cannot be given with --clear
    let long = b.refused(&["block", "3", "x", "--on", &"n".repeat(41)]);
    assert!(long.contains("the limit is 40"), "{long}");
    b.ok(&["block", "3", "x", "--on", "the \x1b[31mother   side"]);
    assert_eq!(b.block_of(NOW, 3)["blocked_on"], "the other side");
    let o = b.run(&["block", "3", "--clear", "--on", "#2"]);
    assert_eq!(o.status.code(), Some(2), "--clear and --on together is an argument error");
}

/// `recheck` is derived from the board's today, at read time. Nothing is stored, and no
/// clock runs: the same file read at two instants answers differently.
#[test]
fn recheck_is_derived_at_read_time_from_the_boards_today() {
    let b = Board::new();
    b.ok(&["add", "permits: renewal"]);
    b.ok(&["block", "1", "waiting", "--until", "2026-10-09"]);
    for (at, want) in [
        ("2026-10-07T23:59:59Z", false),
        ("2026-10-08T23:59:59Z", false),
        ("2026-10-09T00:00:00Z", true),
        ("2026-10-09T23:59:59Z", true),
        ("2027-01-01T00:00:00Z", true),
    ] {
        assert_eq!(b.block_of(day(at), 1)["recheck"], want, "at {at}");
    }
    // nothing about the card changed: the answer is computed, not written
    let raw: (Option<String>, Option<String>) = rusqlite::Connection::open(&b.db)
        .unwrap()
        .query_row("SELECT blocked_until, blocked_on FROM cards WHERE id=1", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(raw, (Some("2026-10-09".to_string()), None));
    assert_eq!(b.json(&["show", "1", "--json"])["events"].as_array().unwrap().len(), 2, "no event was written by a read");
    // the board's own zone decides the day, not the reader's
    b.ok(&["config", "tz", "Pacific/Auckland"]);
    assert_eq!(b.block_of(day("2026-10-08T10:59:59Z"), 1)["recheck"], false, "23:59 on the 8th in Auckland");
    assert_eq!(b.block_of(day("2026-10-08T11:00:00Z"), 1)["recheck"], true, "midnight in Auckland, 13 hours before UTC");
    // the plain word changes with it
    b.ok(&["config", "tz", "UTC"]);
    assert!(String::from_utf8_lossy(&b.at(day("2026-10-08T00:00:00Z"), &["show", "1"]).stdout).contains("look again 2026-10-09"));
    assert!(String::from_utf8_lossy(&b.at(day("2026-10-09T00:00:00Z"), &["show", "1"]).stdout).contains("recheck 2026-10-09"));
}

/// A5: a card blocked on another card unblocks itself when that card reaches DONE — and only
/// then. The three neighbouring cases are decided and stated here.
#[test]
fn auto_unblock_on_done_and_what_the_other_cases_do() {
    // (1) DONE unblocks, in the same breath as the move, whoever finishes it
    let b = Board::new();
    for t in ["permits: renewal", "tax: return", "lease: notice"] {
        b.ok(&["add", t]);
    }
    b.ok(&["block", "1", "waiting for the return", "--on", "#2", "--until", "2026-12-01"]);
    b.ok(&["block", "3", "waiting for the return too", "--on", "#2"]);
    // a card waiting on something else entirely must be left exactly as it is
    b.ok(&["add", "audit: the letter"]);
    b.ok(&["block", "4", "their counsel has it", "--on", "the other side", "--until", "2026-12-24"]);
    b.ok(&["take", "2", "--as", "bob"]);
    b.ok(&["done", "2", "--as", "bob"]);
    assert_eq!(b.block_of(NOW, 1)["blocked_on_state"], "open", "review is not done");
    b.ok(&["done", "2", "--as", "carol"]);
    for id in [1, 3] {
        let v = b.block_of(NOW, id);
        assert!(v["blocked"].is_null() && v["blocked_on"].is_null() && v["blocked_until"].is_null(), "#{id}: {v}");
        let last = b.json(&["show", &id.to_string(), "--json"])["events"].as_array().unwrap().last().unwrap().clone();
        assert_eq!((last["kind"].as_str(), last["text"].as_str()), (Some("unblocked"), Some("#2 is done")), "#{id}");
        assert_eq!(last["actor"], "carol", "the unblock is recorded against whoever finished the card");
    }
    let untouched = b.block_of(NOW, 4);
    assert_eq!(
        untouched,
        serde_json::json!({"blocked": "their counsel has it", "blocked_on": "the other side", "blocked_until": "2026-12-24", "recheck": false, "blocked_on_state": null}),
        "a card waiting on something else was touched"
    );
    assert_eq!(b.json(&["show", "4", "--json"])["events"].as_array().unwrap().len(), 2, "#4 got no event");
    // (2) reopening the finished card does NOT block anything again
    b.ok(&["move", "2", "todo"]);
    assert!(b.block_of(NOW, 1)["blocked"].is_null(), "a reopened card does not re-block what it freed");
    // (3) a card that is already done cannot be waited on: nothing would ever lift it
    b.ok(&["take", "2"]);
    b.ok(&["done", "2"]);
    b.ok(&["done", "2", "--as", "carol"]);
    let e = b.refused(&["block", "1", "x", "--on", "#2"]);
    assert!(e.contains("#2 is already done, so nothing would lift that block"), "{e}");
    // (4) DELETED: the block stands and says the card is gone — tb does not decide for you
    let b = Board::new();
    b.ok(&["add", "permits: renewal"]);
    b.ok(&["add", "tax: return"]);
    b.ok(&["block", "1", "waiting", "--on", "#2"]);
    b.ok(&["rm", "2"]);
    let v = b.block_of(NOW, 1);
    assert_eq!((v["blocked"].as_str(), v["blocked_on"].as_str(), v["blocked_on_state"].as_str()), (Some("waiting"), Some("#2"), Some("gone")));
    assert!(b.ok(&["show", "1"]).contains("on #2 (gone)"), "{}", b.ok(&["show", "1"]));
    b.ok(&["block", "1", "--clear"]);
    // (5) ARCHIVED (soft delete): the same — gone, and restoring it brings the waiting back
    let b = Board::new();
    b.ok(&["add", "permits: renewal"]);
    b.ok(&["add", "tax: return"]);
    b.ok(&["config", "rm", "archive"]);
    b.ok(&["block", "1", "waiting", "--on", "#2"]);
    b.ok(&["rm", "2"]);
    assert_eq!(b.block_of(NOW, 1)["blocked_on_state"], "gone", "an archived card is gone from the board");
    b.ok(&["restore", "2"]);
    assert_eq!(b.block_of(NOW, 1)["blocked_on_state"], "open", "restored: we are waiting on it again");
    b.ok(&["take", "2"]);
    b.ok(&["done", "2"]);
    b.ok(&["done", "2", "--as", "carol"]);
    assert!(b.block_of(NOW, 1)["blocked"].is_null(), "and finishing it still unblocks");
}

/// A blocked card is never handed out, whatever the board sorts by.
#[test]
fn tb_next_never_hands_out_a_blocked_card_even_when_it_is_the_nearest_due() {
    let b = Board::new();
    b.ok(&["config", "sort", "due"]);
    b.ok(&["add", "permits: the nearest", "--due", "2026-10-02"]);
    b.ok(&["add", "tax: later", "--due", "2026-11-30"]);
    b.ok(&["block", "1", "waiting for the fee", "--on", "the other side", "--until", "2026-10-03"]);
    assert_eq!(b.json(&["board", "--json"])["columns"]["todo"][0]["id"], 1, "it is still first on the board");
    assert_eq!(b.json(&["next", "--json"])["card"]["id"], 2, "but tb next skips it");
    let o = b.run(&["next", "--as", "bob"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("no todo cards"), "{}", String::from_utf8_lossy(&o.stderr));
    // the same under the default sort
    b.ok(&["config", "sort", "position"]);
    let o = b.run(&["next", "--as", "bob"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("no todo cards"));
    // unblock it and it is next
    b.ok(&["block", "1", "--clear"]);
    assert_eq!(b.json(&["next", "--json", "--as", "bob"])["card"]["id"], 1);
}

/// A8: a blocked card frees a work slot — but only up to `wip` of them, so blocking
/// everything can never hand out unlimited work.
#[test]
fn wip_counts_blocked_no_frees_a_slot_but_is_capped_at_the_limit() {
    let b = Board::new();
    b.ok(&["config", "wip", "2"]);
    for i in 1..=9 {
        b.ok(&["add", &format!("card {i}")]);
    }
    assert_eq!(b.ok(&["config", "wip-counts-blocked"]).trim(), "yes", "the default is today's behaviour");
    b.ok(&["take", "1", "--as", "a1"]);
    b.ok(&["take", "2", "--as", "a2"]);
    b.ok(&["block", "1", "waiting", "--as", "a1"]);
    // by default a blocked card still uses its slot
    let e = b.refused(&["take", "3", "--as", "a3"]);
    assert!(e.contains("doing is full (2/2"), "{e}");
    // with the discount it frees one
    assert_eq!(b.json(&["config", "wip-counts-blocked", "no", "--json"])["config"]["value"], "no");
    b.ok(&["take", "3", "--as", "a3"]);
    assert_eq!(b.json(&["board", "--json"])["columns"]["doing"].as_array().unwrap().len(), 3);
    // block another and a fourth fits (2 blocked, 2 counted)
    b.ok(&["block", "2", "waiting", "--as", "a2"]);
    b.ok(&["take", "4", "--as", "a4"]);
    // now the cap bites: 3 blocked cards, only `wip` (2) are discounted
    b.ok(&["block", "3", "waiting", "--as", "a3"]);
    let e = b.refused(&["take", "5", "--as", "a5"]);
    assert!(e.contains("doing is full (4/2"), "the cap is the WIP limit itself: {e}");
    // blocking every card in hand cannot get past it either
    b.ok(&["block", "4", "waiting", "--as", "a4"]);
    let e = b.refused(&["take", "5", "--as", "a5"]);
    assert!(e.contains("doing is full (4/2"), "{e}");
    assert_eq!(b.json(&["board", "--json"])["columns"]["doing"].as_array().unwrap().len(), 4, "DOING never passes twice the limit");
    // unblocking gives the slot back to real work
    b.ok(&["block", "1", "--clear", "--as", "a1"]);
    b.ok(&["done", "1", "--as", "a1"]);
    b.ok(&["take", "5", "--as", "a5"]);
    // back to yes: every DOING card counts again
    b.ok(&["config", "wip-counts-blocked", "yes"]);
    let e = b.refused(&["take", "6", "--as", "a6"]);
    assert!(e.contains("doing is full"), "{e}");
    for bad in ["maybe", "", "1"] {
        let e = b.refused(&["config", "wip-counts-blocked", bad]);
        assert!(e.contains("is not yes|no") && e.contains("'tb config wip-counts-blocked no'"), "{bad:?}: {e}");
    }
}

/// C7: the waiting lane is display only. A card is drawn in ONE place, the counts stay true,
/// and JSON, `tb next` and every command are untouched.
#[test]
fn the_waiting_lane_is_display_only_and_never_double_counts() {
    let b = Board::new();
    for t in ["permits: renewal", "tax: return", "lease: notice", "audit: letter"] {
        b.ok(&["add", t]);
    }
    b.ok(&["take", "4", "--as", "bob"]);
    b.ok(&["block", "1", "waiting for the signed copy", "--on", "#2", "--until", "2026-09-28"]);
    b.ok(&["block", "4", "their counsel has it", "--on", "the other side", "--as", "bob"]);
    let before_json = b.json(&["board", "--json"]);
    let before_list = b.ok(&["list"]);
    let plain_before = b.ok(&["board"]);
    assert!(!plain_before.contains("WAITING"), "no lane by default");
    assert_eq!(b.ok(&["config", "waiting-lane"]).trim(), "hidden");

    b.ok(&["config", "waiting-lane", "shown"]);
    let plain = b.ok(&["board"]);
    // every blocked card is in the lane, with its block text, and NOT in its column
    let lane = plain.split("WAITING (2)").nth(1).expect("a WAITING section");
    assert!(lane.contains("#1 renewal") && lane.contains("#4 letter"), "{plain}");
    assert!(lane.contains("x blocked by waiting for the signed copy") && lane.contains("on #2 · recheck 2026-09-28"), "{plain}");
    let columns = plain.split("WAITING (2)").next().unwrap();
    assert!(!columns.contains("#1 renewal") && !columns.contains("#4 letter"), "drawn twice:\n{plain}");
    assert!(columns.contains("#2 return") && columns.contains("#3 notice"));
    // the counts are the REAL counts, and say how many are elsewhere
    assert!(columns.contains("TODO (3) · 1 waiting"), "{plain}");
    assert!(columns.contains("DOING (1/3) · 1 waiting"), "{plain}");
    // and nothing else moved: JSON, list and every command are as they were
    assert_eq!(b.json(&["board", "--json"]), before_json, "the lane changed the JSON");
    assert_eq!(b.ok(&["list"]), before_list, "the lane changed tb list");
    assert_eq!(b.json(&["next", "--json", "--as", "carol"])["card"]["id"], 2, "tb next is unaffected");
    assert_eq!(b.column(1), "todo", "the card never left its column");
    // a done card is never in the lane, even blocked before it finished
    b.ok(&["move", "3", "done"]);
    b.ok(&["block", "3", "stale", "--as", "alice"]);
    assert_eq!(b.ok(&["board"]).matches("WAITING (2)").count(), 1, "the done card is not in the lane");
    b.ok(&["config", "waiting-lane", "hidden"]);
    let back = b.ok(&["board"]);
    assert!(!back.contains("WAITING (") && !back.contains(" waiting\n"), "hidden leaves no trace of the lane:\n{back}");
    let todo = back.split("TODO (").nth(1).unwrap();
    assert!(todo.starts_with("1)") && todo.contains("#1 renewal"), "the blocked card is back in its column:\n{back}");
    assert!(back.contains("DOING (2/3)") && back.split("DOING (2/3)").nth(1).unwrap().contains("#4 letter"), "{back}");
    let _ = &plain_before;
    for bad in ["maybe", ""] {
        let e = b.refused(&["config", "waiting-lane", bad]);
        assert!(e.contains("is not shown|hidden") && e.contains("'tb config waiting-lane shown'"), "{bad:?}: {e}");
    }
}

/// A board that sets nothing and blocks nothing structurally is what it always was.
#[test]
fn a_board_that_sets_nothing_is_unchanged() {
    let b = Board::new();
    b.ok(&["add", "widgets: gh#7 fix it", "-d", "Done = fixed", "--check", "repro"]);
    b.ok(&["add", "ops: rotate tokens"]);
    b.ok(&["block", "2", "#1"]);
    assert_eq!(
        b.ok(&["config"]),
        "wip           3\ntheme         dark\nlayout        auto\ngithub        off\ngithub-panel  shown\nagents-panel  shown\ntz            UTC\n",
        "only the tz this fixture sets"
    );
    assert_eq!(b.ok(&["list"]), "todo    #1 gh#7 fix it  [widgets - 0m - 0/1]\ntodo    #2 rotate tokens  [ops - 0m  x blocked by #1]\n");
    let card = b.json(&["show", "2", "--json"]);
    assert!(card["blocked_on"].is_null() && card["blocked_until"].is_null() && card["blocked_on_state"].is_null());
    assert_eq!(card["recheck"], false);
    assert_eq!(b.json(&["board", "--json"])["v"], 1);
    assert!(!b.ok(&["board"]).contains("WAITING") && !b.ok(&["board"]).contains("waiting"));
}

/// The columns arrived by migration on a board written by the previous version.
#[test]
fn an_older_board_file_gains_the_columns_and_keeps_its_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("old.db");
    {
        // a v2.0-shaped cards table: no blocked_on / blocked_until
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute_batch(
            r#"CREATE TABLE cards (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL, tag TEXT,
                 description TEXT NOT NULL DEFAULT '', "column" TEXT NOT NULL DEFAULT 'todo', owner TEXT, due TEXT,
                 gh_ref INTEGER, created_at INTEGER NOT NULL, column_since INTEGER NOT NULL, blocked TEXT,
                 position INTEGER NOT NULL DEFAULT 0, reviewer TEXT);
               INSERT INTO cards(id, title, created_at, column_since, blocked) VALUES (1, 'old card', 1, 1, '#9');"#,
        )
        .unwrap();
    }
    let b = Board { _dir: dir, db };
    let v = b.json(&["show", "1", "--json"]);
    assert_eq!(v["blocked"], "#9", "the old block is untouched");
    assert!(v["blocked_on"].is_null() && v["blocked_until"].is_null());
    assert_eq!(v["recheck"], false);
    b.ok(&["block", "1", "waiting", "--on", "the other side", "--until", "2026-10-09"]);
    assert_eq!(b.block_of(NOW, 1)["blocked_on"], "the other side");
}

/// docs/SCHEMA.md documents every column (tests/schema.rs enforces it); this pins the two
/// new ones and the event text, which agents read.
#[test]
fn the_new_columns_and_the_block_event_are_documented() {
    let md = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/SCHEMA.md")).unwrap();
    for want in ["`blocked_on`", "`blocked_until`"] {
        assert!(md.contains(want), "docs/SCHEMA.md does not document {want}");
    }
}
