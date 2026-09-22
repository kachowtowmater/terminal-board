//! Many cards from one file: `tb import FILE.json|-` and `tb edit --from FILE.json|-`.
//! All or nothing, a report per row, history never forged. Every test drives the real binary
//! and checks the board file itself where "nothing was written" is the claim.
#![cfg(unix)]
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Board {
    dir: tempfile::TempDir,
    db: PathBuf,
}

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("board.db");
        Board { dir, db }
    }
    fn cmd(&self, args: &[&str], actor: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).current_dir(self.dir.path()).env("TB_DB", &self.db).env("TB_AS", actor).env("TB_NO_HERDR", "1");
        for k in ["TB_BOARD", "TTYBOARD_BOARD", "HERDR_AGENT_NAME", "TB_NOW"] {
            c.env_remove(k);
        }
        c.stdin(Stdio::null());
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args, "importer").output().unwrap()
    }
    fn run_as(&self, args: &[&str], actor: &str) -> Output {
        self.cmd(args, actor).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stdout)
    }
    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
    fn refused(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(1), "{args:?} should be refused: {}{}", text(&o.stdout), text(&o.stderr));
        assert!(o.stdout.is_empty(), "{args:?}: a plain refusal prints nothing on stdout: {}", text(&o.stdout));
        text(&o.stderr)
    }
    fn file(&self, name: &str, v: &Value) -> String {
        std::fs::write(self.dir.path().join(name), serde_json::to_string_pretty(v).unwrap()).unwrap();
        name.to_string()
    }
    fn card(&self, id: i64) -> Value {
        self.json(&["show", &id.to_string(), "--json"])
    }
    /// (kind, actor, text) of every event on a card.
    fn events(&self, id: i64) -> Vec<(String, String, String)> {
        self.card(id)["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["kind"].as_str().unwrap().into(), e["actor"].as_str().unwrap().into(), e["text"].as_str().unwrap().into()))
            .collect()
    }
    fn cards(&self) -> Vec<Value> {
        self.json(&["list", "--json"]).as_array().unwrap().clone()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

fn s(a: &str, b: &str, c: &str) -> (String, String, String) {
    (a.into(), b.into(), c.into())
}

/// Every row of every table, in rowid order: what `sqlite3 FILE .dump` would show.
fn dump(db: &Path) -> String {
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|t| t.unwrap())
        .collect();
    tables.sort();
    let mut out = String::new();
    for t in tables {
        out.push_str(&format!("== {t}\n"));
        let mut st = conn.prepare(&format!("SELECT * FROM \"{t}\" ORDER BY rowid")).unwrap();
        let n = st.column_count();
        let mut rows = st.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            let cells: Vec<String> = (0..n).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect();
            out.push_str(&cells.join("|"));
            out.push('\n');
        }
    }
    out
}

/// Imported cards land in TODO with new ids, and their history is the import — whatever the
/// file claims about columns, owners, timestamps or events.
#[test]
fn import_lands_in_todo_and_never_forges_history() {
    let b = Board::new();
    b.ok(&["add", "x: already here"]);
    let f = b.file(
        "new.json",
        &json!([
            {"title": "docs: write the guide", "due": "2026-10-09", "description": "  Done = merged\n\nwith `ticks` and $HOME  ",
             "checklist": ["draft", {"text": "review", "done": true, "n": 2, "idx": 2}], "blocked": "by #1"},
            {"id": 999, "title": "fix lifter", "tag": "Widgets", "gh_ref": 7, "column": "done", "position": 0, "owner": "mallory", "reviewer": "eve",
             "created_at": 1, "column_since": 1, "last_event_at": 1, "round": 9, "days_left": -400, "due_state": "overdue",
             "events": [{"ts": 1, "actor": "mallory", "kind": "approved", "text": "forged"}], "some_future_field": {"x": 1}},
            {"title": "plain one", "due": null, "tag": null, "blocked": null, "checklist": []}
        ]),
    );
    let o = b.run(&["import", &f]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let out = text(&o.stdout);
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "row 1    #2 created  docs: write the guide  due 2026-10-09 · checklist 2 items (1 ticked) · blocked by #1",
            "row 2    #3 created  widgets: gh#7 fix lifter",
            "row 3    #4 created  plain one",
            "3 cards imported into TODO (#2–#4) from new.json",
        ]
    );
    // ignored, with a warning — one line, not one per row
    let warned = text(&o.stderr);
    assert_eq!(warned.lines().count(), 1, "{warned}");
    for field in ["column (1)", "owner (1)", "events (1)", "id (1)", "created_at (1)", "some_future_field (1)"] {
        assert!(warned.contains(field), "the warning names {field}: {warned}");
    }
    assert!(warned.contains("new cards always land in TODO with a new id, and history is never imported"), "{warned}");

    let c = b.card(3);
    assert_eq!((&c["column"], &c["owner"], &c["reviewer"], &c["round"]), (&json!("todo"), &Value::Null, &Value::Null, &json!(1)));
    assert_eq!((&c["title"], &c["tag"], &c["gh_ref"]), (&json!("fix lifter"), &json!("widgets"), &json!(7)));
    assert!(c["created_at"].as_i64().unwrap() > 1_700_000_000, "the file's timestamp was not used");
    assert_eq!(b.events(3), [s("created", "importer", ""), s("imported", "importer", "row 2 of new.json")]);
    let c = b.card(2);
    assert_eq!(c["description"], "Done = merged\n\nwith `ticks` and $HOME", "kept as written, trimmed like every description");
    assert_eq!((&c["due"], &c["blocked"]), (&json!("2026-10-09"), &json!("#1")));
    let list: Vec<(String, bool)> = c["checklist"].as_array().unwrap().iter().map(|i| (i["text"].as_str().unwrap().to_string(), i["done"].as_bool().unwrap())).collect();
    assert_eq!(list, [("draft".to_string(), false), ("review".to_string(), true)]);
    assert_eq!(
        b.events(2),
        [s("created", "importer", ""), s("imported", "importer", "row 1 of new.json"), s("due", "importer", "none -> 2026-10-09"), s("blocked", "importer", "by #1")],
        "the same events the single commands write, plus the import itself"
    );
    // new cards go to the BOTTOM of TODO, in file order
    let order: Vec<i64> = b.json(&["board", "--json"])["columns"]["todo"].as_array().unwrap().iter().map(|c| c["id"].as_i64().unwrap()).collect();
    assert_eq!(order, [1, 2, 3, 4]);
}

