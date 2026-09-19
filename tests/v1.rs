//! v1.0.0 features: delete, reorder/shift-move, edit, GitHub auto-move + forced done, help.
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use terminal_board::github::{plan_moves, GhSnapshot, Issue, Pr, RefState};
use terminal_board::herdr::AgentsState;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App, Confirm, EditForm, Mode};

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn shift(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::SHIFT)
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn board(titles: &[&str]) -> (tempfile::TempDir, Store, App) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    for t in titles {
        s.add(t, "", &[], "me").unwrap();
    }
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    app.reload(&s);
    (dir, s, app)
}

fn todo_ids(s: &Store) -> Vec<i64> {
    s.snapshot().unwrap().in_column("todo").iter().map(|c| c.id).collect()
}

// ---------- 1. delete ----------

#[test]
fn delete_asks_then_deletes() {
    let (_d, mut s, mut app) = board(&["one", "two"]);
    app.handle_key(key(KeyCode::Char('x')), &mut s);
    assert!(matches!(app.mode, Mode::Confirm { action: Confirm::Delete(1), .. }));
    assert!(render(&app, 140, 40).contains("delete #1 \"one\"? y/n"));
    app.handle_key(key(KeyCode::Char('n')), &mut s);
    assert_eq!(s.list().unwrap().len(), 2, "n cancels");
    app.handle_key(key(KeyCode::Char('x')), &mut s);
    app.handle_key(key(KeyCode::Char('y')), &mut s);
    assert_eq!(s.list().unwrap().iter().map(|c| c.id).collect::<Vec<_>>(), [2]);
    assert!(s.show(1).is_err());
    let log: Vec<String> = s.board_events().unwrap().into_iter().map(|e| e.3).collect();
    assert!(log.contains(&"deleted #1 \"one\"".to_string()), "{log:?}");
    assert!(s.delete_card(9, "me").unwrap_err().to_string().contains("tb list"));
}

// ---------- 2. reorder / shift-move ----------

#[test]
fn shift_arrows_and_jk_reorder_and_move() {
    let (_d, mut s, mut app) = board(&["a", "b", "c"]);
    render(&app, 140, 40);
    // Shift+Down moves #1 down; the selection follows
    app.handle_key(shift(KeyCode::Down), &mut s);
    assert_eq!(todo_ids(&s), [2, 1, 3]);
    assert_eq!(app.selected().unwrap().id, 1);
    // J / K fallbacks
    app.handle_key(key(KeyCode::Char('J')), &mut s);
    assert_eq!(todo_ids(&s), [2, 3, 1]);
    app.handle_key(key(KeyCode::Char('K')), &mut s);
    app.handle_key(shift(KeyCode::Up), &mut s);
    assert_eq!(todo_ids(&s), [1, 2, 3]);
    assert_eq!(app.selected().unwrap().id, 1);
    // Shift+Right: to the bottom of DOING, selection follows
    app.handle_key(shift(KeyCode::Right), &mut s);
    assert_eq!(s.card(1).unwrap().column, "doing");
    assert_eq!((app.col, app.selected().unwrap().id), (1, 1));
    app.handle_key(key(KeyCode::Left), &mut s);
    app.handle_key(shift(KeyCode::Right), &mut s);
    let doing: Vec<i64> = s.snapshot().unwrap().in_column("doing").iter().map(|c| c.id).collect();
    assert_eq!(doing, [1, 2], "moved card goes to the bottom");
    // WIP enforced on shift-move
    s.set_wip(2).unwrap();
    app.reload(&s);
    app.handle_key(key(KeyCode::Left), &mut s);
    app.handle_key(shift(KeyCode::Right), &mut s);
    assert_eq!(s.card(3).unwrap().column, "todo");
    assert!(app.status.as_ref().unwrap().0.contains("doing is full (2/2:"));
    // Shift+Left back
    app.col = 1;
    app.row[1] = 0;
    app.handle_key(shift(KeyCode::Left), &mut s);
    assert_eq!(s.card(1).unwrap().column, "todo");
}

