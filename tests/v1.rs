//! v1.0.0 features: delete, reorder/shift-move, edit, GitHub auto-move + forced done, help.
mod common;
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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
    let mut f = EditForm {
        id: 1,
        title: "abc".into(),
        open_title: "abc".into(),
        due: String::new(),
        open_due: String::new(),
        desc: String::new(),
        open_desc: String::new(),
        field: 0,
        cursor: 3,
        from_popup: false,
    };
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
    // tab now walks three fields: title -> due -> description -> title
    f.key(KeyCode::Tab);
    assert_eq!((f.field, f.cursor), (1, 0), "the due field comes after the title");
    for c in "2026-10-09".chars() {
        f.key(KeyCode::Char(c));
    }
    assert_eq!((f.due.as_str(), f.cursor), ("2026-10-09", 10));
    f.key(KeyCode::Tab);
    assert_eq!((f.field, f.cursor), (2, 0));
    for c in "héllo".chars() {
        f.key(KeyCode::Char(c));
    }
    f.key(KeyCode::Left);
    f.key(KeyCode::Backspace);
    assert_eq!(f.desc, "hélo", "multi-byte safe");
    f.key(KeyCode::Tab);
    assert_eq!((f.field, f.cursor), (0, 2));
    // and shift+tab walks back the other way
    f.key(KeyCode::BackTab);
    assert_eq!(f.field, 2, "back from the title is the description");
    f.key(KeyCode::BackTab);
    assert_eq!((f.field, f.cursor), (1, 10), "and back again is the due field, cursor at its end");
}

