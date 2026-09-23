//! The full-screen board goes through the same one transition as the CLI (store/verifier.rs):
//! its `d` key meets the same review-first and verifier refusals, with the same identity the
//! `tb` binary records. One test in its own binary: this process's identity is set once.
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use terminal_board::herdr::AgentsState;
use terminal_board::store::Store;
use terminal_board::tui::{App, Mode};

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

#[test]
fn the_board_keys_meet_the_same_rules_as_the_cli() {
    // this process is an agent with no verifier role — exactly what `tb` would record
    for k in ["TB_ROLE", "TTYBOARD_ROLE", "HERDR_PANE_ID", "AI_AGENT", "OMPCODE"] {
        std::env::remove_var(k);
    }
    std::env::set_var("TB_HARNESS", "test-agent");
    terminal_board::store::actors::use_environment();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let todo = s.add("a: straight to done", "", &[], "lead").unwrap();
    let rev = s.add("b: someone else's work", "", &[], "lead").unwrap();
    s.take(rev, "bot-1").unwrap();
    s.done(rev, "bot-1").unwrap();
    assert_eq!(s.card(rev).unwrap().column, "review");
    let mut app = App::new(s.snapshot().unwrap(), "orch");
    app.agents = AgentsState::Unavailable("x".into());
    app.reload(&s);
    // `d` on a TODO card: nothing reaches done except from review
    app.focus_card(todo);
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    assert_eq!(s.card(todo).unwrap().column, "todo");
    let (msg, is_err) = app.status.clone().expect("the refusal is shown");
    assert!(is_err && msg.contains("nothing reaches done except from review"), "{msg}");
    // `d` on someone else's REVIEW card, as an agent with no verifier role
    app.focus_card(rev);
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    assert_eq!(s.card(rev).unwrap().column, "review");
    let (msg, is_err) = app.status.clone().expect("the refusal is shown");
    assert!(is_err && msg.contains("only a verifier moves") && msg.contains("TB_ROLE=verifier"), "{msg}");
    // `d` on its OWN REVIEW card: no "close it anyway?" prompt — a y there would force past
    // the verifier rule too — just the refusal, and nothing moves or is forced
    let own = s.add("c: my own work", "", &[], "lead").unwrap();
    s.take(own, "orch").unwrap();
    s.done(own, "orch").unwrap();
    app.reload(&s);
    app.focus_card(own);
    app.status = None;
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    assert!(!matches!(app.mode, Mode::Confirm { .. }), "no force prompt for an agent without a verifier role: {:?}", app.mode);
    let (msg, is_err) = app.status.clone().expect("the refusal is shown");
    assert!(is_err && msg.contains("only a verifier closes"), "{msg}");
    assert_eq!(s.card(own).unwrap().column, "review");
    assert!(!s.show(own).unwrap().events.iter().any(|e| e.kind == "force"), "nothing was forced");
}
