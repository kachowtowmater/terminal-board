//! `TB_DB` pins ONE board file, so `tb` refuses board names in that mode — and so must the
//! board picker. ONE test in its own binary: `TB_DB` is process-wide.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App, Mode};

#[test]
fn b_refuses_cleanly_when_tb_db_pins_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("pinned.db");
    std::env::set_var("TB_DB", &db);
    std::env::set_var("HOME", dir.path());

    let mut store = Store::open(&db).unwrap().named("default");
    common::seed(&store, "alice").unwrap();
    let mut app = App::new(store.snapshot().unwrap(), "alice");
    app.reload(&store);

    assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('B'), KeyModifiers::NONE), &mut store));
    // no overlay, no half-built list — a message on the footer, and the board untouched
    assert_eq!(app.mode, Mode::Normal, "the picker opened with TB_DB set");
    assert!(app.boards.is_empty(), "the picker collected rows with TB_DB set");
    let (msg, is_err) = app.status.clone().expect("B said nothing");
    assert!(msg.contains("TB_DB"), "the message does not name TB_DB: {msg}");
    assert!(is_err, "the refusal is not reported as a refusal");

    let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
    t.draw(|f| draw(f, &app)).unwrap();
    let b = t.backend().buffer();
    let screen: String =
        b.content.chunks(120).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n");
    assert!(!screen.contains(" Boards "), "an overlay was drawn anyway:\n{screen}");
    assert!(screen.contains("TB_DB pins one board file"), "the refusal is not on screen:\n{screen}");
}