#[test]
fn edit_key_saves_and_reparses_the_tag() {
    common::pin_clock();
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
    // tab twice: the due field sits between the title and the description
    app.handle_key(key(KeyCode::Tab), &mut s);
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
    common::pin_clock();
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
    // TODO cards have no owner: an open PR leaves them in TODO, a closed issue still moves
    // them to DONE; DOING cards (owned by 'me') move on an open PR as before
    let linked = mk(&mut s, "gh#10 linked by closes", "todo");
    let branch = mk(&mut s, "gh#12 linked by branch", "doing");
    let merged = mk(&mut s, "gh#20 a merged pr", "doing");
    let closed = mk(&mut s, "gh#21 a closed issue", "todo");
    let open_rev = mk(&mut s, "gh#11 open, in review", "review");
    let unmerged = mk(&mut s, "gh#22 closed unmerged pr", "doing");
    let linked_owned = mk(&mut s, "gh#13 linked by closes, taken", "doing");
    let done_open = mk(&mut s, "gh#10 already done", "done");
    let plain = mk(&mut s, "no gh ref", "doing");
    let snap = GhSnapshot {
        repo: "o/r".into(),
        prs: vec![pr(30, &[10], "x"), pr(31, &[], "fix/12-thing"), pr(32, &[13], "y")],
        issues: vec![issue(10), issue(11), issue(12), issue(13)],
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
            (branch, "review", "PR gh#31 open → review"),
            (merged, "done", "PR gh#20 merged → done"),
            (closed, "done", "issue gh#21 closed → done"),
            (linked_owned, "review", "PR gh#32 open → review"),
        ]
    );
    // the unowned TODO card with an open PR stays in TODO
    let untouched = [linked, open_rev, unmerged, done_open, plain];
    assert!(moves.iter().all(|m| !untouched.contains(&m.card_id)), "no backwards, no evidence, no move");
    terminal_board::github::apply_moves(&mut s, &moves).unwrap();
    assert_eq!(s.card(merged).unwrap().column, "done");
    let ev = s.show(merged).unwrap().events;
    assert!(ev.iter().any(|e| e.actor == "github" && e.text == "PR gh#20 merged → done"));
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
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("prs.json"), r#"[{"number":30,"title":"fix","headRefName":"x","isDraft":false,"reviewDecision":"","statusCheckRollup":[],"createdAt":"2026-09-18T08:00:00Z","author":{"login":"bot"},"closingIssuesReferences":[{"number":10}]}]"#).unwrap();
    std::fs::write(d.join("issues.json"), r#"[{"number":10,"title":"bug","labels":[],"assignees":[],"createdAt":"2026-09-18T07:00:00Z"}]"#).unwrap();
    let gh = script(
        d,
        &format!(
            r#"#!/bin/sh
case "$1 $2" in
  "repo view") echo '{{"nameWithOwner":"o/r"}}';;
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
    // an open PR moves only work someone took; merged/closed move unowned TODO cards too
    tb(&db, &gh, &["take", "1"]);
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
    common::pin_clock();
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
    assert!(render(&app, 140, 40).contains("issue gh#11 still open on GitHub — mark done anyway? y/n"));
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
    assert!(e.contains("issue gh#11 still open on GitHub") && e.contains("--force"), "{e}");
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
    common::pin_clock();
    let (_d, mut s, mut app) = board(&["a"]);
    let screen = render(&app, 160, 50);
    assert!(screen.contains("a add  e edit  x del  enter open  shift+arrows move") && screen.contains("? help  q quit"), "{screen}");
    // 60 columns is the focus shape: its footer keeps the focus arrow axis (issue: arrow
    // behaviour and help text agree in every view), dropping `enter open` to fit
    let narrow = render(&app, 60, 25);
    let footer = narrow.lines().last().unwrap_or("");
    assert!(footer.contains("arrows card/col") && footer.contains("shift+<> move") && footer.contains("? help"), "{narrow}");
    assert!(!footer.contains("shift+arrows"), "{narrow}");
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
fn a_ref_outside_the_newest_page_still_moves() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = s.add("plain: gh#777 old issue with an open PR", "", &[], "lead").unwrap();
    s.take(id, "bot-1").unwrap();
    // the snapshot page holds PRs 900..; the board's ref 777 is NOT in it
    let snap = GhSnapshot {
        repo: "acme/widgets".into(),
        fetched_at: terminal_board::store::now(),
        issues_open: 63,
        prs: (900..920)
            .map(|n| Pr {
                number: n,
                title: format!("pr {n}"),
                head_ref: format!("fix/{n}"),
                is_draft: false,
                review: "-".into(),
                ci: "ok".into(),
                created_at: "2026-09-18T07:00:00Z".into(),
                author: "x".into(),
                closes: vec![],
                updated_at: String::new(),
            })
            .collect(),
        issues: vec![],
        ..Default::default()
    };
    let cards = s.list().unwrap();
    // needs_state: 777 is not on the page, so a per-number lookup is planned
    assert_eq!(terminal_board::github::needs_state(&snap, &cards), vec![777]);
    let states: std::collections::HashMap<i64, terminal_board::github::RefState> =
        [(777, terminal_board::github::RefState { closed: false, pr: true, merged: false })].into();
    let moves = terminal_board::github::plan_moves(&snap, &cards, &states, &HashMap::new());
    assert!(
        moves.iter().any(|m| m.card_id == id && m.to == "review" && m.text == "PR gh#777 open → review"),
        "off-page open PR moves: {moves:?}"
    );
    // and a page-sized repo is labelled "newest" so 20 never reads as the total
    let (head, _) = terminal_board::github::summary(&snap);
    assert!(head.contains("PRs 20 newest"), "{head}");
    let small = GhSnapshot { prs: snap.prs[..3].to_vec(), ..snap.clone() };
    let (head, _) = terminal_board::github::summary(&small);
    assert!(head.contains("PRs 3 open ·") && !head.contains("newest"), "{head}");
}

#[test]
fn ref_lookups_report_missing_only_on_a_real_404() {
    use terminal_board::github::{classify_lookup, found_states, RefLookup, RefState};
    let closed = r#"{"state":"closed"}"#.to_string();
    assert_eq!(classify_lookup(Ok(closed)), RefLookup::Found(RefState { closed: true, pr: false, merged: false }));
    assert_eq!(classify_lookup(Err("gh: Not Found (HTTP 404)".into())), RefLookup::Missing);
    // network, rate limit, auth: nothing is known, so it is never reported as missing
    for e in ["error connecting to api.github.com", "HTTP 403: API rate limit exceeded", "HTTP 401: Bad credentials", "gh api timed out"] {
        assert!(matches!(classify_lookup(Err(e.into())), RefLookup::Failed(_)), "{e}");
    }
    assert!(matches!(classify_lookup(Ok("not json".into())), RefLookup::Failed(_)));
    let l = vec![(1, RefLookup::Missing), (2, RefLookup::Found(RefState { closed: true, pr: true, merged: true })), (3, RefLookup::Failed("x".into()))];
    assert_eq!(found_states(&l).keys().copied().collect::<Vec<_>>(), [2]);
}

#[test]
fn non_owner_done_drop_move_are_refused() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let mut s = Store::open(&db).unwrap();
    let a = s.add("plain: one", "", &[], "lead").unwrap();
    let b = s.add("plain: two", "", &[], "lead").unwrap();
    s.take(a, "bot-1").unwrap();
    s.take(b, "bot-2").unwrap();
    // non-owner done: refused with holder + your-cards + force hint
    let e = s.done(b, "bot-1").unwrap_err().to_string();
    assert!(e.contains(&format!("#{b} is held by bot-2")), "{e}");
    assert!(e.contains("your cards:"), "{e}");
    assert!(e.contains("--force"), "{e}");
    // owner path unaffected
    let c = s.done(b, "bot-2").unwrap();
    assert_eq!(c.column, "review");
    // non-owner drop: refused
    let e = s.drop_card(a, "bot-2").unwrap_err().to_string();
    assert!(e.contains("is held by bot-1"), "{e}");
    // non-owner move out of DOING: refused
    let e = s.move_to(a, "review", "bot-2").unwrap_err().to_string();
    assert!(e.contains("is held by bot-1"), "{e}");
    // --force works and is logged as its own event
    let c = s.move_to_forced(a, "review", "bot-2").unwrap();
    assert_eq!(c.column, "review");
    let ev = s.show(a).unwrap().events;
    assert!(ev.iter().any(|e| e.kind == "force" && e.text.contains("held by bot-1")), "{ev:?}");
    // REVIEW moves by reviewers are unaffected (another agent can review it to done)
    let d = s.move_to(a, "done", "bot-3").unwrap();
    assert_eq!(d.column, "done");
}

