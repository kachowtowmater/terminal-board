//! v2: the agent that did the work cannot approve its own REVIEW card.
/// A fake `gh` that answers `repo view` positively (for `config github`'s existence
/// check) and nothing else; shared by tests that pin `TB_GH` to a nonexistent path.
fn fake_gh_ok() -> std::path::PathBuf {
    use std::sync::OnceLock;
    static GH: OnceLock<std::path::PathBuf> = OnceLock::new();
    GH.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("tb-fake-gh-{}", std::process::id()));
        std::fs::write(&p, "#!/bin/sh\ncase \"$1 $2\" in\n  \"repo view\") echo '{\"nameWithOwner\":\"acme/widgets\"}';;\n  *) exit 0;;\nesac\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    })
    .clone()
}
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
        .env("TB_GH", fake_gh_ok())
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

/// How many `--force` overrides a card has logged so far (a force-MOVE into review logs one
/// too, so the approval tests count them rather than look for the kind).
fn forces(s: &Store, id: i64) -> usize {
    kinds(s, id).iter().filter(|k| *k == "force").count()
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
    assert_eq!(s.send_back(id, "not done yet", "bot-1").unwrap().column, "doing");
    // forced: allowed, and logged as its own event
    assert_eq!(s.done(id, "bot-1").unwrap().column, "review");
    assert_eq!(s.done_forced(id, "bot-1").unwrap().column, "done");
    let ev = s.show(id).unwrap().events;
    let f = ev.iter().find(|e| e.kind == "force").expect("force event");
    assert_eq!((f.actor.as_str(), f.text.as_str()), ("bot-1", "approved own work"));
}

#[test]
fn author_is_the_owner_and_falls_back_to_the_mover_only_when_unowned() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    // a person force-moved bot-1's card to review: bot-1 held it, so bot-1 did the work
    let id = s.add("a", "", &[], "lead").unwrap();
    s.take(id, "bot-1").unwrap();
    s.move_to_forced(id, "review", "lead").unwrap();
    assert_eq!(s.author(id).unwrap().as_deref(), Some("bot-1"));
    assert!(s.done(id, "bot-1").unwrap_err().to_string().contains(REFUSED));
    assert_eq!(s.done(id, "lead").unwrap().column, "done");
    // GitHub sync moved it: the owner is the author
    let id = s.add("b", "", &[], "lead").unwrap();
    s.take(id, "bot-1").unwrap();
    s.move_to(id, "review", "github").unwrap();
    assert_eq!(s.author(id).unwrap().as_deref(), Some("bot-1"));
    assert!(s.done(id, "bot-1").unwrap_err().to_string().contains(REFUSED));
    assert_eq!(s.done(id, "lead").unwrap().column, "done");
    // an UNOWNED card that reached review: the fallback names whoever moved it there
    let id = s.add("c", "", &[], "lead").unwrap();
    s.move_to(id, "review", "lead").unwrap();
    assert_eq!(s.card(id).unwrap().owner, None);
    assert_eq!(s.author(id).unwrap().as_deref(), Some("lead"));
    assert!(s.done(id, "lead").unwrap_err().to_string().contains(REFUSED));
    assert_eq!(s.done(id, "rev").unwrap().column, "done");
    // an unowned card the sync moved has no author: `github` never counts as the worker
    let id = s.add("d", "", &[], "lead").unwrap();
    s.move_to(id, "review", "github").unwrap();
    assert_eq!(s.author(id).unwrap(), None);
    assert_eq!(s.done(id, "lead").unwrap().column, "done");
    // todo -> done has no author to protect
    let id = s.add("e", "", &[], "lead").unwrap();
    assert_eq!(s.done(id, "lead").unwrap().column, "done");
}

/// A card bot-1 holds, force-moved into REVIEW by someone who is not its owner. This is the
/// state the `--force` escape hatch leaves behind, and where the author rule used to credit
/// the mover with work they did not do.
fn force_moved_to_review(s: &mut Store, worker: &str, mover: &str) -> i64 {
    let id = s.add("widgets: fix the thing", "", &[], "lead").unwrap();
    s.take(id, worker).unwrap();
    assert_eq!(s.move_to_forced(id, "review", mover).unwrap().column, "review");
    id
}

