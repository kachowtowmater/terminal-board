//! The holder rule on `rm` / `edit` / `block` (and the delete key), the reserved `github`
//! name, and soft delete: `tb config rm archive`, `tb list --archived`, `tb restore ID`.
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::path::PathBuf;
use std::process::{Command, Output};
use terminal_board::herdr::AgentsState;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App, Mode};

struct Board {
    dir: tempfile::TempDir,
}

impl Board {
    fn new() -> Board {
        Board { dir: tempfile::tempdir().unwrap() }
    }
    fn db(&self) -> PathBuf {
        self.dir.path().join("board.db")
    }
    /// `tb ARGS --as WHO` on the pinned scratch file, clock pinned.
    fn run(&self, who: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("HOME", self.dir.path().join("home"))
            .env("TB_DB", self.db())
            .env("TB_AS", who)
            .env("TB_NO_HERDR", "1")
            .env("TB_NOW", "1790000000")
            .env_remove("HERDR_AGENT_NAME")
            .env_remove("TB_BOARD")
            .output()
            .unwrap()
    }
    fn ok(&self, who: &str, args: &[&str]) -> String {
        let o = self.run(who, args);
        assert!(o.status.success(), "{args:?} as {who} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    }
    fn json(&self, who: &str, args: &[&str]) -> serde_json::Value {
        let o = self.run(who, args);
        serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{args:?}: not JSON ({e}): {}", String::from_utf8_lossy(&o.stdout)))
    }
    fn sql<T: rusqlite::types::FromSql>(&self, q: &str) -> T {
        rusqlite::Connection::open(self.db()).unwrap().query_row(q, [], |r| r.get(0)).unwrap()
    }
    fn kinds(&self, id: i64) -> Vec<(String, String)> {
        let v = self.json("reader", &["show", &id.to_string(), "--json"]);
        v["events"].as_array().unwrap().iter().map(|e| (e["kind"].as_str().unwrap().into(), e["text"].as_str().unwrap().into())).collect()
    }
}

fn errs(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

/// bot-1 holds #1 in DOING; #2 is an unheld TODO card.
fn held() -> Board {
    let b = Board::new();
    b.ok("lead", &["add", "t: one", "-d", "Done = x", "--check", "step"]);
    b.ok("lead", &["add", "t: two"]);
    b.ok("bot-1", &["take", "1"]);
    b
}

#[test]
fn rm_edit_and_block_refuse_a_card_someone_else_holds() {
    let b = held();
    let before = b.json("reader", &["show", "1", "--json"]);
    for (args, what) in [
        (vec!["rm", "1"], "delete it"),
        (vec!["edit", "1", "--title", "t: rewritten"], "edit it"),
        (vec!["edit", "1", "--desc", "Done = something else"], "edit it"),
        (vec!["edit", "1", "--due", "2026-10-09"], "edit it"),
        (vec!["block", "1", "#2"], "block it"),
        (vec!["block", "1", "--clear"], "unblock it"),
    ] {
        let o = b.run("bot-2", &args);
        assert!(!o.status.success(), "{args:?} must be refused");
        let e = errs(&o);
        assert!(e.contains("#1 is held by bot-1 — your cards: none"), "{args:?}: {e}");
        assert!(e.contains(&format!("to {what} anyway use --force (logged)")), "{args:?}: {e}");
        let mut j = args.clone();
        j.push("--json");
        let o = b.run("bot-2", &j);
        assert!(!o.status.success());
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["ok"], false, "{args:?}");
        assert_eq!(v["error"], "#1 is held by bot-1", "{args:?}");
        assert!(v["hint"].as_str().unwrap().contains("--force"), "{args:?}: {v}");
    }
    assert_eq!(b.json("reader", &["show", "1", "--json"]), before, "every refusal left the card exactly as it was");

    // open by design: adding to a held card is not taking it over
    b.ok("bot-2", &["note", "1", "evidence from someone else"]);
    b.ok("bot-2", &["check", "1", "1"]);
    b.ok("bot-2", &["prio", "1", "top"]);
    // a card nobody holds (TODO), and a REVIEW card, are open to anyone — the same rule moves follow
    b.ok("bot-2", &["edit", "2", "--title", "t: two, edited"]);
    b.ok("bot-2", &["block", "2", "waiting"]);
    b.ok("bot-2", &["block", "2", "--clear"]);
    b.ok("bot-1", &["done", "1"]);
    b.ok("bot-2", &["edit", "1", "--desc", "Done = reviewed wording"]);
    assert!(!b.kinds(1).iter().any(|(k, _)| k == "force"), "nothing here was forced");
}

#[test]
fn force_goes_through_and_is_logged_and_the_holder_needs_none() {
    let b = held();
    // the holder: no --force, no force event
    b.ok("bot-1", &["edit", "1", "--title", "t: one, by its holder"]);
    b.ok("bot-1", &["block", "1", "#2"]);
    b.ok("bot-1", &["block", "1", "--clear"]);
    assert!(!b.kinds(1).iter().any(|(k, _)| k == "force"));

    // someone else, forced: each change lands and is its own `force` event
    b.ok("bot-2", &["edit", "1", "--title", "t: one, forced", "--force"]);
    b.ok("bot-2", &["block", "1", "#2", "--force"]);
    b.ok("bot-2", &["block", "1", "--clear", "--force"]);
    let forced: Vec<String> = b.kinds(1).into_iter().filter(|(k, _)| k == "force").map(|(_, t)| t).collect();
    assert_eq!(forced, ["edited #1 held by bot-1", "blocked #1 held by bot-1", "unblocked #1 held by bot-1"]);
    assert_eq!(b.json("reader", &["show", "1", "--json"])["title"], "one, forced");

    // a forced rm destroys the card's own events, so the board log carries it
    let v = b.json("bot-2", &["rm", "1", "--force", "--json"]);
    assert_eq!(v["ok"], true);
    assert_eq!(v["card"]["id"], 1);
    assert!(v.get("archived").is_none(), "a plain delete answers exactly what it always did");
    assert!(!b.run("reader", &["show", "1"]).status.success());
    let n: i64 = b.sql("SELECT COUNT(*) FROM board_events WHERE kind='force' AND actor='bot-2' AND text='deleted #1 held by bot-1'");
    assert_eq!(n, 1);
    // the holder deleting their own card: no force row
    b.ok("bot-1", &["take", "2"]);
    assert_eq!(b.ok("bot-1", &["rm", "2"]).trim(), "deleted #2 \"two\"");
    assert_eq!(b.sql::<i64>("SELECT COUNT(*) FROM board_events WHERE kind='force'"), 1);
}

#[test]
fn the_name_github_is_not_a_way_past_the_holder_rule() {
    let b = held();
    for name in ["github", "GitHub"] {
        for args in [vec!["move", "1", "review"], vec!["drop", "1"], vec!["rm", "1"], vec!["note", "1", "x"]] {
            let o = b.run(name, &args);
            assert!(!o.status.success(), "{args:?} as {name}");
            assert!(errs(&o).contains("'github' is the name tb's own GitHub sync acts under"), "{}", errs(&o));
            assert!(errs(&o).contains("--as bot-1"), "{}", errs(&o));
        }
    }
    let v = b.json("github", &["move", "1", "review", "--json"]);
    assert_eq!(v["ok"], false);
    assert!(v["hint"].as_str().unwrap().contains("pass your own name"), "{v}");
    let card = b.json("reader", &["show", "1", "--json"]);
    assert_eq!((card["column"].as_str(), card["owner"].as_str()), (Some("doing"), Some("bot-1")), "untouched");
    assert_eq!(card["events"].as_array().unwrap().len(), 2, "created + taken, nothing else: {card}");
    // reading under that name is nobody's problem
    assert!(b.ok("github", &["list"]).contains("one"));
}

#[test]
fn the_default_is_todays_hard_delete_and_the_schema_is_untouched() {
    let b = held();
    assert_eq!(b.ok("lead", &["config", "rm"]).trim(), "delete");
    assert!(!b.ok("lead", &["config"]).contains("\nrm "), "hidden from the listing until the board sets it");
    assert_eq!(b.ok("lead", &["rm", "2"]).trim(), "deleted #2 \"two\"");
    assert!(b.ok("lead", &["list", "--archived"]).contains("no archived cards"));
    assert_eq!(b.json("lead", &["list", "--archived", "--json"]), serde_json::json!([]));
    let o = b.run("lead", &["restore", "2"]);
    assert!(!o.status.success());
    assert!(errs(&o).contains("no archived card #2 — see 'tb list --archived'"), "{}", errs(&o));
    assert_eq!(b.sql::<i64>("SELECT COUNT(*) FROM sqlite_master WHERE name='archived_cards'"), 0, "no table until asked for");
    let o = b.run("lead", &["config", "rm", "sometimes"]);
    assert!(!o.status.success());
    assert!(errs(&o).contains("is not delete|archive") && errs(&o).contains("'tb config rm archive'"), "{}", errs(&o));
}

#[test]
fn archive_and_restore_round_trip_keeps_the_card_and_its_whole_history() {
    let b = Board::new();
    assert_eq!(b.json("lead", &["config", "rm", "archive", "--json"])["config"], serde_json::json!({"key": "rm", "value": "archive"}));
    assert!(b.ok("lead", &["config"]).lines().any(|l| l.starts_with("rm ") && l.ends_with("archive")));
    // a card with everything on it
    b.ok("lead", &["add", "case: gh#7 file the brief", "-d", "Done = filed", "--check", "draft", "--check", "review", "--due", "2026-10-09"]);
    b.ok("lead", &["add", "case: another"]);
    b.ok("bot-1", &["take", "1"]);
    b.ok("bot-1", &["note", "1", "draft written"]);
    b.ok("bot-1", &["check", "1", "1"]);
    b.ok("bot-1", &["block", "1", "#2"]);
    b.ok("bot-1", &["done", "1"]);
    b.ok("rev", &["move", "1", "doing", "the exhibit list is missing"]);
    b.ok("bot-1", &["done", "1"]);
    let before = b.json("reader", &["show", "1", "--json"]);
    assert_eq!(before["column"], "review");

    // rm archives: gone from every view, listed in the archive
    let said = b.ok("lead", &["rm", "1"]);
    assert!(said.contains("archived #1 \"file the brief\"") && said.contains("'tb restore 1'"), "{said}");
    assert!(!b.run("reader", &["show", "1"]).status.success());
    assert!(!b.ok("reader", &["list"]).contains("file the brief"));
    assert!(!b.ok("reader", &["board", "--json"]).contains("file the brief"));
    let listed = b.ok("reader", &["list", "--archived"]);
    assert!(listed.contains("#1 file the brief") && listed.contains("was review") && listed.contains("by lead"), "{listed}");
    let a = b.json("reader", &["list", "--archived", "--json"]);
    assert_eq!(a.as_array().unwrap().len(), 1);
    assert_eq!(
        (a[0]["id"].as_i64(), a[0]["column"].as_str(), a[0]["owner"].as_str(), a[0]["archived_by"].as_str(), a[0]["archived_at"].as_i64()),
        (Some(1), Some("review"), Some("bot-1"), Some("lead"), Some(1790000000))
    );
    // --json on the write says it was archived (an additive field)
    let v = b.json("lead", &["rm", "2", "--json"]);
    assert_eq!((v["ok"].as_bool(), v["archived"].as_bool(), v["card"]["id"].as_i64()), (Some(true), Some(true), Some(2)));
    // ids are never reused, so a restored card cannot collide
    assert!(b.ok("lead", &["add", "case: a new one"]).contains("added #3"));

    // restore: the same card, the same history, plus the two events that say what happened
    assert!(b.ok("lead", &["restore", "1"]).contains("#1 restored to review with its history"));
    let after = b.json("reader", &["show", "1", "--json"]);
    for field in ["id", "title", "tag", "description", "column", "owner", "reviewer", "due", "gh_ref", "blocked", "created_at", "column_since", "checklist", "round"] {
        assert_eq!(after[field], before[field], "{field}");
    }
    let (was, now) = (before["events"].as_array().unwrap(), after["events"].as_array().unwrap());
    assert_eq!(&now[..was.len()], &was[..], "history identical, event for event");
    let tail: Vec<&str> = now[was.len()..].iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(tail, ["archived", "restored"]);
    assert!(b.json("reader", &["list", "--archived", "--json"]).as_array().unwrap().iter().all(|c| c["id"] != 1));
    let o = b.run("lead", &["restore", "1"]);
    assert!(!o.status.success(), "it is not in the archive any more");
    let v = b.json("lead", &["restore", "1", "--json"]);
    assert_eq!(v["ok"], false);
    assert!(v["hint"].as_str().unwrap().contains("tb list --archived"), "{v}");
    // the board log has both
    let log: String = b.sql("SELECT group_concat(kind || ':' || text, ' | ') FROM board_events WHERE kind IN ('archive','restore')");
    assert!(log.contains("archive:archived #1 \"file the brief\"") && log.contains("restore:restored #1 \"file the brief\""), "{log}");

    // back to the default: rm deletes again; what is archived stays restorable
    b.ok("lead", &["config", "rm", "delete"]);
    assert!(!b.ok("lead", &["config"]).contains("\nrm "));
    assert!(b.ok("lead", &["rm", "3"]).contains("deleted #3"));
    assert!(b.json("lead", &["restore", "2", "--json"])["ok"].as_bool().unwrap());
    assert_eq!(b.json("reader", &["show", "2", "--json"])["title"], "another");
}

#[test]
fn restoring_into_doing_respects_the_holder_rule_and_the_wip_limit() {
    let b = Board::new();
    b.ok("lead", &["config", "rm", "archive"]);
    b.ok("lead", &["config", "wip", "1"]);
    b.ok("lead", &["add", "t: held"]);
    b.ok("lead", &["add", "t: other"]);
    b.ok("bot-1", &["take", "1"]);
    // archiving someone's held card is refused like deleting it; forced, it is logged and kept
    let o = b.run("bot-2", &["rm", "1"]);
    assert!(!o.status.success());
    assert!(errs(&o).contains("#1 is held by bot-1") && errs(&o).contains("to archive it anyway use --force (logged)"), "{}", errs(&o));
    b.ok("bot-2", &["rm", "1", "--force"]);
    b.ok("bot-2", &["take", "2"]);
    let o = b.run("lead", &["restore", "1"]);
    assert!(!o.status.success(), "DOING is full");
    assert!(errs(&o).contains("doing is full (1/1: #2 bot-2)"), "{}", errs(&o));
    b.ok("bot-2", &["drop", "2"]);
    b.ok("lead", &["restore", "1"]);
    let c = b.json("reader", &["show", "1", "--json"]);
    assert_eq!((c["column"].as_str(), c["owner"].as_str()), (Some("doing"), Some("bot-1")), "back with its holder");
    let kinds: Vec<String> = b.kinds(1).into_iter().map(|(k, t)| format!("{k}:{t}")).collect();
    assert_eq!(kinds, ["created:", "taken:", "force:archived #1 held by bot-1", "archived:", "restored:"]);
}

#[test]
fn a_hint_names_the_board_it_was_run_on() {
    let home = tempfile::tempdir().unwrap();
    let tb = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("HOME", home.path())
            .env("TB_AS", "lead")
            .env("TB_NO_HERDR", "1")
            .env_remove("TB_DB")
            .env_remove("TTYBOARD_DB")
            .env_remove("TB_BOARD")
            .env_remove("HERDR_AGENT_NAME")
            .output()
            .unwrap()
    };
    assert!(tb(&["work", "config", "rm", "archive"]).status.success());
    assert!(tb(&["work", "add", "t: one"]).status.success());
    let o = tb(&["work", "rm", "1"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("'tb work restore 1'"), "{}", String::from_utf8_lossy(&o.stdout));
    assert!(String::from_utf8_lossy(&tb(&["work", "list", "--archived"]).stdout).contains("'tb work restore ID'"));
    assert!(tb(&["work", "restore", "1"]).status.success());
    // `restore` is a command now, so it cannot be mistaken for a board name
    let o = tb(&["restore", "add", "t: x"]);
    assert!(!o.status.success());
}

// ---------- the full-screen board ----------

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// A board file with two TODO cards, opened as `me`.
fn screen(setup: impl FnOnce(&mut Store)) -> (tempfile::TempDir, Store, App) {
    std::env::set_var("TB_NOW", "1790000000");
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.add("one", "", &[], "lead").unwrap();
    s.add("two", "", &[], "lead").unwrap();
    setup(&mut s);
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    app.reload(&s);
    (dir, s, app)
}

fn raw(dir: &tempfile::TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.path().join("b.db")).unwrap()
}

#[test]
fn the_delete_key_names_the_holder_of_a_card_that_is_not_yours() {
    let (dir, mut s, mut app) = screen(|s| {
        s.take(1, "bot-1").unwrap();
    });
    // DOING holds #1 (bot-1's): select it
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &mut s);
    app.handle_key(key('x'), &mut s);
    assert!(matches!(app.mode, Mode::Confirm { .. }));
    let shown = render(&app, 140, 40);
    assert!(shown.contains("#1 is held by bot-1 — delete it anyway? y/n (logged)"), "{shown}");
    app.handle_key(key('n'), &mut s);
    assert!(s.show(1).is_ok(), "n cancels");
    app.handle_key(key('x'), &mut s);
    app.handle_key(key('y'), &mut s);
    assert!(s.show(1).is_err(), "y still deletes");
    let forced: i64 = raw(&dir)
        .query_row("SELECT COUNT(*) FROM board_events WHERE kind='force' AND actor='me' AND text='deleted #1 held by bot-1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(forced, 1, "and it is logged as forced");

    // the edit key does not open a form on someone else's held card
    let (_d, mut s, mut app) = screen(|s| {
        s.take(1, "bot-1").unwrap();
    });
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &mut s);
    app.handle_key(key('e'), &mut s);
    assert!(matches!(app.mode, Mode::Normal), "no edit form");
    assert!(render(&app, 160, 40).contains("#1 is held by bot-1"), "{}", render(&app, 160, 40));
}

#[test]
fn the_delete_key_archives_on_an_archive_board() {
    let (dir, mut s, mut app) = screen(|_| {});
    raw(&dir).execute("INSERT INTO config(key, value) VALUES ('rm', 'archive')", []).unwrap();
    app.handle_key(key('x'), &mut s);
    let shown = render(&app, 140, 40);
    assert!(shown.contains("archive #1 \"one\"? y/n"), "{shown}");
    app.handle_key(key('y'), &mut s);
    assert!(s.show(1).is_err(), "off the board");
    let kept: i64 = raw(&dir).query_row("SELECT COUNT(*) FROM archived_cards WHERE card_id=1", [], |r| r.get(0)).unwrap_or(0);
    assert_eq!(kept, 1, "and in the archive");
    assert!(render(&app, 140, 40).contains("archived #1 \"one\""));
}