/// The `--json` report: one object, a row per card, the ids, the warnings.
#[test]
fn the_json_report() {
    let b = Board::new();
    let f = b.file("two.json", &json!({"cards": [{"title": "a: one", "due": "2026-10-09"}, {"title": "two", "owner": "x"}]}));
    let dry = b.json(&["import", &f, "--dry-run", "--json"]);
    assert!(!b.db.exists(), "a dry run created the board file");
    let real = b.json(&["import", &f, "--json"]);
    let mut keys: Vec<&String> = real.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["command", "created", "dry_run", "ok", "rows", "source", "warnings"]);
    assert_eq!((&real["ok"], &real["command"], &real["dry_run"], &real["created"], &real["source"]), (&json!(true), &json!("import"), &json!(false), &json!([1, 2]), &json!("two.json")));
    assert_eq!(real["rows"][0], json!({"row": 1, "id": 1, "action": "created", "title": "a: one", "ignored": [],
        "changes": [{"field": "title", "from": null, "to": "one"}, {"field": "tag", "from": null, "to": "a"}, {"field": "due", "from": null, "to": "2026-10-09"}]}));
    assert_eq!(real["rows"][1]["ignored"], json!(["owner"]));
    assert_eq!(real["warnings"].as_array().unwrap().len(), 1);
    // the dry run said exactly what the real run did
    let mut expected = real.clone();
    expected["dry_run"] = json!(true);
    assert_eq!(dry, expected);

    let e = b.file("e.json", &json!([{"id": 1, "due": "2026-10-16", "title": "one"}, {"id": 2, "description": "new text"}, {"id": 2_i64.pow(40), "due": null}]));
    let o = b.run(&["edit", "--from", &e, "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!((&v["ok"], &v["error"]), (&json!(false), &json!("1 problem in e.json")));
    assert_eq!(v["hint"], "nothing was written; fix it and check again with 'tb edit --from e.json --dry-run'");
    assert_eq!(v["problems"], json!([{"row": 3, "id": 2_i64.pow(40), "field": "id", "problem": format!("no card #{}", 2_i64.pow(40)), "hint": "see 'tb list' for ids"}]));
    let e = b.file("e.json", &json!([{"id": 1, "due": "2026-10-16", "title": "one"}, {"id": 2, "description": "new text"}]));
    let v = b.json(&["edit", "--from", &e, "--json"]);
    assert_eq!((&v["changed"], &v["unchanged"], &v["command"]), (&json!([1, 2]), &json!([]), &json!("edit")));
    assert_eq!(v["rows"][0]["changes"], json!([{"field": "due", "from": "2026-10-09", "to": "2026-10-16"}]), "a title that did not change is not a change");
    assert_eq!(v["rows"][1]["changes"], json!([{"field": "description", "from": "", "to": "new text"}]));
}

/// A board built with the ordinary commands, for the round trips.
fn worked_board() -> Board {
    let b = Board::new();
    b.ok(&["add", "docs: gh#12 write the guide", "-d", "Done = merged", "--check", "draft", "--check", "review", "--due", "2026-10-09"]);
    b.ok(&["add", "ops: rotate tokens"]);
    b.ok(&["add", "a plain card", "-d", "line one\n\n\tline three with `ticks`, $VARS and \"quotes\""]);
    b.ok(&["add", "ops: waits on the guide"]);
    b.ok(&["check", "1", "1"]);
    b.ok(&["block", "4", "#1"]);
    // the DOING card is held by the actor that runs `edit --from` below: the holder rule has
    // its own test, and this one is about which FIELDS travel
    b.ok(&["take", "1"]);
    assert!(b.run_as(&["take", "2"], "bob").status.success());
    assert!(b.run_as(&["done", "2"], "bob").status.success());
    b
}

const CONTENT: [&str; 6] = ["title", "tag", "gh_ref", "description", "due", "blocked"];

fn content(c: &Value) -> Value {
    let mut o = serde_json::Map::new();
    for k in CONTENT {
        o.insert(k.into(), c[k].clone());
    }
    Value::Object(o)
}

/// export → import: `tb board --json` of one board is a file another board can import.
#[test]
fn an_export_imports_into_a_fresh_board() {
    let from = worked_board();
    let export = from.ok(&["board", "--json"]);
    let to = Board::new();
    std::fs::write(to.dir.path().join("export.json"), &export).unwrap();
    to.ok(&["import", "export.json"]);
    let export: Value = serde_json::from_str(&export).unwrap();
    // board order: todo, doing, review, done
    let exported: Vec<&Value> = ["todo", "doing", "review", "done"].iter().flat_map(|c| export["columns"][c].as_array().unwrap()).collect();
    let imported = to.cards();
    assert_eq!(imported.len(), exported.len());
    for (new, old) in imported.iter().zip(&exported) {
        let full = to.card(new["id"].as_i64().unwrap());
        assert_eq!(content(&full), content(old), "the content travels");
        let list = |c: &Value| -> Vec<(String, bool)> { c["checklist"].as_array().unwrap().iter().map(|i| (i["text"].as_str().unwrap().into(), i["done"].as_bool().unwrap())).collect() };
        assert_eq!(list(&full), list(old), "the checklist travels, ticks included");
        assert_eq!((&full["column"], &full["owner"]), (&json!("todo"), &Value::Null), "the column and the owner do not");
        assert_eq!(to.events(full["id"].as_i64().unwrap())[..2], [s("created", "importer", ""), s("imported", "importer", &format!("row {} of export.json", new["id"]))]);
    }
    // `tb show ID --json` of one card is a document too
    let one = from.ok(&["show", "3", "--json"]);
    let o = to.cmd(&["import", "-"], "importer").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut o = o;
    o.stdin.take().unwrap().write_all(one.as_bytes()).unwrap();
    let o = o.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("from standard input"), "{}", text(&o.stdout));
    assert_eq!(content(&to.card(5)), content(&from.card(3)));
}