#[test]
fn tui_asks_before_moving_another_agents_doing_card() {
    common::pin_clock();
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let key = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
    let shift = |c: KeyCode| KeyEvent::new(c, KeyModifiers::SHIFT);
    // each key moves bot-1's DOING card for bot-2: it asks first, then goes where the key
    // pointed (left = TODO, right = REVIEW), on the forced, logged path
    for (press, want) in [
        (shift(KeyCode::Left), "todo"),
        (shift(KeyCode::Right), "review"),
        (key(KeyCode::Char('<')), "todo"),
        (key(KeyCode::Char('>')), "review"),
        (key(KeyCode::Char('d')), "review"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("b.db")).unwrap();
        let id = s.add("plain: theirs", "", &[], "lead").unwrap();
        s.take(id, "bot-1").unwrap();
        let mut app = App::new(s.snapshot().unwrap(), "bot-2");
        app.agents = AgentsState::Unavailable("x".into());
        app.reload(&s);
        app.focus_card(id);
        app.handle_key(press, &mut s);
        let Mode::Confirm { prompt, .. } = app.mode.clone() else {
            panic!("{press:?}: asks y/n instead of moving: {:?}, card in {}", app.mode, s.card(id).unwrap().column)
        };
        assert!(prompt.contains("held by bot-1") && prompt.contains(&format!("to {want}")) && prompt.contains("y/n"), "{prompt}");
        assert_eq!(s.card(id).unwrap().column, "doing", "{press:?}: nothing moves before y");
        // n leaves it
        app.handle_key(key(KeyCode::Char('n')), &mut s);
        assert_eq!(s.card(id).unwrap().column, "doing");
        // y moves it where the key pointed, logged
        app.focus_card(id);
        app.handle_key(press, &mut s);
        app.handle_key(key(KeyCode::Char('y')), &mut s);
        assert_eq!(s.card(id).unwrap().column, want, "{press:?}");
        let ev = s.show(id).unwrap().events;
        assert!(ev.iter().any(|e| e.kind == "force" && e.actor == "bot-2"), "{press:?}: {ev:?}");
    }
    // the holder's own keys never ask
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = s.add("plain: mine", "", &[], "lead").unwrap();
    s.take(id, "bot-1").unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "bot-1");
    app.agents = AgentsState::Unavailable("x".into());
    app.reload(&s);
    app.focus_card(id);
    app.handle_key(shift(KeyCode::Right), &mut s);
    assert!(matches!(app.mode, Mode::Normal), "{:?}", app.mode);
    assert_eq!(s.card(id).unwrap().column, "review");
}

#[test]
fn auto_move_event_text_carries_no_repeated_source() {
    common::pin_clock();
    // the actor is `github` and the kind is `github`; the text must not repeat "github:"
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let mut s = Store::open(&db).unwrap();
    let id = s.add("plain: merge me", "", &[], "me").unwrap();
    s.take(id, "me").unwrap();
    let _ = s.note_kind(id, "github", "PR gh#9 merged → done", "github");
    let ev = s.show(id).unwrap().events;
    let e = ev.iter().find(|e| e.actor == "github" && e.kind == "github").unwrap();
    assert!(!e.text.starts_with("github: "), "no triple 'github' in one line: {}", e.text);
    assert!(e.text.starts_with("PR gh#"), "{}", e.text);
}

