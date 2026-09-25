//! The board on its way out: `tb export --json`, `tb export --csv`, `tb log`,
//! `tb list --done --since`. Every test drives the real binary.
#![cfg(unix)]
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Board {
    dir: tempfile::TempDir,
    db: PathBuf,
}

/// A card title, a description and a note that a spreadsheet, a shell or a terminal would
/// each mangle in its own way.
const NASTY_TITLE: &str = "=cmd|' /C calc'!A1";
const NASTY_DESC: &str = "Done = merged, with a \"quote\", a comma, and\na second line\twith a tab";

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("board.db");
        Board { dir, db }
    }
    fn cmd(&self, args: &[&str], actor: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args)
            .current_dir(self.dir.path())
            .env("TB_DB", &self.db)
            .env("TB_AS", actor)
            .env("TB_NO_HERDR", "1")
            .env("TZ", "UTC");
        for k in ["TB_BOARD", "TTYBOARD_BOARD", "HERDR_AGENT_NAME", "TB_CONFIG"] {
            c.env_remove(k);
        }
        c.stdin(Stdio::null());
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args, "alice").output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stdout)
    }
    fn ok_bytes(&self, args: &[&str]) -> Vec<u8> {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}", text(&o.stderr));
        o.stdout
    }
    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
    fn refused(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(1), "{args:?} should be refused: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stderr)
    }
    /// A board with everything an export has to carry.
    fn worked(&self) -> &Board {
        self.ok(&["config", "tz", "America/Los_Angeles"]);
        self.ok(&["add", "docs: gh#12 write the guide", "-d", NASTY_DESC, "--check", "draft", "--check", "review", "--due", "2026-10-09"]);
        self.ok(&["add", NASTY_TITLE]);
        self.ok(&["add", "ops: naïve héllo — 你好 🚀"]);
        self.ok(&["add", "ops: waits"]);
        self.ok(&["take", "1"]);
        self.ok(&["check", "1", "1"]);
        self.ok(&["note", "1", "a note, with a comma and a \"quote\""]);
        self.ok(&["block", "4", "#1"]);
        self.ok(&["done", "1"]);
        assert!(self.cmd(&["done", "1"], "bob").output().unwrap().status.success());
        self
    }
    fn file(&self, name: &str, body: &[u8]) -> String {
        std::fs::write(self.dir.path().join(name), body).unwrap();
        name.to_string()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

/// Every byte of a file, plus the size of its `-wal`: what "nothing was written" means here.
fn fingerprint(db: &Path) -> (Vec<u8>, u64) {
    let bytes = std::fs::read(db).unwrap_or_default();
    let wal = std::fs::metadata(format!("{}-wal", db.display())).map(|m| m.len()).unwrap_or(0);
    (bytes, wal)
}

/// Split a CSV line into cells, honouring RFC 4180 quoting.
fn cells(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// The CSV as (header, rows), with the byte-order mark removed and CRLF row ends checked.
fn parse_csv(raw: &[u8]) -> (Vec<String>, Vec<Vec<String>>) {
    assert_eq!(&raw[..3], b"\xef\xbb\xbf", "a UTF-8 byte-order mark, so Excel reads it as UTF-8");
    let body = std::str::from_utf8(&raw[3..]).expect("UTF-8");
    // rows end CRLF; a record may still hold a bare LF inside a quoted cell
    let mut rows: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut it = body.chars().peekable();
    while let Some(c) = it.next() {
        if c == '"' {
            quoted = !quoted;
        }
        if c == '\r' && !quoted && it.peek() == Some(&'\n') {
            it.next();
            rows.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(c);
    }
    assert!(cur.is_empty(), "the file ends with a complete row: {cur:?}");
    let header = cells(&rows[0]);
    (header, rows[1..].iter().map(|r| cells(r)).collect())
}

fn column<'a>(header: &[String], row: &'a [String], name: &str) -> &'a str {
    let i = header.iter().position(|h| h == name).unwrap_or_else(|| panic!("no column {name} in {header:?}"));
    &row[i]
}

/// THE reason both exist: a board exported as JSON goes straight back in through `tb import`.
#[test]
fn an_export_imports_straight_back() {
    let from = Board::new();
    from.worked();
    let export = from.ok_bytes(&["export", "--json"]);
    let to = Board::new();
    to.file("export.json", &export);
    let out = to.ok(&["import", "export.json"]);
    assert!(out.contains("4 cards imported"), "{out}");

    let doc: Value = serde_json::from_slice(&export).unwrap();
    let exported = doc["cards"].as_array().unwrap();
    assert_eq!(exported.len(), 4);
    for (i, old) in exported.iter().enumerate() {
        let new = to.json(&["show", &(i as i64 + 1).to_string(), "--json"]);
        for k in ["title", "tag", "gh_ref", "description", "due", "blocked"] {
            assert_eq!(new[k], old[k], "#{}: {k} did not survive the round trip", i + 1);
        }
        let list = |c: &Value| -> Vec<(String, bool)> {
            c["checklist"].as_array().unwrap().iter().map(|i| (i["text"].as_str().unwrap().into(), i["done"].as_bool().unwrap())).collect()
        };
        assert_eq!(list(&new), list(old), "#{}: the checklist did not survive", i + 1);
    }
    // and `tb edit --from` reads the very same document
    let e = to.ok(&["edit", "--from", "export.json", "--dry-run"]);
    assert!(e.contains("4 of 4 cards") || e.contains("cards would be changed"), "{e}");
}

/// The exported card object is the documented `card` object — the one `tb board --json`
/// prints — with EVERY event instead of the last ten.
#[test]
fn a_card_in_the_export_is_the_documented_card_object() {
    let b = Board::new();
    b.worked();
    for _ in 0..12 {
        b.ok(&["note", "2", "another note"]);
    }
    let doc = b.json(&["export", "--json"]);
    let board = b.json(&["board", "--json"]);
    let keys = |v: &Value| {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    let exported = doc["cards"].as_array().unwrap();
    let live = &board["columns"]["todo"][0];
    assert_eq!(keys(&exported[0]), keys(live), "the export's card has different fields from tb board --json");
    assert_eq!((&doc["v"], &doc["board"]), (&serde_json::json!(1), &serde_json::json!("default")));
    assert_eq!(doc["tz"], "America/Los_Angeles");
    assert!(doc["exported_at"].as_i64().unwrap() > 1_700_000_000);
    // the whole history: the live card object caps events at ten, the export does not
    let card2 = exported.iter().find(|c| c["id"] == 2).unwrap();
    let live2 = board["columns"]["todo"].as_array().unwrap().iter().find(|c| c["id"] == 2).unwrap();
    assert_eq!(live2["events"].as_array().unwrap().len(), 10, "the live object still caps at ten");
    assert_eq!(card2["events"].as_array().unwrap().len(), 13, "the export carries every event");
    assert_eq!(card2["events"][0]["kind"], "created", "oldest first");
}

/// The CSV a person opens in Excel: quoting, embedded newlines, encoding, and the guard that
/// stops a card title being run as a formula.
#[test]
fn the_csv_is_safe_to_open_in_a_spreadsheet() {
    let b = Board::new();
    b.worked();
    let raw = b.ok_bytes(&["export", "--csv"]);
    let (header, rows) = parse_csv(&raw);
    assert_eq!(header[0], "id");
    assert_eq!(rows.len(), 4, "one row per card");

    // FORMULA INJECTION: a title that starts `=` must not be a formula in the sheet
    let evil = rows.iter().find(|r| column(&header, r, "id") == "2").unwrap();
    let title = column(&header, evil, "title");
    assert_eq!(title, format!("'{NASTY_TITLE}"), "a leading = must be neutralised");
    assert!(!title.starts_with('='), "{title}");

    // a description with a comma, a quote and a newline is ONE cell, whole
    let first = rows.iter().find(|r| column(&header, r, "id") == "1").unwrap();
    assert_eq!(column(&header, first, "description"), NASTY_DESC);
    assert_eq!(column(&header, first, "checklist"), "[x] draft\n[ ] review");
    assert_eq!(column(&header, first, "checklist done"), "1");
    assert_eq!(column(&header, first, "checklist total"), "2");
    assert_eq!(column(&header, first, "title"), "write the guide");
    assert_eq!(column(&header, first, "github"), "12");
    assert_eq!(column(&header, first, "due"), "2026-10-09");
    assert_eq!(column(&header, first, "column"), "done");
    // unicode survives, because the file says it is UTF-8
    let uni = rows.iter().find(|r| column(&header, r, "id") == "3").unwrap();
    assert_eq!(column(&header, uni, "title"), "naïve héllo — 你好 🚀");
    // a blocked card names what it waits on
    let blocked = rows.iter().find(|r| column(&header, r, "id") == "4").unwrap();
    assert_eq!(column(&header, blocked, "blocked"), "#1");

    // the history sheet
    let raw = b.ok_bytes(&["export", "--csv", "--history"]);
    let (header, rows) = parse_csv(&raw);
    assert_eq!(header, ["when", "card", "card title", "who", "what", "detail"]);
    assert!(rows.len() >= 10, "{} events", rows.len());
    assert_eq!(column(&header, &rows[0], "what"), "created");
    assert_eq!(column(&header, &rows[0], "who"), "alice");
    let note = rows.iter().find(|r| column(&header, r, "what") == "note").unwrap();
    assert_eq!(column(&header, note, "detail"), "a note, with a comma and a \"quote\"");
    // a card title in the history sheet is guarded too
    let evil = rows.iter().find(|r| column(&header, r, "card") == "#2").unwrap();
    assert!(column(&header, evil, "card title").starts_with("'="), "{:?}", evil);
    // every `when` is a readable local time, not a unix second
    for r in &rows {
        let when = column(&header, r, "when");
        assert!(when.len() == 16 && when.as_bytes()[4] == b'-', "{when}");
    }
}

/// Nothing here writes: the board file is byte-identical afterwards and the `-wal` has not
/// grown, whichever of the four read commands ran.
#[test]
fn every_export_command_leaves_the_board_file_untouched() {
    let b = Board::new();
    b.worked();
    // settle the file first, so the baseline is what a closed board looks like
    b.ok(&["list"]);
    let before = fingerprint(&b.db);
    for args in [
        &["export", "--json"][..],
        &["export", "--csv"][..],
        &["export", "--csv", "--history"][..],
        &["log"][..],
        &["log", "--json"][..],
        &["log", "--json", "--since", "2020-01-01"][..],
        &["list", "--done"][..],
        &["list", "--done", "--since", "2020-01-01"][..],
        &["list", "--done", "--json"][..],
    ] {
        b.ok(args);
        let after = fingerprint(&b.db);
        assert!(after.0 == before.0, "{args:?} changed the board file");
        assert!(after.1 <= before.1, "{args:?} grew the -wal from {} to {}", before.1, after.1);
    }
}

/// `tb log`, plain and `--json`, and `--since` as a local calendar day in the board's zone.
#[test]
fn log_reads_the_history_from_a_date() {
    let b = Board::new();
    b.worked();
    let plain = b.ok(&["log"]);
    assert!(plain.lines().count() >= 10, "{plain}");
    assert!(plain.lines().next().unwrap().contains("created"), "{plain}");
    assert!(plain.contains("#1") && plain.contains("alice"), "{plain}");

    let all = b.json(&["log", "--json"]);
    let all = all.as_array().unwrap();
    assert!(all.len() >= 10);
    let first = &all[0];
    let mut keys: Vec<&String> = first.as_object().unwrap().keys().collect();
    keys.sort();
    // `identity` (the whole identity behind `actor_id`, inline) was added by the verifier rule's trace;
    // `ancestry` (the kernel's parent chain, card #169) joins the line on events that record it
    let without = first.as_object().unwrap().keys().filter(|k| *k != "ancestry").cloned().collect::<Vec<_>>();
    assert_eq!(without, ["actor", "actor_id", "card_id", "identity", "kind", "text", "ts", "v"]);
    let mut keys: Vec<&String> = first.as_object().unwrap().keys().collect();
    keys.sort();
    let expected: Vec<String> = ["actor", "actor_id", "ancestry", "card_id", "identity", "kind", "text", "ts", "v"].iter().map(|s| s.to_string()).collect();
    let mut expected = expected; expected.sort();
    assert_eq!(keys, expected);
    assert_eq!((&first["v"], &first["card_id"], &first["kind"]), (&serde_json::json!(1), &serde_json::json!(1), &serde_json::json!("created")));
    // oldest first, and never going backwards
    let ts: Vec<i64> = all.iter().map(|e| e["ts"].as_i64().unwrap()).collect();
    assert!(ts.windows(2).all(|w| w[0] <= w[1]), "not in order");

    // a date far in the future hides everything; the epoch shows it all
    let future = format!("{}-01-01", 2000 + 99);
    assert!(b.json(&["log", "--json", "--since", &future]).as_array().unwrap().is_empty());
    assert_eq!(b.json(&["log", "--json", "--since", "1970-01-01"]).as_array().unwrap().len(), all.len());
    assert!(b.ok(&["log", "--since", &future]).contains("no history yet"));
    // a unix second works too (what `tb watch --since` takes)
    assert_eq!(b.json(&["log", "--json", "--since", "0"]).as_array().unwrap().len(), all.len());
    let e = b.refused(&["log", "--since", "09/10/2026"]);
    assert!(e.contains("is not a date") && e.contains("'tb log --since 2026-10-09'"), "{e}");
    let o = b.run(&["log", "--since", "09/10/2026", "--json"]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], false, "{v}");
}

/// `--since` is a LOCAL calendar day in the board's zone: the same date means a different
/// instant in Los Angeles and in Auckland, and it must never be UTC midnight.
#[test]
fn a_since_boundary_is_a_local_day_not_a_utc_instant() {
    // 2026-10-09 07:30 UTC = 2026-10-09 00:30 in Los Angeles, and 2026-10-09 20:30 in Auckland
    let at = 1_791_531_000_i64;
    let made = |zone: &str| -> Board {
        let b = Board::new();
        b.cmd(&["config", "tz", zone], "alice").output().unwrap();
        // one card created at `at`, one a day earlier
        for (n, when) in [("older", at - 86_400), ("today", at)] {
            let o = b.cmd(&["add", &format!("x: {n}")], "alice").env("TB_NOW", when.to_string()).output().unwrap();
            assert!(o.status.success(), "{}", text(&o.stderr));
        }
        b
    };
    for (zone, expected_on_the_day) in [("America/Los_Angeles", 1), ("Pacific/Auckland", 1), ("UTC", 1)] {
        let b = made(zone);
        let since = b.cmd(&["log", "--json", "--since", "2026-10-09"], "alice").env("TB_NOW", at.to_string()).output().unwrap();
        let v: Value = serde_json::from_slice(&since.stdout).unwrap();
        let created: Vec<&Value> = v.as_array().unwrap().iter().filter(|e| e["kind"] == "created").collect();
        assert_eq!(created.len(), expected_on_the_day, "{zone}: {v}");
        assert_eq!(created[0]["card_id"], 2, "{zone}: the card made on the 9th, not the one from the 8th");
    }
    // the boundary itself: in Los Angeles the 9th starts at 07:00 UTC, so an event at 06:59
    // UTC is still the 8th there and must be excluded
    let b = made("America/Los_Angeles");
    let just_before = 1_791_529_140; // 2026-10-09 06:59 UTC = 2026-10-08 23:59 in Los Angeles
    let o = b.cmd(&["add", "x: late on the 8th"], "alice").env("TB_NOW", just_before.to_string()).output().unwrap();
    assert!(o.status.success());
    let v: Value = serde_json::from_slice(&b.cmd(&["log", "--json", "--since", "2026-10-09"], "alice").output().unwrap().stdout).unwrap();
    let ids: Vec<i64> = v.as_array().unwrap().iter().filter(|e| e["kind"] == "created").map(|e| e["card_id"].as_i64().unwrap()).collect();
    assert_eq!(ids, [2], "23:59 local on the 8th is not 'since the 9th'");
}

/// `tb list --done` shows what the board's DONE column shows; `--since` reaches further back.
#[test]
fn list_done_shows_today_and_since_reaches_back() {
    let b = Board::new();
    b.ok(&["add", "x: old work"]);
    b.ok(&["add", "x: todays work"]);
    b.ok(&["add", "x: still going"]);
    let now = 1_791_531_000_i64;
    let long_ago = now - 40 * 86_400;
    for (id, when) in [(1, long_ago), (2, now)] {
        let id = id.to_string();
        for (verb, who) in [("take", "alice"), ("done", "alice"), ("done", "bob")] {
            let o = b.cmd(&[verb, &id], who).env("TB_NOW", when.to_string()).output().unwrap();
            assert!(o.status.success(), "{verb} {id} as {who}: {}", text(&o.stderr));
        }
    }
    let done_now = |args: &[&str]| -> String {
        let o = b.cmd(args, "alice").env("TB_NOW", now.to_string()).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", text(&o.stderr));
        text(&o.stdout)
    };
    // today only: the card finished forty days ago is not in it
    let today = done_now(&["list", "--done"]);
    assert!(today.contains("todays work") && !today.contains("old work"), "{today}");
    // reaching back finds it, newest first
    let old = done_now(&["list", "--done", "--since", "1970-01-01"]);
    assert!(old.contains("old work") && old.contains("todays work"), "{old}");
    assert!(old.find("todays work").unwrap() < old.find("old work").unwrap(), "newest first: {old}");
    // never a card that is not finished
    assert!(!old.contains("still going"), "{old}");
    // --json is the same set, as card objects
    let o = b.cmd(&["list", "--done", "--since", "1970-01-01", "--json"], "alice").env("TB_NOW", now.to_string()).output().unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let ids: Vec<i64> = v.as_array().unwrap().iter().map(|c| c["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, [2, 1]);
    assert!(v[0]["column_label"].is_string(), "a full card object: {v}");
    // nothing finished at all
    let empty = Board::new();
    empty.ok(&["add", "x: never finished"]);
    assert!(empty.ok(&["list", "--done"]).contains("no cards finished today"));
    assert_eq!(empty.json(&["list", "--done", "--json"]).as_array().unwrap().len(), 0);
    // --since needs --done, and --archived is a different list
    assert_eq!(empty.run(&["list", "--since", "2026-10-09"]).status.code(), Some(2));
    assert_eq!(empty.run(&["list", "--done", "--archived"]).status.code(), Some(2));
}

/// A reader that goes away (`tb export --csv | head`) ends tb quietly, like every other
/// command — never a "broken pipe" error in the middle of someone's terminal.
#[test]
fn a_closed_reader_is_not_an_error() {
    let b = Board::new();
    b.worked();
    for args in [&["export", "--csv"][..], &["export", "--json"][..], &["log"][..], &["log", "--json"][..]] {
        let mut child = b.cmd(args, "alice").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        drop(child.stdout.take());
        let o = child.wait_with_output().unwrap();
        assert!(o.status.success(), "{args:?} exited {:?}: {}", o.status.code(), text(&o.stderr));
        assert!(text(&o.stderr).is_empty(), "{args:?} complained: {}", text(&o.stderr));
    }
}

/// The manuals teach it, and the agent manual stays under its cap.
#[test]
fn the_manuals_teach_it() {
    let agents = include_str!("../docs/AGENTS.md");
    for phrase in ["tb export", "tb log", "--done"] {
        assert!(agents.contains(phrase), "docs/AGENTS.md lacks `{phrase}`");
    }
    assert!(agents.lines().count() <= 250, "the agent manual is over its line cap");
    for (name, doc) in [("README.md", include_str!("../README.md")), ("docs/HUMANS.md", include_str!("../docs/HUMANS.md")), ("docs/JSON.md", include_str!("../docs/JSON.md"))] {
        assert!(doc.contains("tb export"), "{name} never shows tb export");
    }
    assert!(include_str!("../docs/JSON.md").contains("formula"), "docs/JSON.md states the CSV formula guard");
}
