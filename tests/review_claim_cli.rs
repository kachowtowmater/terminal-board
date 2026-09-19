//! `tb next --review` through the binary only, so this file builds against any tb (a
//! negative control on a tree without the flag fails by assertion, not by compile error).
use std::path::Path;
use std::process::{Command, Output};

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

/// A card `bot-1` took and sent to REVIEW, through the CLI.
fn cli_in_review(db: &Path, title: &str) -> i64 {
    let o = tb(db, "bot-1", &["add", title, "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let id = v["card"]["id"].as_i64().unwrap();
    assert!(tb(db, "bot-1", &["take", &id.to_string()]).status.success());
    assert!(tb(db, "bot-1", &["done", &id.to_string()]).status.success());
    id
}

fn cli_json(db: &Path, who: &str, args: &[&str]) -> (bool, serde_json::Value) {
    let o = tb(db, who, args);
    (o.status.success(), serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null))
}

/// Only the CLI, so it builds against any tb: two reviewers, the author refused, drop and
/// take clear the claim.
#[test]
fn cli_two_reviewers_author_refused_and_claims_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let one = cli_in_review(&db, "first");
    let two = cli_in_review(&db, "second");
    let (ok, v) = cli_json(&db, "bot-1", &["next", "--review", "--json"]);
    assert!(!ok && v["ok"] == false, "the author is refused: {v}");
    assert!(v["error"].as_str().unwrap_or("").contains("no review cards for you"), "{v}");
    let (ok, v) = cli_json(&db, "rev-1", &["next", "--review", "--json"]);
    assert!(ok, "rev-1 claims: {v}");
    assert_eq!((v["card"]["id"].as_i64(), v["card"]["reviewer"].as_str()), (Some(one), Some("rev-1")));
    let (ok, v) = cli_json(&db, "rev-2", &["next", "--review", "--json"]);
    assert!(ok, "rev-2 claims the other card: {v}");
    assert_eq!((v["card"]["id"].as_i64(), v["card"]["reviewer"].as_str()), (Some(two), Some("rev-2")));
    // the plain claim text tells the reviewer how to send the card back (since the send-back
    // rule: `move N doing "why"`, never `move N todo`)
    let three = cli_in_review(&db, "third");
    let o = tb(&db, "rev-4", &["next", "--review"]);
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(text.contains(&format!("tb move {three} doing \"what is missing\"")), "{text}");
    assert!(!text.contains(&format!("tb move {three} todo")), "{text}");
    let (ok, v) = cli_json(&db, "rev-3", &["next", "--review", "--json"]);
    assert!(!ok && v["error"] == "no review cards waiting", "{v}");
    // drop clears the claim, and so does the next take
    assert!(tb(&db, "rev-1", &["drop", &one.to_string()]).status.success());
    let (_, v) = cli_json(&db, "x", &["show", &one.to_string(), "--json"]);
    assert_eq!((v["column"].as_str(), v["reviewer"].as_str()), (Some("todo"), None), "{v}");
    assert!(tb(&db, "bot-2", &["take", &one.to_string()]).status.success());
    let (_, v) = cli_json(&db, "x", &["show", &one.to_string(), "--json"]);
    assert_eq!((v["owner"].as_str(), v["reviewer"].as_str()), (Some("bot-2"), None), "{v}");
    let list = String::from_utf8_lossy(&tb(&db, "x", &["list"]).stdout).into_owned();
    assert!(!list.contains("review rev-1"), "no stale reviewer on the board:\n{list}");
}
