//! GOLDEN tests for the JSON contract (docs/JSON.md): exact field names of every shape,
//! plus `watch --json` streaming a new line after a CLI write.
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use terminal_board::contract;
use terminal_board::herdr::parse_agents;
use terminal_board::store::Store;

fn keys(v: &serde_json::Value) -> Vec<String> {
    v.as_object().unwrap_or_else(|| panic!("not an object: {v}")).keys().cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

fn sorted(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>().into_iter().collect()
}

fn tb(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(args)
        .env("TB_DB", db)
        .env("TB_AS", "tester")
        .env("TB_NO_HERDR", "1")
        .env("TB_GH", "/nonexistent/gh")
        .output()
        .unwrap()
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&o.stdout)))
}

const CARD: &[&str] = &[
    "id", "title", "tag", "description", "column", "position", "owner", "due", "gh_ref", "blocked", "created_at",
    "column_since", "checklist", "events",
];

#[test]
fn golden_board_shape() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    tb(&db, &["add", "widgets: gh#7 fix it", "--check", "repro"]);
    tb(&db, &["config", "github", "o/r"]);
    let v = json(&tb(&db, &["board", "--json"]));
    assert_eq!(keys(&v), sorted(&["v", "board", "wip", "theme", "layout", "github", "columns"]));
    assert_eq!(v["v"], 1);
    assert_eq!((v["wip"].as_i64(), v["theme"].as_str(), v["layout"].as_str()), (Some(3), Some("dark"), Some("auto")));
    assert_eq!(keys(&v["github"]), sorted(&["repo", "snapshot", "error"]));
    assert_eq!(v["github"]["repo"], "o/r");
    assert!(v["github"]["snapshot"].is_null());
    assert_eq!(keys(&v["columns"]), sorted(&["todo", "doing", "review", "done"]));
    let card = &v["columns"]["todo"][0];
    assert_eq!(keys(card), sorted(CARD));
    assert_eq!((card["tag"].as_str(), card["gh_ref"].as_i64(), card["position"].as_i64()), (Some("widgets"), Some(7), Some(0)));
    assert_eq!(keys(&card["checklist"][0]), sorted(&["n", "idx", "text", "done"]));
    assert_eq!(keys(&card["events"][0]), sorted(&["ts", "actor", "kind", "text"]));
    assert!(card["created_at"].is_i64() && card["events"][0]["ts"].is_i64(), "unix seconds");
    // bare `tb --json` (not a TTY) prints the same object
    let bare = json(&tb(&db, &["--json"]));
    assert_eq!(keys(&bare), keys(&v));
}

#[test]
fn golden_card_events_capped_at_ten() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    tb(&db, &["add", "busy"]);
    for i in 0..12 {
        tb(&db, &["note", "1", &format!("note {i}")]);
    }
    let v = json(&tb(&db, &["board", "--json"]));
    let ev = v["columns"]["todo"][0]["events"].as_array().unwrap();
    assert_eq!(ev.len(), 10);
    assert_eq!(ev.last().unwrap()["text"], "note 11", "the last 10, oldest first");
}

#[test]
fn golden_write_results_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let writes: Vec<Vec<&str>> = vec![
        vec!["add", "a card"],
        vec!["add", "second"],
        vec!["note", "1", "hi"],
        vec!["check", "1", "--add", "item"],
        vec!["check", "1", "1"],
        vec!["edit", "1", "--desc", "d"],
        vec!["prio", "2", "top"],
        vec!["block", "2", "#9"],
        vec!["block", "2", "--clear"],
        vec!["take", "1"],
        vec!["done", "1"],
        vec!["move", "1", "doing"],
        vec!["drop", "1"],
        vec!["next"],
        vec!["rm", "2"],
    ];
    for w in writes {
        let mut args = w.clone();
        args.push("--json");
        let o = tb(&db, &args);
        assert!(o.status.success(), "{w:?}: {}", String::from_utf8_lossy(&o.stdout));
        let v = json(&o);
        assert_eq!(keys(&v), sorted(&["ok", "card"]), "{w:?}");
        assert_eq!(v["ok"], true);
        assert_eq!(keys(&v["card"]), sorted(CARD), "{w:?}");
    }
    let v = json(&tb(&db, &["config", "wip", "4", "--json"]));
    assert_eq!(keys(&v), sorted(&["ok", "config"]));
    assert_eq!(keys(&v["config"]), sorted(&["key", "value"]));
    // errors: ok=false, error, hint, non-zero exit
    for bad in [vec!["show", "99"], vec!["rm", "99"], vec!["done", "99"], vec!["prio", "1", "sideways"], vec!["config", "nope", "1"]] {
        let mut args = bad.clone();
        args.push("--json");
        let o = tb(&db, &args);
        assert!(!o.status.success(), "{bad:?}");
        let v = json(&o);
        assert_eq!(keys(&v), sorted(&["ok", "error", "hint"]), "{bad:?}");
        assert_eq!(v["ok"], false);
        assert!(v["hint"].as_str().unwrap().contains("tb "), "hint names a command: {v}");
    }
    let v = json(&tb(&db, &["rm", "99", "--json"]));
    assert!(v["hint"].as_str().unwrap().contains("tb list"));
}