#[test]
fn next_takes_the_top_by_position_and_prio_cli() {
    let (_d, mut s, _app) = board(&["old", "mid", "new"]);
    s.reorder(3, "top", "me").unwrap();
    assert_eq!(todo_ids(&s), [3, 1, 2]);
    s.block(3, Some("#9"), "me").unwrap();
    assert_eq!(s.next("me").unwrap().id, 1, "top unblocked card");
    s.reorder(2, "up", "me").unwrap();
    assert_eq!(todo_ids(&s), [2, 3]);
    s.reorder(2, "bottom", "me").unwrap();
    assert_eq!(todo_ids(&s), [3, 2]);
    s.reorder(2, "down", "me").unwrap();
    assert_eq!(todo_ids(&s), [3, 2], "already at the bottom");
    assert!(s.reorder(2, "sideways", "me").unwrap_err().to_string().contains("tb prio 2 top|bottom|up|down"));
}

#[test]
fn position_migration_orders_existing_cards_by_created_at() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("old.db");
    {
        let c = rusqlite::Connection::open(&p).unwrap();
        c.execute_batch(
            r#"CREATE TABLE cards (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL, tag TEXT,
                 description TEXT NOT NULL DEFAULT '', "column" TEXT NOT NULL DEFAULT 'todo', owner TEXT, due TEXT,
                 gh_ref INTEGER, created_at INTEGER NOT NULL, column_since INTEGER NOT NULL, blocked TEXT);
               INSERT INTO cards(title, created_at, column_since) VALUES ('newest', 300, 300), ('oldest', 100, 100), ('middle', 200, 200);"#,
        )
        .unwrap();
    }
    let mut s = Store::open(&p).unwrap();
    let order: Vec<String> = s.snapshot().unwrap().in_column("todo").iter().map(|c| c.title.clone()).collect();
    assert_eq!(order, ["oldest", "middle", "newest"]);
    assert_eq!(s.next("me").unwrap().title, "oldest");
    let id = s.add("added", "", &[], "me").unwrap();
    assert_eq!(*todo_ids(&s).last().unwrap(), id, "new cards go to the bottom");
}

// ---------- 3. edit ----------

#[test]
fn edit_form_cursor_ops() {
    let mut f = EditForm { id: 1, title: "abc".into(), desc: String::new(), field: 0, cursor: 3, from_popup: false };
    f.key(KeyCode::Home);
    f.key(KeyCode::Right);
    f.key(KeyCode::Delete); // removes 'b'
    assert_eq!((f.title.as_str(), f.cursor), ("ac", 1));
    f.key(KeyCode::Char('X'));
    assert_eq!((f.title.as_str(), f.cursor), ("aXc", 2));
    f.key(KeyCode::End);
    f.key(KeyCode::Backspace);
    assert_eq!((f.title.as_str(), f.cursor), ("aX", 2));
    f.key(KeyCode::Left);
    f.key(KeyCode::Left);
    f.key(KeyCode::Left);
    f.key(KeyCode::Backspace);
    assert_eq!((f.title.as_str(), f.cursor), ("aX", 0), "nothing before the start");
    f.key(KeyCode::Tab);
    assert_eq!((f.field, f.cursor), (1, 0));
    for c in "héllo".chars() {
        f.key(KeyCode::Char(c));
    }
    f.key(KeyCode::Left);
    f.key(KeyCode::Backspace);
    assert_eq!(f.desc, "hélo", "multi-byte safe");
    f.key(KeyCode::Tab);
    assert_eq!((f.field, f.cursor), (0, 2));
}

