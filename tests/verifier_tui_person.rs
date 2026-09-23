//! The full-screen board's force prompt on a person's own REVIEW card names EVERY rule `y`
//! gets past — the list the transition itself applies — never just the first. No identity is
//! recorded in this process, so the actor is a person (the verifier rule does not bind them).
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use terminal_board::herdr::AgentsState;
use terminal_board::store::Store;
use terminal_board::tui::{App, Confirm, Mode};

#[test]
fn the_force_prompt_names_every_rule_it_skips() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_done_by(Some("anna")).unwrap();
    let id = s.add("a: my own work", "", &[], "lead").unwrap();
    s.take(id, "pat").unwrap();
    s.done(id, "pat").unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "pat");
    app.agents = AgentsState::Unavailable("x".into());
    app.reload(&s);
    app.focus_card(id);
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE), &mut s);
    let Mode::Confirm { action, prompt } = app.mode.clone() else { panic!("no confirm: {:?}", app.mode) };
    assert_eq!(action, Confirm::ApproveOwn(id));
    assert_eq!(prompt, "this is your work — close it anyway, skipping: never approve your own work, done-by? y/n (logged)");
    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE), &mut s);
    assert_eq!(s.card(id).unwrap().column, "done");
    // exactly the rules it named were forced, one logged event each
    let forced: Vec<String> = s.show(id).unwrap().events.into_iter().filter(|e| e.kind == "force").map(|e| e.text).collect();
    assert_eq!(forced, ["approved own work".to_string(), format!("closed #{id}, not on the done-by list")]);
}