/// export → edit the file → `edit --from`: only what changed in the file changes on the board,
/// whatever else the export carried; running it again changes nothing.
#[test]
fn an_edited_export_changes_only_what_was_edited() {
    let b = worked_board();
    let mut export: Value = b.json(&["board", "--json"]);
    let before: Vec<Value> = (1..=4).map(|i| b.card(i)).collect();
    for col in ["todo", "doing", "review", "done"] {
        for c in export["columns"][col].as_array_mut().unwrap() {
            match c["id"].as_i64().unwrap() {
                1 => c["due"] = json!("2026-10-16"),
                2 => c["due"] = json!("2026-11-02"),
                3 => c["title"] = json!("a plain card, renamed"),
                _ => {}
            }
            // things a person might also scribble on, which must not travel
            c["column"] = json!("done");
            c["owner"] = json!("mallory");
        }
    }
    let f = b.file("edited.json", &export);
    let out = b.ok(&["edit", "--from", &f]);
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "row 1    #3 changed  title a plain card -> a plain card, renamed",
            "row 2    #4 unchanged",
            "row 3    #1 changed  due 2026-10-09 -> 2026-10-16",
            "row 4    #2 changed  due none -> 2026-11-02",
            "3 of 4 cards changed (1 unchanged) from edited.json",
        ]
    );
    for (i, old) in before.iter().enumerate() {
        let now = b.card(i as i64 + 1);
        for k in ["column", "owner", "position", "checklist", "created_at", "column_since", "round", "reviewer"] {
            assert_eq!(now[k], old[k], "#{} {k} is untouched", i + 1);
        }
    }
    assert_eq!(b.card(3)["description"], before[2]["description"]);
    assert_eq!(b.events(1).last().unwrap(), &s("due", "importer", "2026-10-09 -> 2026-10-16"));
    assert_eq!(b.events(3).last().unwrap(), &s("edit", "importer", "title edited"));
    // idempotent: the same file again changes nothing and logs nothing
    let snapshot = dump(&b.db);
    let again = b.ok(&["edit", "--from", &f]);
    assert!(again.ends_with("0 of 4 cards changed (4 unchanged) from edited.json\n"), "{again}");
    assert_eq!(dump(&b.db), snapshot);
}