#[test]
fn golden_agents_shape() {
    let agents = parse_agents(
        r#"{"result":{"agents":[{"name":"bot","agent":"aider","agent_status":"working","pane_id":"w:p1"}]}}"#,
        Some(r#"{"result":{"panes":[{"agent":"aider","label":"W1 · x · aider · fix #1","pane_id":"w:p1"}]}}"#),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = s.add("x", "", &[], "me").unwrap();
    s.take(id, "bot").unwrap();
    let v = serde_json::to_value(contract::agents(&agents, &s.list().unwrap())).unwrap();
    assert_eq!(keys(&v[0]), sorted(&["name", "harness", "status", "pane_id", "job", "card_id"]));
    assert_eq!((v[0]["card_id"].as_i64(), v[0]["job"].as_str()), (Some(id), Some("fix #1")));
    // CLI: an array (empty without herdr)
    let o = tb(&dir.path().join("b.db"), &["agents", "--json"]);
    assert!(json(&o).as_array().is_some());
}

#[test]
fn watch_streams_a_new_object_after_a_write() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    tb(&db, &["add", "first"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(["watch", "--json"])
        .env("TB_DB", &db)
        .env("TB_NO_HERDR", "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let first: serde_json::Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(first["v"], 1);
    assert_eq!(first["columns"]["todo"].as_array().unwrap().len(), 1);
    assert!(tb(&db, &["add", "second"]).status.success());
    let second: serde_json::Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(second["columns"]["todo"].as_array().unwrap().len(), 2, "a new full object after the write");
    child.kill().unwrap();
    child.wait().unwrap();
    // closing the reader ends watch cleanly (no panic, exit 0)
    let mut child = Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(["watch", "--json"])
        .env("TB_DB", &db)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut l = String::new();
    out.read_line(&mut l).unwrap();
    drop(out);
    tb(&db, &["add", "third"]); // a change makes watch write into the closed pipe
    let status = child.wait().unwrap();
    assert!(status.success(), "exits cleanly on a closed stdout: {status:?}");
}

#[test]
fn consumers_must_tolerate_unknown_fields_and_kinds() {
    // docs/JSON.md: "consumers must ignore unknown fields and unknown event kinds".
    // Pinned here: the shapes that today's readers rely on keep working when a future tb
    // adds a field to a card/event and a kind nobody has seen.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let mut s = Store::open(&db).unwrap();
    let id = s.add("widgets: future proof", "", &[], "me").unwrap();
    s.take(id, "me").unwrap();
    // an event of a kind that does not exist yet (as a future tb would record)
    let _ = s.note_kind(id, "future-agent", "an unknown kind", "scrying");
    let card = contract::card_by_id(&s, id).unwrap();
    // the golden reader contract: field names are stable; an unknown KIND is still a well-
    // formed {ts, actor, kind, text} object, so a consumer that dispatches on known kinds
    // and ignores the rest keeps working.
    let known_kinds = ["created", "taken", "moved", "note", "check", "blocked", "unblocked", "dropped", "edit", "prio", "github"];
    let bad: Vec<_> = card.events.iter().filter(|e| !known_kinds.contains(&e.kind.as_str())).collect();
    assert_eq!(bad.len(), 1, "the unknown-kind event is carried through: {bad:?}");
    assert_eq!(bad[0].kind, "scrying");
    assert!(bad[0].ts > 0 && !bad[0].actor.is_empty(), "unknown kinds keep the event shape");
    // an unknown FIELD (a future card with an extra key) parses fine for a typed reader that
    // names only the fields it uses — the way an app reads — and the fields it names are there
    #[derive(serde::Deserialize)]
    struct ReaderEvent {
        kind: String,
    }
    #[derive(serde::Deserialize)]
    struct ReaderCard {
        id: i64,
        title: String,
        column: String,
        owner: Option<String>,
        events: Vec<ReaderEvent>,
    }
    let mut v = serde_json::to_value(&card).unwrap();
    v["brand_new_field"] = serde_json::json!({"anything": true});
    v["events"][0]["brand_new_event_field"] = serde_json::json!(1);
    let r: ReaderCard = serde_json::from_value(v).expect("a typed reader ignores unknown fields");
    assert_eq!((r.id, r.title.as_str(), r.column.as_str(), r.owner.as_deref()), (id, "future proof", "doing", Some("me")));
    assert!(r.events.iter().any(|e| e.kind == "scrying"));
}