#[test]
fn approve_refuses_the_author() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let mut s = Store::open(&db).unwrap();
    let id = s.add("plain: mine", "", &[], "lead").unwrap();
    s.take(id, "bot-1").unwrap();
    s.done(id, "bot-1").unwrap(); // bot-1 moved it to review: the author
    let gh = Path::new("/nonexistent/gh");
    let ids = id.to_string();
    let as_ = |who: &str| {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(["done", &ids, "--approve"])
            .env("TB_DB", &db)
            .env("TB_GH", gh)
            .env("TB_AS", who)
            .env("TB_NO_HERDR", "1")
            .output()
            .unwrap()
    };
    // the author is refused through the CLI, and nothing is recorded
    let o = as_("BOT-1");
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("you did this work"), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(!s.show(id).unwrap().events.iter().any(|e| e.kind == "approved"));
    // another agent's approval is recorded; the card stays in review
    let o = as_("rev");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(s.show(id).unwrap().events.iter().any(|e| e.kind == "approved" && e.actor == "rev"));
    assert_eq!(s.card(id).unwrap().column, "review");
    // a card that is not in review cannot be approved
    let doing = s.add("plain: still working", "", &[], "lead").unwrap();
    s.take(doing, "bot-1").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(["done", &doing.to_string(), "--approve"])
        .env("TB_DB", &db)
        .env("TB_GH", gh)
        .env("TB_AS", "rev")
        .env("TB_NO_HERDR", "1")
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("not in review"), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(s.card(doing).unwrap().column, "doing");
    assert!(!s.show(doing).unwrap().events.iter().any(|e| e.kind == "approved"));
}

#[test]
fn form_save_writes_only_changed_fields_and_refuses_conflicts() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = s.add("plain: first card", "original desc", &[], "lead").unwrap();
    // the form opened with these values
    let (open_title, open_desc) = ("plain: first card", "original desc");
    // an agent edits the description while the form is open
    s.edit(id, None, Some("new brief from an agent"), "bot-1", None).unwrap();
    // person adds '!' to the title only: the unchanged (stale) desc must NOT be written
    let typed_title = "plain: first card!";
    let c = s
        .edit(id, Some(typed_title), Some(open_desc), "lead", Some((open_title, open_desc)))
        .unwrap();
    assert_eq!(c.title, "first card!", "tag/plain: is stripped by parse");
    assert_eq!(c.description, "new brief from an agent", "the agent's desc survives");
    let ev = s.show(id).unwrap().events;
    let e = ev.iter().rev().find(|e| e.kind == "edit").unwrap();
    assert_eq!(e.text, "title edited", "the log names only the written field: {e:?}");
    // a REAL conflict: the person also changed the desc, which moved since open — refused
    let e = s
        .edit(id, Some(typed_title), Some("my own wording"), "lead", Some((open_title, open_desc)))
        .unwrap_err()
        .to_string();
    assert!(e.contains("changed while you were editing") && e.contains("reopen with e"), "{e}");
    // nothing was overwritten by the refused save
    assert_eq!(s.card(id).unwrap().description, "new brief from an agent");
    // negative control on the CLI path: no baseline, unchanged field writes as before
    let c2 = s.edit(id, None, Some("cli desc wins"), "bot-2", None).unwrap();
    assert_eq!(c2.description, "cli desc wins");
}

#[test]
fn sync_never_moves_an_unowned_todo_card_to_review() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    // unowned todo card linked to an issue that has an open PR
    let unowned = s.add("plain: gh#60 nobody took it", "", &[], "lead").unwrap();
    // an owned one, for the positive control
    let owned = s.add("plain: gh#61 taken", "", &[], "lead").unwrap();
    s.take(owned, "bot-1").unwrap();
    let snap = GhSnapshot {
        repo: "acme/widgets".into(),
        fetched_at: terminal_board::store::now(),
        issues_open: 2,
        // each card's issue has its own open PR: the unowned one (#62 closes #60) moved on main
        prs: vec![pr(62, &[60], "fix/60"), pr(61, &[61], "fix/61")],
        issues: vec![issue(60), issue(61)],
        ..Default::default()
    };
    let states: std::collections::HashMap<i64, terminal_board::github::RefState> = [
        (60, terminal_board::github::RefState { pr: true, merged: false, closed: false }),
        (61, terminal_board::github::RefState { pr: true, merged: false, closed: false }),
    ]
    .into();
    let moves = terminal_board::github::plan_moves(&snap, &s.list().unwrap(), &states, &HashMap::new());
    assert!(moves.iter().all(|m| m.card_id != unowned), "unowned card stays in todo: {moves:?}");
    assert!(moves.iter().any(|m| m.card_id == owned && m.to == "review"), "owned card moves: {moves:?}");
    terminal_board::github::apply_moves(&mut s, &moves).unwrap();
    assert_eq!(s.card(unowned).unwrap().column, "todo", "still todo, still unowned");
    assert_eq!(s.card(unowned).unwrap().owner, None);
    assert_eq!(s.card(owned).unwrap().column, "review");
    // and the self-approval check has an author for every synced REVIEW card
    for c in s.list().unwrap().into_iter().filter(|c| c.column == "review") {
        assert!(s.author(c.id).unwrap().is_some(), "#{} in review without an author", c.id);
    }
}