/// One bad row in the middle of a large file: nothing is written, and the row is named.
#[test]
fn a_bad_row_in_the_middle_of_500_writes_nothing() {
    let b = Board::new();
    b.ok(&["add", "x: already here", "--due", "2026-01-01"]);
    let mut rows: Vec<Value> = (1..=500).map(|i| json!({"title": format!("bulk: card {i}"), "due": "2026-10-09"})).collect();
    rows[249]["due"] = json!("2026-02-30");
    rows[376]["tag"] = json!("two words");
    rows[376]["checklist"] = json!(["ok", ""]);
    let f = b.file("big.json", &json!(rows));
    let before = dump(&b.db);
    let e = b.refused(&["import", &f]);
    assert_eq!(
        e.lines().collect::<Vec<_>>(),
        [
            "row 250 due: '2026-02-30' is not a real calendar date — use YYYY-MM-DD, e.g. \"due\": \"2026-10-09\" (or null for none)",
            "row 377 tag: 'two words' cannot be a tag — a tag is up to 20 letters, digits, '-' or '_' (or null for none)",
            "row 377 checklist[2]: the item is empty — an item is text, or {\"text\": \"…\", \"done\": false}",
            "tb: 3 problems in big.json — nothing was written; fix them and check again with 'tb import big.json --dry-run'",
        ]
    );
    assert_eq!(dump(&b.db), before, "a refused import wrote something");
    assert_eq!(b.cards().len(), 1);

    // the same for edit --from, where the bad row is one only the BOARD can find
    rows[249]["due"] = json!("2026-10-09");
    rows[376] = json!({"title": "bulk: card 377"});
    b.ok(&["import", &b.file("big.json", &json!(rows))]);
    let mut edits: Vec<Value> = (2..=501).map(|id| json!({"id": id, "due": "2026-12-24"})).collect();
    edits[249]["id"] = json!(9999);
    edits[400]["due"] = json!("24.12.2026");
    let f = b.file("redate.json", &json!(edits));
    let before = dump(&b.db);
    let e = b.refused(&["edit", "--from", &f]);
    assert_eq!(
        e.lines().collect::<Vec<_>>(),
        [
            "row 250 (#9999) id: no card #9999 — see 'tb list' for ids",
            "row 401 (#402) due: '24.12.2026' is not a date — use YYYY-MM-DD, e.g. \"due\": \"2026-10-09\" (or null for none)",
            "tb: 2 problems in redate.json — nothing was written; fix them and check again with 'tb edit --from redate.json --dry-run'",
        ]
    );
    assert_eq!(dump(&b.db), before, "a refused edit --from wrote something");
}

/// A dry run reports exactly what the real run then does, row for row — and writes nothing,
/// not even a board file that does not exist yet.
#[test]
fn a_dry_run_writes_nothing_and_predicts_the_real_run() {
    let b = Board::new();
    let f = b.file("new.json", &json!([{"title": "a: one", "due": "2026-10-09"}, {"title": "b: two", "checklist": ["x"]}]));
    let dry = b.ok(&["import", &f, "--dry-run"]);
    assert!(!b.db.exists(), "a dry run created the board file");
    assert!(dry.ends_with("dry run — nothing was written; run it for real with 'tb import new.json'\n"), "{dry}");
    let real = b.ok(&["import", &f]);
    let predicted: Vec<String> = dry.lines().take(2).map(|l| l.replace("would create", "created")).collect();
    assert_eq!(predicted, real.lines().take(2).collect::<Vec<_>>());

    let e = b.file("e.json", &json!([{"id": 1, "due": null, "blocked": "#2"}, {"id": 2, "title": "b: two, renamed", "description": "now with words"}]));
    let before = dump(&b.db);
    let dry = b.ok(&["edit", "--from", &e, "--dry-run"]);
    assert_eq!(dump(&b.db), before, "a dry run wrote something");
    assert_eq!(
        dry.lines().collect::<Vec<_>>(),
        [
            "row 1    #1 would change  due 2026-10-09 -> none · blocked none -> #2",
            "row 2    #2 would change  title two -> two, renamed · description edited",
            "2 of 2 cards would be changed (0 unchanged) from e.json",
            "dry run — nothing was written; run it for real with 'tb edit --from e.json'",
        ]
    );
    let real = b.ok(&["edit", "--from", &e]);
    assert_eq!(real.lines().next().unwrap(), "row 1    #1 changed  due 2026-10-09 -> none · blocked none -> #2");
    assert_ne!(dump(&b.db), before);
    // a single edit has no dry run, and must never write while claiming to be one
    let before = dump(&b.db);
    let e = b.refused(&["edit", "1", "--title", "a: oops", "--dry-run"]);
    assert!(e.contains("--dry-run goes with --from") && e.contains("'tb edit --from FILE.json --dry-run'"), "{e}");
    assert_eq!(dump(&b.db), before);
}