#[test]
fn edit_key_saves_and_reparses_the_tag() {
    let (_d, mut s, mut app) = board(&["widgets: gh#308 csv export"]);
    app.handle_key(key(KeyCode::Char('e')), &mut s);
    let Mode::Edit(f) = app.mode.clone() else { panic!("edit form") };
    assert_eq!(f.title, "widgets: gh#308 csv export", "prefilled with tag and gh ref");
    let screen = render(&app, 140, 40);
    assert!(screen.contains("Edit #1") && screen.contains("Description"), "{screen}");
    app.handle_key(key(KeyCode::Home), &mut s);
    for _ in 0.."widgets".len() {
        app.handle_key(key(KeyCode::Delete), &mut s);
    }
    for c in "admin".chars() {
        app.handle_key(key(KeyCode::Char(c)), &mut s);
    }
    app.handle_key(key(KeyCode::Tab), &mut s);
    for c in "a long description that wraps".chars() {
        app.handle_key(key(KeyCode::Char(c)), &mut s);
    }
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(app.mode, Mode::Normal);
    let c = s.card(1).unwrap();
    assert_eq!((c.tag.as_deref(), c.gh_ref, c.title.as_str()), (Some("admin"), Some(308), "csv export"));
    assert_eq!(c.description, "a long description that wraps");
    assert!(s.show(1).unwrap().events.iter().any(|e| e.kind == "edit"));
    // esc cancels; e works from the popup too
    app.handle_key(key(KeyCode::Enter), &mut s);
    app.handle_key(key(KeyCode::Char('e')), &mut s);
    app.handle_key(key(KeyCode::Char('!')), &mut s);
    app.handle_key(key(KeyCode::Esc), &mut s);
    assert_eq!(app.mode, Mode::Popup(1));
    assert_eq!(s.card(1).unwrap().title, "csv export");
}

// ---------- 4. GitHub auto-move ----------

fn pr(n: i64, closes: &[i64], branch: &str) -> Pr {
    Pr {
        number: n,
        title: format!("pr {n}"),
        head_ref: branch.into(),
        is_draft: false,
        review: "-".into(),
        ci: "ok".into(),
        created_at: "2026-09-18T08:00:00Z".into(),
        author: "bot".into(),
        closes: closes.to_vec(),
        updated_at: String::new(),
    }
}

fn issue(n: i64) -> Issue {
    Issue { number: n, title: format!("issue {n}"), labels: vec![], assignees: vec![], created_at: "2026-09-18T07:00:00Z".into() }
}

#[test]
fn auto_move_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let mk = |s: &mut Store, t: &str, col: &str| {
        let id = s.add(t, "", &[], "me").unwrap();
        if col != "todo" {
            s.move_to(id, col, "me").unwrap();
        }
        id
    };
    s.set_wip(99).unwrap();
    let linked = mk(&mut s, "gh#10 linked by closes", "todo");
    let branch = mk(&mut s, "gh#12 linked by branch", "doing");
    let merged = mk(&mut s, "gh#20 a merged pr", "doing");
    let closed = mk(&mut s, "gh#21 a closed issue", "todo");
    let open_rev = mk(&mut s, "gh#11 open, in review", "review");
    let unmerged = mk(&mut s, "gh#22 closed unmerged pr", "doing");
    let done_open = mk(&mut s, "gh#10 already done", "done");
    let plain = mk(&mut s, "no gh ref", "doing");
    let snap = GhSnapshot {
        repo: "o/r".into(),
        prs: vec![pr(30, &[10], "x"), pr(31, &[], "fix/12-thing")],
        issues: vec![issue(10), issue(11), issue(12)],
        ..Default::default()
    };
    let cards = s.list().unwrap();
    let needs = terminal_board::github::needs_state(&snap, &cards);
    assert_eq!(needs, [20, 21, 22], "only refs the open lists don't cover");
    let states: HashMap<i64, RefState> = [
        (20, RefState { closed: true, pr: true, merged: true }),
        (21, RefState { closed: true, pr: false, merged: false }),
        (22, RefState { closed: true, pr: true, merged: false }),
    ]
    .into();
    let moves = plan_moves(&snap, &cards, &states, &HashMap::new());
    let got: Vec<(i64, &str, &str)> = moves.iter().map(|m| (m.card_id, m.to.as_str(), m.text.as_str())).collect();
    assert_eq!(
        got,
        [
            (linked, "review", "github: PR #30 open → review"),
            (branch, "review", "github: PR #31 open → review"),
            (merged, "done", "github: PR #20 merged → done"),
            (closed, "done", "github: issue #21 closed → done"),
        ]
    );
    let untouched = [open_rev, unmerged, done_open, plain];
    assert!(moves.iter().all(|m| !untouched.contains(&m.card_id)), "no backwards, no evidence, no move");
    terminal_board::github::apply_moves(&mut s, &moves).unwrap();
    assert_eq!(s.card(merged).unwrap().column, "done");
    let ev = s.show(merged).unwrap().events;
    assert!(ev.iter().any(|e| e.actor == "github" && e.text == "github: PR #20 merged → done"));
    // a second pass moves nothing (review never goes back to review, done stays done)
    let moves = plan_moves(&snap, &s.list().unwrap(), &states, &HashMap::new());
    assert!(moves.is_empty(), "{moves:?}");
}