#[test]
fn wip_full_message_is_actor_aware() {
    common::pin_clock();
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

/// Issue #34's repro through `tb sync` and a fake gh: the page holds the newest 20 open PRs;
/// an older issue's open PR (#950, `closes #777`) is only found by the sync-only lookup.
#[test]
fn sync_finds_an_older_issues_pr_beyond_the_newest_page() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let page: Vec<String> = (900..920)
        .map(|n| format!(r#"{{"number":{n},"title":"pr {n}","headRefName":"feat/x{n}","isDraft":false,"reviewDecision":"","statusCheckRollup":[],"createdAt":"2026-09-18T08:00:00Z","author":{{"login":"bot"}},"closingIssuesReferences":[]}}"#))
        .collect();
    std::fs::write(d.join("page.json"), format!("[{}]", page.join(","))).unwrap();
    std::fs::write(d.join("pr950.json"), r#"[{"number":950,"title":"fix the old one","headRefName":"feat/y","isDraft":false,"reviewDecision":"","statusCheckRollup":[],"createdAt":"2026-09-01T08:00:00Z","author":{"login":"bot"},"closingIssuesReferences":[{"number":777}]}]"#).unwrap();
    let issues: Vec<String> = (700..720)
        .chain([777, 778])
        .map(|n| format!(r#"{{"number":{n},"title":"issue {n}","labels":[],"assignees":[],"createdAt":"2026-09-01T07:00:00Z"}}"#))
        .collect();
    std::fs::write(d.join("issues.json"), format!("[{}]", issues.join(","))).unwrap();
    let gh = script(
        d,
        &format!(
            r#"#!/bin/sh
echo "$*" >> {p}/calls.log
case "$*" in
  *"--search 777"*) cat {p}/pr950.json; exit 0;;
  *"--search"*"merged"*) echo '[]'; exit 0;;
  *"--search"*) echo '[]'; exit 0;;
esac
case "$1 $2" in
  "repo view") echo '{{"nameWithOwner":"o/r"}}';;
  "pr list") cat {p}/page.json;;
  "issue list") cat {p}/issues.json;;
  "run list") echo '[]';;
  "api repos/o/r/issues/777") echo '{{"state":"open"}}';;
  "api repos/o/r/issues/778") echo '{{"state":"open"}}';;
  api*) echo 60;;
  *) exit 2;;
esac
"#,
            p = d.display()
        ),
    );
    let db = d.join("b.db");
    tb(&db, &gh, &["add", "gh#777 an older issue"]);
    tb(&db, &gh, &["add", "gh#778 an issue with no PR"]);
    tb(&db, &gh, &["take", "1"]);
    tb(&db, &gh, &["take", "2"]);
    tb(&db, &gh, &["config", "github", "o/r"]);
    // the panel: a full page is labelled, and the refresh path never runs the extra lookup
    let o = tb(&db, &gh, &["github", "--refresh"]);
    let text = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(text.contains("PRs 20 newest"), "{text}");
    let log = std::fs::read_to_string(d.join("calls.log")).unwrap();
    assert!(!log.contains("--search 777"), "no extra lookup on refresh: {log}");
    // sync: the card for issue 777 moves on its off-page PR; 778 has none and stays
    let o = tb(&db, &gh, &["sync", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let moves: Vec<(i64, String, String)> = v["moves"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["card_id"].as_i64().unwrap(), m["to"].as_str().unwrap().into(), m["text"].as_str().unwrap().into()))
        .collect();
    assert_eq!(moves, [(1, "review".to_string(), "PR gh#950 open → review".to_string())]);
}