/// The scenario this exists for: sixty cards, already being worked on, all need a date.
#[test]
fn sixty_cards_are_re_dated_in_one_command() {
    let b = Board::new();
    let rows: Vec<Value> = (1..=60).map(|i| json!({"title": format!("matter-{:02}: file the response", i)})).collect();
    b.ok(&["import", &b.file("cards.json", &json!(rows))]);
    b.ok(&["config", "wip", "10"]);
    for (id, who) in [("3", "alice"), ("4", "bob"), ("5", "carol")] {
        assert!(b.run_as(&["take", id], who).status.success());
    }
    assert!(b.run_as(&["done", "5"], "carol").status.success());
    b.ok(&["block", "6", "#3"]);
    let before: Vec<Value> = b.cards();

    let dates: Vec<Value> = (1..=60).map(|id| json!({"id": id, "due": format!("2026-{:02}-{:02}", 10 + id % 3, 1 + id % 28)})).collect();
    let f = b.file("dates.json", &json!(dates));
    // two of the sixty are held by somebody else in DOING: the whole re-date is refused,
    // naming each one, and nothing is written — all or nothing, never "58 of 60".
    // (#5 is carol's but in REVIEW, and a REVIEW card is not held: the same rule a single
    // `tb edit` follows.)
    let snapshot = dump(&b.db);
    let e = b.refused(&["edit", "--from", &f]);
    for (row, who) in [(3, "alice"), (4, "bob")] {
        assert!(e.contains(&format!("row {row} (#{row}) id: is held by {who}")), "{e}");
    }
    assert!(!e.contains("#5"), "a REVIEW card is not held: {e}");
    assert!(e.contains("2 problems in dates.json") && e.contains("nothing was written"), "{e}");
    assert_eq!(dump(&b.db), snapshot, "a refused re-date wrote something");
    // with --force it goes through, and every override is logged per card
    assert!(b.ok(&["edit", "--from", &f, "--dry-run", "--force"]).contains("60 of 60 cards would be changed"));
    let out = b.ok(&["edit", "--from", &f, "--force"]);
    assert!(out.ends_with("60 of 60 cards changed (0 unchanged) from dates.json\n"), "{out}");
    let after = b.cards();
    for (old, new) in before.iter().zip(&after) {
        let id = new["id"].as_i64().unwrap();
        assert_eq!(new["due"], json!(format!("2026-{:02}-{:02}", 10 + id % 3, 1 + id % 28)), "#{id}");
        for k in ["title", "tag", "column", "owner", "blocked", "position", "description"] {
            assert_eq!(new[k], old[k], "#{id} {k}: a re-date touches nothing else — not the column, not the owner");
        }
        assert!(new["days_left"].is_i64() || new["column"] == "done", "#{id} has a due state now");
    }
    // the due event, then the force event — the order a single `tb edit --force` writes them
    assert_eq!(
        b.events(3).into_iter().rev().take(2).collect::<Vec<_>>(),
        [s("force", "importer", "edited #3 held by alice"), s("due", "importer", "none -> 2026-10-04")]
    );
    for (id, who) in [(3, "alice"), (4, "bob")] {
        let forced: Vec<String> = b.events(id).into_iter().filter(|e| e.0 == "force").map(|e| e.2).collect();
        assert_eq!(forced, [format!("edited #{id} held by {who}")], "#{id}");
        assert_eq!(b.card(id)["owner"], json!(who), "#{id} still belongs to its holder");
    }
    assert!(b.events(5).iter().all(|e| e.0 != "force"), "a REVIEW card needed no override");
    // a second pass moves ten, clears one, and leaves the rest alone
    let mut second = dates.clone();
    for row in second.iter_mut().take(10) {
        row["due"] = json!("2027-01-15");
    }
    second[59]["due"] = Value::Null;
    let out = b.ok(&["edit", "--from", &b.file("dates2.json", &json!(second)), "--force"]);
    assert!(out.ends_with("11 of 60 cards changed (49 unchanged) from dates2.json\n"), "{out}");
    assert_eq!(b.card(60)["due"], Value::Null);
    assert_eq!(b.events(60).last().unwrap().0, "due");
    assert_eq!(b.events(20).iter().filter(|e| e.0 == "due").count(), 1, "an unchanged card logs nothing");
}

/// Two — here four — imports at the same moment: every one goes through whole, none interleaves.
#[test]
fn imports_at_the_same_moment_each_go_through_whole() {
    let b = Board::new();
    b.ok(&["add", "x: the board exists"]);
    let files: Vec<String> = (0..4)
        .map(|p| {
            let rows: Vec<Value> = (1..=150).map(|i| json!({"title": format!("p{p}: card {i}"), "due": "2026-10-09", "checklist": ["a", "b"]})).collect();
            b.file(&format!("p{p}.json"), &json!(rows))
        })
        .collect();
    let children: Vec<_> = files.iter().map(|f| b.cmd(&["import", f, "--json"], "importer").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()).collect();
    for (p, child) in children.into_iter().enumerate() {
        let o = child.wait_with_output().unwrap();
        assert!(o.status.success(), "import {p} failed instead of waiting its turn: {}{}", text(&o.stdout), text(&o.stderr));
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        let ids: Vec<i64> = v["created"].as_array().unwrap().iter().map(|i| i.as_i64().unwrap()).collect();
        assert_eq!(ids.len(), 150);
        assert!(ids.windows(2).all(|w| w[1] == w[0] + 1), "import {p} was interleaved with another: {ids:?}");
        // and the ids it reported are the cards it made
        for (row, id) in [(1usize, ids[0]), (150, ids[149])] {
            assert_eq!(b.card(id)["title"], json!(format!("card {row}")), "import {p}");
            assert_eq!(b.card(id)["tag"], json!(format!("p{p}")));
        }
    }
    let cards = b.cards();
    assert_eq!(cards.len(), 601);
    let mut positions: Vec<i64> = cards.iter().map(|c| c["position"].as_i64().unwrap()).collect();
    positions.sort();
    assert_eq!(positions, (0..601).collect::<Vec<i64>>(), "every card has its own place in TODO");
}