fn script(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join("gh-fake");
    std::fs::write(&p, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

fn tb(db: &Path, gh: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(args)
        .env("TB_DB", db)
        .env("TB_GH", gh)
        .env("TB_AS", "tester")
        .env("TB_NO_HERDR", "1")
        .output()
        .unwrap()
}

#[test]
fn sync_cli_moves_cards_via_fake_gh() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("prs.json"), r#"[{"number":30,"title":"fix","headRefName":"x","isDraft":false,"reviewDecision":"","statusCheckRollup":[],"createdAt":"2026-09-18T08:00:00Z","author":{"login":"bot"},"closingIssuesReferences":[{"number":10}]}]"#).unwrap();
    std::fs::write(d.join("issues.json"), r#"[{"number":10,"title":"bug","labels":[],"assignees":[],"createdAt":"2026-09-18T07:00:00Z"}]"#).unwrap();
    let gh = script(
        d,
        &format!(
            r#"#!/bin/sh
case "$1 $2" in
  "pr list") case "$*" in *merged*) echo '[]';; *) cat {p}/prs.json;; esac;;
  "issue list") cat {p}/issues.json;;
  "run list") echo '[]';;
  "api repos/o/r/issues/20") echo '{{"state":"closed","pull_request":{{"merged_at":"2026-09-18T09:00:00Z"}}}}';;
  "api repos/o/r/issues/21") echo '{{"state":"closed"}}';;
  api*) echo 1;;
  *) exit 2;;
esac
"#,
            p = d.display()
        ),
    );
    let db = d.join("b.db");
    for t in ["gh#10 linked", "gh#20 merged", "gh#21 closed"] {
        tb(&db, &gh, &["add", t]);
    }
    tb(&db, &gh, &["config", "github", "o/r"]);
    let o = tb(&db, &gh, &["sync", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], true);
    let moves: Vec<(i64, String)> =
        v["moves"].as_array().unwrap().iter().map(|m| (m["card_id"].as_i64().unwrap(), m["to"].as_str().unwrap().into())).collect();
    assert_eq!(moves, [(1, "review".to_string()), (2, "done".into()), (3, "done".into())]);
    let o = tb(&db, &gh, &["sync"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("nothing to move"));
}

