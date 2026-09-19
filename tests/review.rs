//! v2: the agent that did the work cannot approve its own REVIEW card.
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::Path;
use std::process::{Command, Output};
use terminal_board::herdr::AgentsState;
use terminal_board::store::Store;
use terminal_board::tui::{App, Confirm, Mode};

const REFUSED: &str = "you did this work — ask another person or agent to review it";

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

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn kinds(s: &Store, id: i64) -> Vec<String> {
    s.show(id).unwrap().events.into_iter().map(|e| e.kind).collect()
}

/// A card that `worker` took and sent to REVIEW.
fn in_review(s: &mut Store, worker: &str) -> i64 {
    let id = s.add("widgets: fix the thing", "", &[], "lead").unwrap();
    s.take(id, worker).unwrap();
    assert_eq!(s.done(id, worker).unwrap().column, "review");
    id
}

#[test]
fn store_refuses_the_author_and_accepts_another_agent() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = in_review(&mut s, "bot-1");
    assert_eq!(s.author(id).unwrap().as_deref(), Some("bot-1"));
    // both store paths refuse the author, case-insensitively
    let e = s.done(id, "bot-1").unwrap_err().to_string();
    assert!(e.contains(REFUSED) && !e.contains("--review"), "{e}");
    assert!(s.move_to(id, "done", "BOT-1").unwrap_err().to_string().contains(REFUSED));
    assert_eq!(s.card(id).unwrap().column, "review");
    // another agent approves; nothing is logged as forced
    assert_eq!(s.done(id, "bot-2").unwrap().column, "done");
    assert!(!kinds(&s, id).contains(&"force".to_string()));
    // other moves out of review stay open to the author
    let id = in_review(&mut s, "bot-1");
    assert_eq!(s.move_to(id, "doing", "bot-1").unwrap().column, "doing");
    // forced: allowed, and logged as its own event
    assert_eq!(s.done(id, "bot-1").unwrap().column, "review");
    assert_eq!(s.done_forced(id, "bot-1").unwrap().column, "done");
    let ev = s.show(id).unwrap().events;
    let f = ev.iter().find(|e| e.kind == "force").expect("force event");
    assert_eq!((f.actor.as_str(), f.text.as_str()), ("bot-1", "approved own work"));
}

#[test]
fn author_is_the_mover_or_the_owner_after_github_sync() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    // a person moved bot-1's card to review (forced: not the owner): the person is the author
    let id = s.add("a", "", &[], "lead").unwrap();
    s.take(id, "bot-1").unwrap();
    s.move_to_forced(id, "review", "lead").unwrap();
    assert_eq!(s.author(id).unwrap().as_deref(), Some("lead"));
    assert!(s.done(id, "lead").is_err());
    assert_eq!(s.done(id, "bot-1").unwrap().column, "done");
    // GitHub sync moved it: the owner is the author
    let id = s.add("b", "", &[], "lead").unwrap();
    s.take(id, "bot-1").unwrap();
    s.move_to(id, "review", "github").unwrap();
    assert_eq!(s.author(id).unwrap().as_deref(), Some("bot-1"));
    assert!(s.done(id, "bot-1").unwrap_err().to_string().contains(REFUSED));
    assert_eq!(s.done(id, "lead").unwrap().column, "done");
    // todo -> done has no author to protect
    let id = s.add("c", "", &[], "lead").unwrap();
    assert_eq!(s.done(id, "lead").unwrap().column, "done");
}

#[test]
fn cli_refuses_on_done_and_move_and_force_is_logged() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let mut s = Store::open(&db).unwrap();
    let id = in_review(&mut s, "bot-1");
    let ids = id.to_string();
    // tb done
    let o = tb(&db, "bot-1", &["done", &ids]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains(REFUSED), "{}", stderr(&o));
    // tb move ID done, as JSON: error + hint
    let o = tb(&db, "bot-1", &["move", &ids, "done", "--json"]);
    assert!(!o.status.success());
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"], "you did this work");
    assert_eq!(v["hint"], "ask another person or agent to review it");
    assert_eq!(s.card(id).unwrap().column, "review");
    // --force works on both paths and is logged
    assert!(tb(&db, "bot-1", &["move", &ids, "done", "--force"]).status.success());
    assert_eq!(s.card(id).unwrap().column, "done");
    assert!(kinds(&s, id).contains(&"force".to_string()));
    let id = in_review(&mut s, "bot-1");
    assert!(tb(&db, "bot-1", &["done", &id.to_string(), "--force"]).status.success());
    assert!(kinds(&s, id).contains(&"force".to_string()));
    // another agent passes without --force
    let id = in_review(&mut s, "bot-1");
    let o = tb(&db, "bot-2", &["done", &id.to_string()]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(s.card(id).unwrap().column, "done");
    assert!(!kinds(&s, id).contains(&"force".to_string()));
}

#[test]
fn tui_asks_the_author_before_approving_own_work() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
    let id = in_review(&mut s, "bot-1");
    let mut app = App::new(s.snapshot().unwrap(), "bot-1");
    app.agents = AgentsState::Unavailable("x".into());
    app.reload(&s);
    app.focus_card(id);
    // d asks; n leaves the card alone
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    let Mode::Confirm { action, prompt } = app.mode.clone() else { panic!("no confirm: {:?}", app.mode) };
    assert_eq!(action, Confirm::ApproveOwn(id));
    assert_eq!(prompt, "you moved this to review yourself — approve your own work? y/n");
    app.handle_key(key(KeyCode::Char('n')), &mut s);
    assert_eq!(s.card(id).unwrap().column, "review");
    assert!(!kinds(&s, id).contains(&"force".to_string()));
    // shift+right asks too
    app.focus_card(id);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT), &mut s);
    assert!(matches!(app.mode, Mode::Confirm { action: Confirm::ApproveOwn(_), .. }), "{:?}", app.mode);
    app.handle_key(key(KeyCode::Esc), &mut s);
    assert_eq!(s.card(id).unwrap().column, "review");
    // `>` asks too; y approves on the forced, logged path
    app.focus_card(id);
    app.handle_key(key(KeyCode::Char('>')), &mut s);
    assert!(matches!(app.mode, Mode::Confirm { action: Confirm::ApproveOwn(_), .. }));
    app.handle_key(key(KeyCode::Char('y')), &mut s);
    assert_eq!(s.card(id).unwrap().column, "done");
    let ev = s.show(id).unwrap().events;
    assert!(ev.iter().any(|e| e.kind == "force" && e.actor == "bot-1"), "{ev:?}");
    // another agent's d approves without asking and without a force event
    let id = in_review(&mut s, "bot-1");
    app.actor = "bot-2".into();
    app.reload(&s);
    app.focus_card(id);
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    assert!(matches!(app.mode, Mode::Normal), "{:?}", app.mode);
    assert_eq!(s.card(id).unwrap().column, "done");
    assert!(!kinds(&s, id).contains(&"force".to_string()));
}