/// `edit --from` writes each field exactly as its single command does: same values, same events.
#[test]
fn edit_from_matches_the_single_commands() {
    let (single, bulk) = (Board::new(), Board::new());
    for b in [&single, &bulk] {
        b.ok(&["add", "docs: gh#12 write the guide", "-d", "old words", "--due", "2026-10-09"]);
        b.ok(&["add", "ops: rotate tokens"]);
        b.ok(&["block", "2", "#1"]);
    }
    single.ok(&["edit", "1", "--title", "docs: gh#12 the install guide", "--desc", "  new words  "]);
    single.ok(&["edit", "1", "--due", "2026-10-16"]);
    single.ok(&["block", "1", "by #2"]);
    single.ok(&["edit", "2", "--title", "ops: gh#30 rotate tokens"]);
    single.ok(&["edit", "2", "--due", "none"]);
    single.ok(&["block", "2", "--clear"]);
    let f = bulk.file(
        "e.json",
        &json!([
            {"id": 1, "title": "the install guide", "description": "  new words  ", "due": "2026-10-16", "blocked": "by #2"},
            {"id": 2, "gh_ref": 30, "due": null, "blocked": null}
        ]),
    );
    bulk.ok(&["edit", "--from", &f]);
    for id in [1, 2] {
        let (a, z) = (single.card(id), bulk.card(id));
        assert_eq!(content(&a), content(&z), "#{id}");
        assert_eq!(single.events(id), bulk.events(id), "#{id}: the same events, in the same order");
    }
    // a title without its own tag keeps the card's tag; "tag": null drops it; they must agree
    let f = bulk.file("t.json", &json!([{"id": 1, "tag": null}, {"id": 2, "title": "infra: rotate tokens"}]));
    bulk.ok(&["edit", "--from", &f]);
    assert_eq!((bulk.card(1)["tag"].clone(), bulk.card(1)["title"].clone()), (Value::Null, json!("the install guide")));
    assert_eq!((bulk.card(2)["tag"].clone(), bulk.card(2)["gh_ref"].clone()), (json!("infra"), json!(30)));
    let e = bulk.refused(&["edit", "--from", &bulk.file("c.json", &json!([{"id": 2, "title": "ops: x", "tag": "infra"}, {"id": 1, "title": "gh#5 y", "gh_ref": 6}]))]);
    assert!(e.contains("row 1 (#2) tag: the title carries the tag 'ops' but tag is 'infra'") && e.contains("row 2 (#1) gh_ref: the title carries gh#5 but gh_ref is 6"), "{e}");
}

/// Every kind of problem is named by row, card and field, and every hint is a next step.
#[test]
fn every_problem_names_its_row_card_and_field() {
    let b = Board::new();
    b.ok(&["add", "x: one"]);
    let f = b.file(
        "bad.json",
        &json!([{"id": 1, "due": 20261009}, "just text", {"id": 1, "title": 7}, {"title": "no id"}, {"id": -3}, {"id": 2, "description": ["a"]}, {"id": 1, "blocked": true, "gh_ref": "x", "tag": 5}]),
    );
    let e = b.refused(&["edit", "--from", &f]);
    let lines: Vec<&str> = e.lines().collect();
    assert_eq!(lines.len(), 12, "{e}");
    for l in &lines[..11] {
        assert!(l.starts_with("row ") && l.contains(": ") && l.contains(" — "), "row N field: what — what to do: {l}");
    }
    for want in [
        "row 1 (#1) due: the due date is a number, not text — ",
        "row 2 row: the row is text, not a card object — a row looks like {\"id\": 7, \"due\": \"2026-10-09\"}",
        "row 3 (#1) title: the title is a number, not text — ",
        "row 3 (#1) id: card #1 is already changed by row 1 — keep one row per card",
        "row 4 id: the row has no id, so it names no card — add \"id\": N (see 'tb list' for ids), or create cards with 'tb import'",
        "row 5 id: id is -3, not a card number — ",
        "row 6 (#2) description: the description is a list, not text — ",
        "row 7 (#1) tag: the tag is a number, not text — ",
        "row 7 (#1) gh_ref: gh_ref is \"x\", not an issue number — ",
        "row 7 (#1) blocked: blocked is true/false, not text — ",
        "row 7 (#1) id: card #1 is already changed by row 3 — keep one row per card",
    ] {
        assert!(lines.iter().any(|l| l.starts_with(want)), "missing `{want}` in:\n{e}");
    }
    assert_eq!(lines[11], "tb: 11 problems in bad.json — nothing was written; fix them and check again with 'tb edit --from bad.json --dry-run'");
    // import: a missing title
    let e = b.refused(&["import", &b.file("i.json", &json!([{"due": "2026-10-09"}, {"title": "   "}]))]);
    assert!(e.contains("row 1 title: the row has no title — ") && e.contains("row 2 title: the title is empty — "), "{e}");
}