#[test]
fn forced_done_prompt_and_cli_force() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let mut s = Store::open(&db).unwrap();
    s.add("widgets: gh#11 still open", "", &[], "me").unwrap();
    s.add("plain card", "", &[], "me").unwrap();
    s.set_github(Some("o/r")).unwrap();
    s.save_github(&Ok(GhSnapshot { repo: "o/r".into(), issues: vec![issue(11)], fetched_at: terminal_board::store::now(), ..Default::default() })).unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.agents = AgentsState::Unavailable("x".into());
    app.reload(&s);
    // TUI: d on a todo gh card whose issue is open asks first
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    assert!(matches!(app.mode, Mode::Confirm { action: Confirm::ForceDone(1), .. }));
    assert!(render(&app, 140, 40).contains("issue #11 still open on GitHub — mark done anyway? y/n"));
    app.handle_key(key(KeyCode::Char('n')), &mut s);
    assert_eq!(s.card(1).unwrap().column, "todo");
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    app.handle_key(key(KeyCode::Char('y')), &mut s);
    assert_eq!(s.card(1).unwrap().column, "done");
    // plain cards never ask
    app.focus_card(2);
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    assert_eq!(s.card(2).unwrap().column, "done");
    // CLI refuses without --force
    s.move_to(1, "review", "me").unwrap();
    let gh = Path::new("/nonexistent/gh");
    let o = tb(&db, gh, &["done", "1"]);
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("issue #11 still open on GitHub") && e.contains("--force"), "{e}");
    let o = tb(&db, gh, &["move", "1", "done", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert!(v["hint"].as_str().unwrap().contains("--force"));
    let o = tb(&db, gh, &["done", "1", "--force"]);
    assert!(o.status.success());
    assert_eq!(s.card(1).unwrap().column, "done");
}

// ---------- 5. help ----------

#[test]
fn help_overlay_and_footer() {
    let (_d, mut s, mut app) = board(&["a"]);
    let screen = render(&app, 160, 50);
    assert!(screen.contains("a add  e edit  x del  enter open  shift+arrows move") && screen.contains("? help  q quit"), "{screen}");
    let narrow = render(&app, 60, 25);
    assert!(narrow.contains("a add  enter open  ? help  q quit") && !narrow.contains("shift+arrows"), "{narrow}");
    app.handle_key(key(KeyCode::Char('?')), &mut s);
    assert_eq!(app.mode, Mode::Help);
    let screen = render(&app, 160, 50);
    for want in ["Terminal Board keys", "Board", "Cards", "Card popup", "Panels", "View", "CLI", "shift+up/down, K J", "x", "board --json"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    app.handle_key(key(KeyCode::Char('?')), &mut s);
    assert_eq!(app.mode, Mode::Normal);
    app.handle_key(key(KeyCode::Char('?')), &mut s);
    app.handle_key(key(KeyCode::Esc), &mut s);
    assert_eq!(app.mode, Mode::Normal);
    // panels have their own hints + ? help
    app.handle_key(key(KeyCode::Tab), &mut s);
}

#[test]
fn wip_full_message_is_actor_aware() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(2).unwrap();
    let a = s.add("plain: one", "", &[], "lead").unwrap();
    let b = s.add("plain: two", "", &[], "lead").unwrap();
    let c = s.add("plain: three", "", &[], "lead").unwrap();
    s.take(a, "bot-1").unwrap();
    s.take(b, "bot-2").unwrap();
    // an actor holding none: holders listed, no 'tb done' suggestion
    let e = s.take(c, "bot-4").unwrap_err().to_string();
    assert!(e.contains("doing is full (2/2: #1 bot-1, #2 bot-2)"), "{e}");
    assert!(e.contains("you hold none; wait, or ask one of them to finish"), "{e}");
    assert!(!e.contains("tb done"), "{e}");
    // an actor holding one: their card named, with the exact command
    let e = s.take(c, "bot-1").unwrap_err().to_string();
    assert!(e.contains("doing is full (2/2: #1 bot-1, #2 bot-2)"), "{e}");
    assert!(e.contains("finish #1 with 'tb done 1' first"), "{e}");
    // next with json: same message in the hint path
    let e = s.next("bot-2").unwrap_err().to_string();
    assert!(e.contains("finish #2 with 'tb done 2' first"), "{e}");
}