/// Matrix case 1: the holder moved its own card to REVIEW — still refused its own approval.
#[test]
fn case1_the_holder_that_moved_its_own_card_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = in_review(&mut s, "bot-1");
    assert_eq!(s.author(id).unwrap().as_deref(), Some("bot-1"));
    assert!(s.done(id, "bot-1").unwrap_err().to_string().contains(REFUSED));
    assert!(s.move_to(id, "done", "BOT-1").unwrap_err().to_string().contains(REFUSED));
    assert_eq!(s.card(id).unwrap().column, "review");
}

/// Matrix case 2: a third agent — neither holder nor mover — approves.
#[test]
fn case2_a_third_agent_approves() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = in_review(&mut s, "bot-1");
    assert_eq!(s.done(id, "rev").unwrap().column, "done");
    assert!(!kinds(&s, id).contains(&"force".to_string()));
}

/// Matrix case 3: the reviewer who only pushed someone else's card into REVIEW may approve
/// it — moving a column is not doing the work. This used to be refused (the lockout).
#[test]
fn case3_the_reviewer_who_force_moved_the_card_may_approve_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = force_moved_to_review(&mut s, "bot-1", "rev");
    assert_eq!(s.author(id).unwrap().as_deref(), Some("bot-1"), "the holder did the work");
    let before = forces(&s, id); // the force-move itself logged one
    let c = s.done(id, "rev").unwrap();
    assert_eq!(c.column, "done", "the mover is not the author and needs no --force");
    assert_eq!(forces(&s, id), before, "the approval itself was not forced: {:?}", kinds(&s, id));
}

/// Matrix case 4 — THE HOLE: after someone else moved the card, the agent that actually held
/// it in DOING is still refused its own approval. On 2.0.0 this was allowed.
#[test]
fn case4_the_worker_is_refused_after_someone_else_moved_the_card() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = force_moved_to_review(&mut s, "bot-1", "rev");
    let r = s.done(id, "bot-1");
    assert!(r.is_err(), "the worker that held the card must not approve its own work: {r:?}");
    assert!(r.unwrap_err().to_string().contains(REFUSED));
    let r = s.move_to(id, "done", "BOT-1");
    assert!(r.is_err(), "the same rule on the move path, case-insensitively: {r:?}");
    assert!(r.unwrap_err().to_string().contains(REFUSED));
    assert_eq!(s.card(id).unwrap().column, "review", "the card did not reach done");
    // --force stays the logged override for a genuine solo user
    let before = forces(&s, id);
    assert_eq!(s.done_forced(id, "bot-1").unwrap().column, "done");
    assert_eq!(forces(&s, id), before + 1, "the forced self-approval is logged");
}

/// `tb next --review` hands a force-moved card to the mover and withholds it from the worker:
/// the claim path reads the same author rule as the approval path.
#[test]
fn next_review_offers_a_force_moved_card_to_the_mover_not_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = force_moved_to_review(&mut s, "bot-1", "rev");
    let r = s.next_review("bot-1");
    assert!(r.is_err(), "the worker is not offered its own card: {r:?}");
    assert!(r.unwrap_err().to_string().contains("your own work"));
    let c = s.next_review("rev").unwrap();
    assert_eq!((c.id, c.reviewer.as_deref()), (id, Some("rev")));
}

/// The reproducer from the issue, end to end on the CLI.
#[test]
fn cli_force_move_does_not_swap_the_worker_and_the_reviewer() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let mut s = Store::open(&db).unwrap();
    let id = s.add("t: two", "Done = x", &[], "lead").unwrap();
    s.take(id, "bot-1").unwrap();
    let ids = id.to_string();
    // a non-holder's plain `tb done` on a DOING card is refused (the ownership guard)
    let o = tb(&db, "rev", &["done", &ids]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("is held by bot-1"), "{}", stderr(&o));
    // forced, it reaches review
    assert!(tb(&db, "rev", &["done", &ids, "--force"]).status.success());
    assert_eq!(s.card(id).unwrap().column, "review");
    // the worker that held it is refused ...
    let o = tb(&db, "bot-1", &["done", &ids]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains(REFUSED), "{}", stderr(&o));
    assert_eq!(s.card(id).unwrap().column, "review");
    // ... and the reviewer that moved it may close it
    let o = tb(&db, "rev", &["done", &ids]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(s.card(id).unwrap().column, "done");
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
    assert_eq!(prompt, "this is your work — approve it yourself? y/n");
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