/// The file itself: not JSON, not cards, empty, too big, standard input — and an explicit
/// board in every hint.
#[test]
fn unusable_files_are_refused_before_any_board_is_touched() {
    let b = Board::new();
    let write = |name: &str, body: &str| {
        std::fs::write(b.dir.path().join(name), body).unwrap();
        name.to_string()
    };
    for (body, want) in [
        ("[{\"title\": \"a\"},", "f.json is not valid JSON ("),
        ("{\"wip\": 3}", "f.json is not a list of cards — give a JSON array of card objects"),
        ("[]", "f.json holds no cards — nothing to do"),
        ("   \n", "'f.json' is empty — nothing to store"),
    ] {
        let e = b.refused(&["import", &write("f.json", body)]);
        assert!(e.contains(want), "{body}: {e}");
    }
    let e = b.refused(&["import", "missing.json"]);
    assert!(e.contains("no file 'missing.json'") && e.contains("'tb import PATH'"), "{e}");
    let e = b.refused(&["edit", "--from", "-"]);
    assert!(e.contains("standard input is empty") && e.contains("'tb edit --from -'"), "{e}");
    let big = format!("[{{\"title\": \"{}\"}}]", "x".repeat(4 * 1024 * 1024));
    let e = b.refused(&["import", &write("big.json", &big)]);
    assert!(e.contains(&format!("limited to {} bytes ({} KiB)", 4 * 1024 * 1024, 4 * 1024)) && e.contains("split it into smaller files"), "{e}");
    assert!(!b.db.exists(), "a refused import created the board file");
    // usage errors stay usage errors
    for args in [&["edit", "1", "--from", "f.json"][..], &["edit", "--from", "f.json", "--due", "2026-10-09"][..], &["edit"][..], &["import"][..]] {
        assert_eq!(b.run(args).status.code(), Some(2), "{args:?}");
    }
    // a named board: never created by import, and named in the hints
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).current_dir(b.dir.path()).env("HOME", home.path()).env("TB_AS", "importer").env("TB_NO_HERDR", "1");
        for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TB_CONFIG"] {
            c.env_remove(k);
        }
        c.stdin(Stdio::null()).output().unwrap()
    };
    let f = write("ok.json", "[{\"title\": \"a: one\"}]");
    let o = run(&["wrok", "import", &f]);
    assert!(!o.status.success() && text(&o.stderr).contains("no board 'wrok'"), "{}", text(&o.stderr));
    assert!(run(&["work", "add", "x: make the board"]).status.success());
    assert!(run(&["work", "import", &f]).status.success());
    let o = run(&["work", "edit", "--from", &write("e.json", "[{\"id\": 77, \"due\": null}]")]);
    let e = text(&o.stderr);
    assert!(e.contains("see 'tb work list' for ids") && e.contains("'tb work edit --from e.json --dry-run'"), "{e}");
    let o = run(&["work", "import", &f, "--dry-run"]);
    assert!(text(&o.stdout).contains("run it for real with 'tb work import ok.json'"), "{}", text(&o.stdout));
}

/// Imported text is data: it is stored as written and never reaches a terminal raw.
#[test]
fn imported_text_goes_through_the_display_sanitiser() {
    let b = Board::new();
    let f = b.file("noisy.json", &json!([{"title": "ops: red\u{1b}[31malert\u{1b}]0;title\u{7} now", "description": "clear\u{1b}[2J", "blocked": "\u{9b}2Jthat"}]));
    let o = b.run(&["import", &f]);
    assert!(o.status.success());
    let mut seen = format!("{}{}", text(&o.stdout), text(&o.stderr));
    for args in [&["list"][..], &["show", "1"][..], &["board"][..], &["edit", "--from", &f, "--dry-run"][..]] {
        let o = b.run(args);
        seen.push_str(&text(&o.stdout));
        seen.push_str(&text(&o.stderr));
    }
    let bad: Vec<String> = seen.chars().filter(|c| (*c as u32) < 0x20 && *c != '\n' || (0x7f..=0x9f).contains(&(*c as u32))).map(|c| format!("U+{:04X}", c as u32)).collect();
    assert!(bad.is_empty(), "control characters reached the terminal: {bad:?}\n{seen}");
    // the store keeps the text as written, but JSON shows it cleaned like the screen: the
    // title still carries its words, and no ESC anywhere
    let title = b.card(1)["title"].as_str().unwrap().to_string();
    assert!(title.contains("red") && title.contains("alert") && title.contains("now"), "{title:?}");
    assert!(!title.contains('\u{1b}') && !title.contains('\u{7}'), "JSON output is cleaned: {title:?}");
}

