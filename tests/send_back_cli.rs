//! v2: a reviewer sends a REVIEW card back with a reason (CLI). Uses only the v1 store API,
//! so this file also runs against a build without the feature (negative control).
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use terminal_board::store::Store;

fn tb(db: &Path, gh: &Path, who: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(args)
        .env("TB_DB", db)
        .env("TB_GH", gh)
        .env("TB_AS", who)
        .env("TB_NO_HERDR", "1")
        .output()
        .unwrap()
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("not json: {}", String::from_utf8_lossy(&o.stdout)))
}

/// The human line for a REVIEW->TODO send-back says the card is back in TODO,
/// unowned, for anyone to take — printed on plain (non-JSON) output.
#[test]
fn review_to_todo_prints_a_todo_line() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let gh = Path::new("/nonexistent/gh");
    let mut s = Store::open(&db).unwrap();
    let id = in_review(&mut s, "widgets: fix the thing");
    let o = tb(&db, gh, "rev", &["move", &id.to_string(), "todo", "FAIL C3 the widget is still red"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(out.contains(&format!("#{id} is back in TODO, unowned (round r2)")) && out.contains(&format!("is back in TODO, unowned")) && out.contains(&format!("take {id}'")) && out.contains("anyone can take it"), "{out}");
    assert_eq!((s.card(id).unwrap().column.as_str(), s.card(id).unwrap().owner.as_deref()), ("todo", None));
}
/// A card that bot-1 took and moved to REVIEW.
fn in_review(s: &mut Store, title: &str) -> i64 {
    let id = s.add(title, "", &[], "lead").unwrap();
    s.take(id, "bot-1").unwrap();
    assert_eq!(s.done(id, "bot-1").unwrap().column, "review");
    id
}

#[test]
fn send_back_needs_a_reason_keeps_the_owner_and_counts_rounds() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let gh = Path::new("/nonexistent/gh");
    let mut s = Store::open(&db).unwrap();
    let id = in_review(&mut s, "widgets: fix the thing");
    let ids = id.to_string();
    // no reason: refused with a hint, the card stays in review
    let o = tb(&db, gh, "rev", &["move", &ids, "doing", "--json"]);
    assert!(!o.status.success());
    let v = json(&o);
    assert_eq!(v["error"], "say why it goes back");
    assert_eq!(v["hint"], format!("'tb move {id} doing \"what to fix\"'"));
    // #81: a stable machine-readable code alongside the prose — the 2.0.0 refusal for a
    // missing send-back reason
    assert_eq!(v["code"], "reason_required");
    assert_eq!(s.card(id).unwrap().column, "review");
    let o = tb(&db, gh, "rev", &["move", &ids, "doing", "  "]);
    assert!(!o.status.success());
    // with a reason: back in doing, same owner, a `returned` event, round 2
    let o = tb(&db, gh, "rev", &["move", &ids, "doing", "tests fail on empty input", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let c = &json(&o)["card"];
    assert_eq!((c["column"].as_str(), c["owner"].as_str(), c["round"].as_i64()), (Some("doing"), Some("bot-1"), Some(2)));
    let ev = c["events"].as_array().unwrap();
    let last = ev.last().unwrap();
    assert_eq!((last["kind"].as_str(), last["actor"].as_str()), (Some("returned"), Some("rev")));
    assert_eq!(last["text"], "tests fail on empty input");
    // the board shows the round in the card's meta; show and board JSON agree
    let o = tb(&db, gh, "rev", &["list"]);
    let list = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(list.contains("r2"), "{list}");
    let o = tb(&db, gh, "rev", &["show", &ids]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("r2"));
    let b = json(&tb(&db, gh, "rev", &["board", "--json"]));
    assert_eq!(b["columns"]["doing"][0]["round"], 2);
    // a second trip: round 3, counted from events
    assert!(tb(&db, gh, "bot-1", &["done", &ids]).status.success());
    let o = tb(&db, gh, "rev", &["move", &ids, "doing", "still fails", "--json"]);
    assert_eq!(json(&o)["card"]["round"], 3);
    // back to review: a DOING card is its holder's to move (the ownership guard refuses
    // anyone else without --force), so the holder does it
    let o = tb(&db, gh, "rev", &["move", &ids, "review"]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("held by bot-1"));
    let o = tb(&db, gh, "bot-1", &["move", &ids, "review"]);
    assert!(o.status.success());
    // plain text says who has it now
    let o = tb(&db, gh, "rev", &["move", &ids, "doing", "one more thing"]);
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(out.contains("back in doing with bot-1") && out.contains("r4"), "{out}");
    // a never-returned card is round 1
    let other = s.add("plain card", "", &[], "lead").unwrap();
    let o = tb(&db, gh, "rev", &["show", &other.to_string(), "--json"]);
    assert_eq!(json(&o)["round"], 1);
    // a reason on any other move is refused (it is not a note)
    let o = tb(&db, gh, "rev", &["move", &other.to_string(), "doing", "why"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("a reason only goes with sending a REVIEW card back"));
}

/// A verifier FAILs a card: `tb move ID todo "<reason>"` from REVIEW puts it in TODO
/// UNOWNED (someone must take it fresh), with the same `returned` event the
/// review->doing send-back logs, so the reason travels on the card and the round counts.
#[test]
fn fail_to_todo_records_the_reason_and_clears_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let gh = Path::new("/nonexistent/gh");
    let mut s = Store::open(&db).unwrap();
    let id = in_review(&mut s, "widgets: fix the thing");
    let ids = id.to_string();
    let o = tb(&db, gh, "rev", &["move", &ids, "todo", "FAIL C3 the widget is still red", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let c = &json(&o)["card"];
    // unowned, in todo, one rework round counted
    assert_eq!((c["column"].as_str(), c["owner"].as_str(), c["round"].as_i64()), (Some("todo"), None, Some(2)));
    let ev = c["events"].as_array().unwrap();
    let last = ev.last().unwrap();
    assert_eq!((last["kind"].as_str(), last["actor"].as_str()), (Some("returned"), Some("rev")));
    assert_eq!(last["text"], "FAIL C3 the widget is still red");
    // the plain-text view carries the reason
    let o = tb(&db, gh, "rev", &["show", &ids]);
    let shown = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(shown.contains("FAIL C3 the widget is still red") && shown.contains("r2"), "{shown}");
    // TODO again: the fresh owner is whoever takes it
    let o = tb(&db, gh, "bot-1", &["take", &ids, "--json"]);
    assert_eq!(json(&o)["card"]["owner"].as_str(), Some("bot-1"));
    // a second FAIL: round 3, unowned again
    assert!(tb(&db, gh, "bot-1", &["done", &ids]).status.success());
    let o = tb(&db, gh, "rev", &["move", &ids, "todo", "still red", "--json"]);
    let c = &json(&o)["card"];
    assert_eq!((c["column"].as_str(), c["owner"].as_str(), c["round"].as_i64()), (Some("todo"), None, Some(3)));
    // a reason on any other ->todo move is still refused
    let o = tb(&db, gh, "bot-1", &["take", &ids]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = tb(&db, gh, "bot-1", &["move", &ids, "todo", "no reason here"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("a reason only goes with sending a REVIEW card back"));
    assert_eq!((s.card(id).unwrap().column.as_str(), s.card(id).unwrap().owner.as_deref()), ("doing", Some("bot-1")));
}

#[test]
fn send_back_is_not_blocked_by_a_full_doing_column() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let gh = Path::new("/nonexistent/gh");
    let mut s = Store::open(&db).unwrap();
    s.set_wip(1).unwrap();
    let id = in_review(&mut s, "widgets: fix the thing");
    let busy = s.add("other work", "", &[], "lead").unwrap();
    s.take(busy, "bot-2").unwrap();
    // doing is full (1/1): new work is refused …
    let fresh = s.add("new work", "", &[], "lead").unwrap();
    let o = tb(&db, gh, "bot-3", &["take", &fresh.to_string()]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("doing is full (1/1:"));
    // … but a returned card is the owner's existing work
    let o = tb(&db, gh, "rev", &["move", &id.to_string(), "doing", "missing test"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(s.card(id).unwrap().column, "doing");
    assert_eq!(s.card(id).unwrap().owner.as_deref(), Some("bot-1"));
}

fn fake_gh(d: &Path) -> PathBuf {
    let p = d.join("gh-fake");
    std::fs::write(
        &p,
        format!(
            r#"#!/bin/sh
case "$1 $2" in
  "pr list") case "$*" in *merged*) echo '[]';; *) cat {p}/prs.json;; esac;;
  "issue list") cat {p}/issues.json;;
  "run list") echo '[]';;
  api*) echo 1;;
  *) exit 2;;
esac
"#,
            p = d.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

fn write_pr(d: &Path, updated: &str) {
    std::fs::write(
        d.join("prs.json"),
        format!(
            r#"[{{"number":30,"title":"fix","headRefName":"x","isDraft":false,"reviewDecision":"","statusCheckRollup":[],"createdAt":"2026-09-18T08:00:00Z","updatedAt":"{updated}","author":{{"login":"bot"}},"closingIssuesReferences":[{{"number":10}}]}}]"#
        ),
    )
    .unwrap();
}

#[test]
fn sync_leaves_a_returned_card_alone_until_its_pr_is_updated() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let gh = fake_gh(d);
    std::fs::write(d.join("issues.json"), r#"[{"number":10,"title":"bug","labels":[],"assignees":[],"createdAt":"2026-09-18T07:00:00Z"}]"#).unwrap();
    write_pr(d, "2020-01-01T00:00:00Z");
    let db = d.join("b.db");
    let mut s = Store::open(&db).unwrap();
    s.set_github(Some("o/r")).unwrap();
    let id = in_review(&mut s, "gh#10 linked");
    let ids = id.to_string();
    assert!(tb(&db, &gh, "rev", &["move", &ids, "doing", "tests fail"]).status.success());
    // the PR is still open, but not updated since the return: the send-back stands
    let o = tb(&db, &gh, "rev", &["sync", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(json(&o)["moves"], serde_json::json!([]));
    assert_eq!(s.card(id).unwrap().column, "doing");
    // the owner pushes a fix (the PR is updated after the return): sync moves it again
    let later = chrono::DateTime::from_timestamp(terminal_board::store::now() + 86400, 0).unwrap().to_rfc3339();
    write_pr(d, &later);
    let o = tb(&db, &gh, "rev", &["sync", "--json"]);
    let moves = json(&o)["moves"].clone();
    assert_eq!(moves[0]["card_id"], id);
    assert_eq!(moves[0]["to"], "review");
    assert_eq!(s.card(id).unwrap().column, "review");
    // the round survives the trip (it is counted from events)
    let o = tb(&db, &gh, "rev", &["show", &ids, "--json"]);
    assert_eq!(json(&o)["round"], 2);
}
