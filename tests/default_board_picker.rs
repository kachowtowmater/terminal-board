//! The board picker (`B`) marks the saved default board, exactly as `tb boards` does.
//! ONE test in its own binary: the picker reads `$HOME`, which is process-wide.
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::process::Command;
use terminal_board::boards;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App, Mode};

#[test]
fn the_picker_marks_the_saved_default_board() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", dir.path());
    for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD", "TB_CONFIG", "TTYBOARD_CONFIG"] {
        std::env::remove_var(k);
    }
    for name in ["default", "home", "work"] {
        Store::open(&boards::path_for(name)).unwrap().named(name).add("x: a card", "", &[], "alice").unwrap();
    }
    let tb = |args: &[&str]| {
        let o = Command::new(env!("CARGO_BIN_EXE_tb")).args(args).env("HOME", dir.path()).env("TB_NO_HERDR", "1").output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    };
    let marked = |store: &mut Store| -> (Vec<String>, String) {
        let mut app = App::new(store.snapshot().unwrap(), "alice");
        app.reload(store);
        app.handle_key(KeyEvent::new(KeyCode::Char('B'), KeyModifiers::NONE), store);
        assert!(matches!(app.mode, Mode::Boards { .. }), "B did not open the picker: {:?}", app.status);
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        t.draw(|f| draw(f, &app)).unwrap();
        let b = t.backend().buffer();
        let screen = b.content.chunks(100).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n");
        (app.boards.iter().filter(|r| r.is_default).map(|r| r.name.clone()).collect(), screen)
    };
    let mut store = Store::open(&boards::path_for("home")).unwrap().named("home");

    // nothing saved: the built-in `default` carries the mark, as it always did
    let (rows, _) = marked(&mut store);
    assert_eq!(rows, ["default"]);

    tb(&["boards", "--default", "work"]);
    let (rows, screen) = marked(&mut store);
    assert_eq!(rows, ["work"], "the picker's default row follows the saved default board");
    let line = |name: &str| screen.lines().find(|l| l.contains(&format!(" {name} "))).unwrap_or_else(|| panic!("no {name} row:\n{screen}")).to_string();
    assert!(line("work").contains("* work"), "the saved default board carries the * on screen:\n{screen}");
    assert!(!line("default").contains('*') && !line("home").contains("* home"), "only one row is marked:\n{screen}");

    tb(&["boards", "--default", "--clear"]);
    let (rows, _) = marked(&mut store);
    assert_eq!(rows, ["default"]);
}
