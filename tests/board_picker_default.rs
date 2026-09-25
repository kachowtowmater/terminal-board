//! `*` in the board picker (`B`) makes the selected board the default — the board a plain
//! `tb` opens — at once, with no question, saved exactly where `tb boards --default NAME`
//! saves it. The `*` mark in the list moves with it, an archived board is refused, `*` on the
//! built-in `default` goes back to it, and the picker's own "the board a bare tb opens"
//! refusal follows the new default.
//!
//! One test function: the picker reads the boards directory under `$HOME`, and `HOME` is
//! process-wide.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use terminal_board::boards;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App, Mode};

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn render(app: &App) -> String {
    let (w, h) = (126, 41);
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn board(name: &str) -> Store {
    Store::open(&boards::path_for(name)).unwrap().named(name)
}

/// Open the picker with the cursor on `name`'s row.
fn select(app: &mut App, store: &mut Store, name: &str) {
    app.mode = Mode::Normal;
    app.handle_key(key('B'), store);
    app.mode = Mode::Boards { sel: 0 };
    for _ in 0..10 {
        let Mode::Boards { sel } = app.mode else { panic!("the picker closed: {:?}", app.mode) };
        let row = [format!(">  {name}"), format!(">* {name}")];
        if render(app).lines().any(|l| row.iter().any(|r| l.contains(r.as_str()))) {
            return;
        }
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), store);
        if app.mode == (Mode::Boards { sel }) {
            break;
        }
    }
    panic!("no picker row for '{name}':\n{}", render(app));
}

fn status(app: &App) -> String {
    app.status.as_ref().map(|(s, _)| s.clone()).unwrap_or_default()
}

/// The row `name` has in the picker right now, starred or not (`*`), selected or not.
fn starred(app: &App, name: &str) -> bool {
    // anchored to the overlay's left border, so the footer hint's "* default" never counts
    render(app).lines().any(|l| l.contains(&format!("┃ >* {name} ")) || l.contains(&format!("┃  * {name} ")))
}

#[test]
fn star_makes_the_selected_board_the_default_at_once() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", dir.path());
    for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD", "TB_CONFIG"] {
        std::env::remove_var(k);
    }
    board("default").add("the built-in board", "", &[], "alice").unwrap();
    board("work").add("ops: rotate tokens", "", &[], "alice").unwrap();
    board("old").add("done long ago", "", &[], "alice").unwrap();
    boards::archive("old").unwrap();
    // the picker runs on `spare`, so no other board is "the board you are on"
    let mut store = board("spare");
    store.add("a card", "", &[], "alice").unwrap();
    let mut app = App::new(store.snapshot().unwrap(), "alice");
    app.reload(&store);
    assert_eq!(boards::default_name(), "default");

    // --- `*` on `work`: the default at once, saved where the CLI saves it, the mark moves
    select(&mut app, &mut store, "work");
    app.handle_key(key('*'), &mut store);
    assert!(matches!(app.mode, Mode::Boards { .. }), "* left the picker or asked something: {:?}", app.mode);
    assert!(status(&app).contains("'work' is now the default"), "no status line saying so: {:?}", app.status);
    assert_eq!(boards::saved_default().unwrap().as_deref(), Some("work"), "not saved as 'tb boards --default work' saves it");
    assert_eq!(boards::default_name(), "work", "a plain tb would not open it");
    assert!(starred(&app, "work") && !starred(&app, "default"), "the * mark did not move:\n{}", render(&app));

    // --- `*` again on the default: nothing changes, and it says so
    app.handle_key(key('*'), &mut store);
    assert!(status(&app).contains("already the default"), "{:?}", app.status);
    assert_eq!(boards::saved_default().unwrap().as_deref(), Some("work"));

    // --- an archived board cannot be the default
    select(&mut app, &mut store, "old");
    app.handle_key(key('*'), &mut store);
    assert!(status(&app).contains("'old' is archived"), "{:?}", app.status);
    assert_eq!(boards::saved_default().unwrap().as_deref(), Some("work"), "an archived board became the default");

    // --- the picker's own refusal follows the new default: `d` on `work` is refused now ...
    select(&mut app, &mut store, "work");
    app.handle_key(key('d'), &mut store);
    assert!(status(&app).contains("delete another board"), "d on the new default: {:?}", app.status);
    assert!(boards::path_for("work").is_file(), "d deleted the new default board");
    // ... and the old default is an ordinary board: `a` archives it
    select(&mut app, &mut store, "default");
    app.handle_key(key('a'), &mut store);
    assert!(!boards::path_for("default").exists(), "a on the old default: {:?}", app.status);
    select(&mut app, &mut store, "default");
    app.handle_key(key('r'), &mut store);
    assert!(boards::path_for("default").is_file(), "r did not bring it back: {:?}", app.status);

    // --- `*` on the built-in `default` goes back to it: the saved choice is cleared
    select(&mut app, &mut store, "default");
    app.handle_key(key('*'), &mut store);
    assert!(status(&app).contains("'default' is now the default"), "{:?}", app.status);
    assert_eq!(boards::saved_default().unwrap(), None, "the saved choice was not cleared");
    assert_eq!(boards::default_name(), "default");
    assert!(starred(&app, "default") && !starred(&app, "work"), "the * mark did not move back:\n{}", render(&app));
}