/// The manuals teach it, and the agent manual stays under its cap.
#[test]
fn the_manuals_teach_it() {
    let agents = include_str!("../docs/AGENTS.md");
    for phrase in ["tb import", "tb edit --from", "--dry-run"] {
        assert!(agents.contains(phrase), "docs/AGENTS.md lacks `{phrase}`");
    }
    assert!(agents.lines().count() <= 250, "the agent manual is over its line cap");
    for (name, doc) in [("README.md", include_str!("../README.md")), ("docs/HUMANS.md", include_str!("../docs/HUMANS.md")), ("docs/JSON.md", include_str!("../docs/JSON.md"))] {
        assert!(doc.contains("tb import") && doc.contains("edit --from"), "{name}");
    }
    assert!(include_str!("../docs/JSON.md").contains("\"problems\""), "docs/JSON.md shows the refusal shape");
}

/// THE HOLDER RULE. `edit --from` has its own write path, so it must take the same guard a
/// single `tb edit` takes: a DOING card somebody else holds is not rewritten from a file
/// either. Because a bulk edit is all or nothing, one guarded row refuses the WHOLE file.
#[test]
fn edit_from_will_not_rewrite_a_card_someone_else_holds() {
    let b = Board::new();
    b.ok(&["add", "x: alice's card"]);
    b.ok(&["add", "x: nobody's card"]);
    assert!(b.run_as(&["take", "1"], "alice").status.success());
    let before = dump(&b.db);
    let f = b.file(
        "e.json",
        &json!([{"id": 2, "description": "this row is fine"}, {"id": 1, "title": "x: bob rewrote it", "description": "bob was here", "due": "2026-12-01", "blocked": "#9"}]),
    );

    // bob, no --force: refused, and NOTHING is written — not even the row that was fine
    let o = b.cmd(&["edit", "--from", &f], "bob").output().unwrap();
    assert_eq!(o.status.code(), Some(1), "bob rewrote a card alice holds: {}", text(&o.stdout));
    let e = text(&o.stderr);
    assert!(e.contains("row 2 (#1) id: is held by alice"), "the refusal names the row and the card: {e}");
    assert!(e.contains("'tb edit --from FILE --force'"), "and how to override it: {e}");
    assert!(e.contains("nothing was written"), "{e}");
    assert_eq!(dump(&b.db), before, "a refused edit --from wrote something");
    assert_eq!(b.card(1)["title"], "alice's card");
    assert_eq!(b.card(2)["description"], "");
    // the same under --json, in the documented shape
    let o = b.cmd(&["edit", "--from", &f, "--json"], "bob").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!((&v["ok"], &v["problems"][0]["row"], &v["problems"][0]["id"]), (&json!(false), &json!(2), &json!(1)));
    assert!(v["problems"][0]["problem"].as_str().unwrap().contains("held by alice"), "{v}");
    assert_eq!(dump(&b.db), before);
    // a dry run is refused the same way, and says nothing would be written
    let o = b.cmd(&["edit", "--from", &f, "--dry-run"], "bob").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(dump(&b.db), before);

    // alice editing HER OWN card from a file is not guarded
    b.run_as(&["edit", "--from", &b.file("a.json", &json!([{"id": 1, "description": "alice's own words"}]))], "alice");
    assert_eq!(b.card(1)["description"], "alice's own words");
    // and an unheld card is never guarded, whoever asks
    let o = b.cmd(&["edit", "--from", &b.file("n.json", &json!([{"id": 2, "description": "anyone may"}]))], "bob").output().unwrap();
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(b.card(2)["description"], "anyone may");
}

/// `--force` overrides the holder rule and is logged per card, exactly as a single
/// `tb edit --force` logs it.
#[test]
fn edit_from_force_is_allowed_and_logged_like_a_single_edit() {
    let (bulk, single) = (Board::new(), Board::new());
    for b in [&bulk, &single] {
        b.ok(&["add", "x: alice's card"]);
        assert!(b.run_as(&["take", "1"], "alice").status.success());
    }
    single.run_as(&["edit", "1", "--desc", "bob was here", "--force"], "bob");
    let f = bulk.file("e.json", &json!([{"id": 1, "description": "bob was here"}]));
    let o = bulk.cmd(&["edit", "--from", &f, "--force"], "bob").output().unwrap();
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("#1 changed"), "{}", text(&o.stdout));
    assert_eq!(bulk.card(1)["description"], "bob was here");
    // the same `force` event, by the same actor, with the same text
    let forced: Vec<(String, String, String)> = bulk.events(1).into_iter().filter(|e| e.0 == "force").collect();
    assert_eq!(forced, [s("force", "bob", "edited #1 held by alice")]);
    assert_eq!(forced, single.events(1).into_iter().filter(|e| e.0 == "force").collect::<Vec<_>>());
    // the card still belongs to alice: a forced edit is not a takeover
    assert_eq!((&bulk.card(1)["owner"], &bulk.card(1)["column"]), (&json!("alice"), &json!("doing")));
    // a row that changes nothing logs no force event
    let before = bulk.events(1).len();
    assert!(bulk.cmd(&["edit", "--from", &f, "--force"], "bob").output().unwrap().status.success());
    assert_eq!(bulk.events(1).len(), before, "an unchanged row logged a force event");
}
