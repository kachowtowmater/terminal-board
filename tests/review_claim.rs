//! v2: reviewers claim REVIEW cards atomically with `tb next --review`.
use std::path::Path;
use std::process::{Command, Output};
use terminal_board::store::Store;

fn tb(db: &Path, who: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(args)
        .env("TB_DB", db)
        .env("TB_GH", "/nonexistent/gh")
        .env("TB_AS", who)
        .env("TB_NO_HERDR", "1")
        .output()
        .unwrap()
}

/// A card that `worker` took and sent to REVIEW.
fn in_review(s: &mut Store, title: &str, worker: &str) -> i64 {
    let id = s.add(title, "", &[], "lead").unwrap();
    s.take(id, worker).unwrap();
    assert_eq!(s.done(id, worker).unwrap().column, "review");
    id
}

#[test]
fn claims_the_top_card_the_reviewer_did_not_author() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let mine = in_review(&mut s, "mine", "rev-1");
    let theirs = in_review(&mut s, "theirs", "bot-1");
    // the author is refused its own card (case-insensitively) and gets the next one
    let c = s.next_review("REV-1").unwrap();
    assert_eq!((c.id, c.reviewer.as_deref()), (theirs, Some("REV-1")));
    assert_eq!(c.column, "review", "claiming does not move the card");
    assert!(s.show(theirs).unwrap().events.iter().any(|e| e.kind == "reviewing" && e.actor == "REV-1"));
    // only own work (and claimed cards) left: a clear refusal
    let e = s.next_review("rev-1").unwrap_err().to_string();
    assert!(e.contains("the one waiting is your own work") && e.contains("ask another person or agent to review it"), "{e}");
    assert_eq!(s.card(mine).unwrap().reviewer, None);
    // another reviewer gets the author's card; claimed cards are not handed out twice
    assert_eq!(s.next_review("rev-2").unwrap().id, mine);
    assert!(s.next_review("bot-1").unwrap_err().to_string().contains("no review cards waiting"));
    // nothing in review at all
    let dir2 = tempfile::tempdir().unwrap();
    let mut empty = Store::open(&dir2.path().join("b.db")).unwrap();
    assert!(empty.next_review("rev-1").unwrap_err().to_string().contains("no review cards waiting"));
}

#[test]
fn blocked_cards_are_skipped_and_reviewer_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let a = in_review(&mut s, "a", "bot-1");
    let b = in_review(&mut s, "b", "bot-1");
    s.block(a, Some("#9"), "lead").unwrap();
    assert_eq!(s.next_review("rev-1").unwrap().id, b);
    // sent back: the claim ends; the next round is claimed afresh
    s.send_back(b, "missing a test", "rev-1").unwrap();
    assert_eq!(s.card(b).unwrap().reviewer, None);
    s.done(b, "bot-1").unwrap();
    assert_eq!(s.next_review("rev-2").unwrap().reviewer.as_deref(), Some("rev-2"));
    // a stale claim is released by moving the card to review again (logged)
    let c = in_review(&mut s, "c", "bot-1");
    assert_eq!(s.next_review("rev-3").unwrap().id, c);
    assert_eq!(s.move_to(c, "review", "lead").unwrap().reviewer, None);
    assert!(s.show(c).unwrap().events.iter().any(|e| e.kind == "unclaimed" && e.text == "rev-3"));
    assert_eq!(s.next_review("rev-4").unwrap().id, c);
    // a send-back with a reason ends the claim too, and the next round is claimed afresh
    let d = in_review(&mut s, "d", "bot-1");
    assert_eq!(s.next_review("rev-5").unwrap().id, d);
    let back = s.send_back(d, "add the rollback step", "rev-5").unwrap();
    assert_eq!((back.column.as_str(), back.reviewer.as_deref()), ("doing", None));
    s.done(d, "bot-1").unwrap();
    assert_eq!(s.next_review("rev-6").unwrap().id, d);
    // approved: the reviewer stays on the DONE card
    assert_eq!(s.done(b, "rev-2").unwrap().reviewer.as_deref(), Some("rev-2"));
}

#[test]
fn two_concurrent_reviewers_never_share_a_card() {
    for round in 0..10 {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("b.db");
        let mut s = Store::open(&db).unwrap();
        let one = in_review(&mut s, "only card", "bot-1");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let got: Vec<_> = ["rev-1", "rev-2"]
            .into_iter()
            .map(|who| {
                let (db, barrier) = (db.clone(), barrier.clone());
                std::thread::spawn(move || {
                    let mut s = Store::open(&db).unwrap();
                    barrier.wait();
                    s.next_review(who).map(|c| c.id).map_err(|e| e.to_string())
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect();
        let wins: Vec<_> = got.iter().filter_map(|r| r.as_ref().ok()).collect();
        assert_eq!(wins, [&one], "round {round}: exactly one reviewer gets the card: {got:?}");
        // with two cards, both reviewers get one each, never the same
        in_review(&mut s, "second card", "bot-1");
        let loser = if got[0].is_err() { "rev-1" } else { "rev-2" };
        let c = s.next_review(loser).unwrap();
        assert_ne!(c.id, one);
    }
}

#[test]
fn cli_flag_json_and_display() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let mut s = Store::open(&db).unwrap();
    let id = in_review(&mut s, "widgets: fix the thing", "bot-1");
    // the author is refused on the CLI, with a hint
    let o = tb(&db, "bot-1", &["next", "--review", "--json"]);
    assert!(!o.status.success());
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert!(v["hint"].as_str().unwrap().contains("tb next"), "{v}");
    // another agent claims it
    let o = tb(&db, "rev-1", &["next", "--review", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!((v["card"]["id"].as_i64(), v["card"]["reviewer"].as_str()), (Some(id), Some("rev-1")));
    assert_eq!(v["card"]["column"], "review");
    // plain list and show put the reviewer next to the owner
    let list = String::from_utf8_lossy(&tb(&db, "x", &["list"]).stdout).into_owned();
    assert!(list.contains("bot-1 - review rev-1"), "{list}");
    let show = String::from_utf8_lossy(&tb(&db, "x", &["show", &id.to_string()]).stdout).into_owned();
    assert!(show.contains("bot-1 - review rev-1"), "{show}");
    // human output tells the reviewer what to do next
    let id2 = in_review(&mut s, "second", "bot-1");
    let out = String::from_utf8_lossy(&tb(&db, "rev-2", &["next", "--review"]).stdout).into_owned();
    assert!(out.contains("reviewing by rev-2") && out.contains(&format!("tb done {id2}")), "{out}");
    // plain `tb next` is unchanged: review cards are not todo work
    assert!(!tb(&db, "rev-3", &["next"]).status.success());
}

#[test]
fn old_boards_gain_the_column_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("old.db");
    {
        let s = Store::open(&db).unwrap();
        s.add("x", "", &[], "lead").unwrap();
    }
    let c = rusqlite::Connection::open(&db).unwrap();
    c.execute_batch("ALTER TABLE cards DROP COLUMN reviewer").unwrap();
    drop(c);
    let s = Store::open(&db).unwrap();
    assert_eq!(s.card(1).unwrap().reviewer, None);
}
