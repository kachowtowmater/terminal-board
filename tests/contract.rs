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

/// Spawn `tb watch` and a watchdog that kills it after `secs` — a watcher that never emits
/// must fail the test, not hang it.
fn spawn_watched(db: &Path, args: &[&str], secs: u64) -> (std::process::Child, BufReader<std::process::ChildStdout>, std::sync::mpsc::Sender<()>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(args)
        .env("TB_DB", db)
        .env("TB_NO_HERDR", "1")
        .env("TB_AS", "tester")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let pid = child.id();
    std::thread::spawn(move || {
        if rx.recv_timeout(std::time::Duration::from_secs(secs)).is_err() {
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
        }
    });
    let out = BufReader::new(child.stdout.take().unwrap());
    (child, out, tx)
}

fn drop_watch(tx: std::sync::mpsc::Sender<()>) {
    let _ = tx.send(()); // disarm the watchdog
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
fn watch_events_streams_one_line_per_event() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let id = {
        let s = Store::open(&db).unwrap();
        s.add("widgets: stream me", "", &[], "me").unwrap()
    };
    let (mut child, stdout, wd) = spawn_watched(&db, &["watch", "--events", "--json"], 30);
    let mut lines = stdout.lines();
    tb(&db, &["note", &id.to_string(), "repro confirmed"]);
    tb(&db, &["take", &id.to_string()]);
    tb(&db, &["move", &id.to_string(), "review"]);
    // replay starts with the card's `created` event; read until we hold all four kinds
    let mut got = Vec::new();
    while got.len() < 4 {
        let l = lines
            .next()
            .unwrap_or_else(|| panic!("watcher stopped after {} events (watchdog fired)", got.len()))
            .unwrap();
        got.push(serde_json::from_str::<serde_json::Value>(&l).unwrap());
    }
    drop_watch(wd);
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(
        keys(&got[0]),
        sorted(&["v", "ts", "card_id", "actor", "kind", "from", "to", "text"]),
        "one NDJSON line per event: {got:?}"
    );
    let kinds: Vec<&str> = got.iter().map(|g| g["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["created", "note", "taken", "moved"], "{got:?}");
    let note = &got[1];
    assert_eq!(note["card_id"], id);
    assert!(note["from"].is_null() && note["to"].is_null(), "non-moves carry no transition: {note}");
    let mv = &got[3];
    assert_eq!(mv["to"], "review");
    assert_eq!(mv["from"], "doing", "take moved it to doing first");
}

#[test]
fn watch_events_since_resumes_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let mut s = Store::open(&db).unwrap();
    let id = s.add("widgets: resume me", "", &[], "me").unwrap();
    s.note(id, "old note", "me").unwrap();
    s.take(id, "me").unwrap();
    s.note(id, "new note", "me").unwrap();
    // --since = the exact ts of the last written event: the stream resumes AT that event
    // (the cursor is the last event strictly before `since`), so a restarted orchestrator
    // sees the tail and every newer event, and nothing is lost to second-granularity races.
    let since: i64 = s
        .events_since(0)
        .unwrap()
        .last()
        .map(|w| w.event.ts)
        .unwrap_or(0);
    let (mut child, stdout, wd) =
        spawn_watched(&db, &["watch", "--events", "--json", "--since", &format!("{since}")], 30);
    let mut lines = stdout.lines();
    // Events at/after `since` stream first — writes inside the same second all share its ts,
    // so the tail may begin earlier in that second (e.g. the card's `created` event). Read
    // until the recorded tail's last event arrives.
    let mut tail = Vec::new();
    loop {
        let l = lines
            .next()
            .unwrap_or_else(|| panic!("watcher emitted nothing after --since (watchdog fired)"))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&l).unwrap();
        tail.push(v.clone());
        if v["text"] == "new note" {
            break;
        }
        assert!(tail.len() <= 4, "tail grew past the recorded events: {tail:?}");
    }
    assert_eq!(tail.last().unwrap()["kind"], "note", "tail ends at the pre-restart note");
    // a new write appears immediately after
    s.note(id, "post-restart note", "me").unwrap();
    let second = lines
        .next()
        .unwrap_or_else(|| panic!("no live event after the write (watchdog fired)"))
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&second).unwrap();
    assert_eq!(v["text"], "post-restart note");
    drop_watch(wd);
    child.kill().unwrap();
    child.wait().unwrap();
    // --since 0 replays the whole history from the start
    let (mut child, stdout, wd) = spawn_watched(&db, &["watch", "--events", "--json", "--since", "0"], 30);
    let mut lines = stdout.lines();
    let first = lines.next().unwrap_or_else(|| panic!("--since 0 replays history (watchdog fired)")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(v["kind"], "created", "--since 0 = stream from the start: {v}");
    drop_watch(wd);
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn watch_flags_are_validated() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let o = tb(&db, &["watch", "--events"]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("--json"), "{}", String::from_utf8_lossy(&o.stderr));
    let o = tb(&db, &["watch", "--since", "123"]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("--events"), "{}", String::from_utf8_lossy(&o.stderr));
}
