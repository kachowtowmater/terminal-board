//! The board picker (`B`) archives, restores and deletes boards, each behind a y/n question
//! that names the board, through the same functions as `tb boards archive|restore|delete`.
//!
//! One test function: the picker reads the boards directory under `$HOME`, and `HOME` is
//! process-wide.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::path::{Path, PathBuf};
use terminal_board::boards;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App, Mode};

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn down() -> KeyEvent {
    KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)
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

/// The archive files of `name` (`archive/<name>@<stamp>.db`).
fn archives(home: &Path, name: &str) -> Vec<PathBuf> {
    let dir = home.join(".local/state/terminal-board/archive");
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    let f = p.file_name().unwrap().to_string_lossy().to_string();
                    f.starts_with(&format!("{name}@")) && f.ends_with(".db")
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Open the picker and put the cursor on the row whose line contains `name`, by pressing
/// down from the top — the way a person would.
fn select(app: &mut App, store: &mut Store, name: &str) {
    app.mode = Mode::Normal;
    app.handle_key(key('B'), store);
    app.mode = Mode::Boards { sel: 0 };
    for _ in 0..10 {
        let Mode::Boards { sel } = app.mode else { panic!("the picker closed: {:?}", app.mode) };
        let screen = render(app);
        let row = [format!(">  {name}"), format!(">* {name}")];
        if screen.lines().any(|l| row.iter().any(|r| l.contains(r.as_str()))) {
            return;
        }
        app.handle_key(down(), store);
        if app.mode == (Mode::Boards { sel }) {
            break;
        }
    }
    panic!("no picker row for '{name}':\n{}", render(app));
}

#[test]
fn the_picker_archives_restores_and_deletes_with_a_question_naming_the_board() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", dir.path());
    for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD"] {
        std::env::remove_var(k);
    }
    let def = board("default");
    common::seed(&def, "alice").unwrap();
    drop(def);
    board("work").add("ops: rotate tokens", "", &[], "alice").unwrap();
    board("old").add("done long ago", "", &[], "alice").unwrap();
    boards::archive("old").unwrap();
    assert_eq!(archives(dir.path(), "old").len(), 1);

    let mut store = board("default");
    let mut app = App::new(store.snapshot().unwrap(), "alice");
    app.reload(&store);

    // --- the picker lists the archived board, under `archived`
    app.handle_key(key('B'), &mut store);
    let screen = render(&app);
    assert!(screen.contains("archived") && screen.contains("old"), "no archived row in the picker:\n{screen}");

    // --- d on it asks, naming the board; n keeps it
    select(&mut app, &mut store, "old");
    app.handle_key(key('d'), &mut store);
    let screen = render(&app);
    assert!(matches!(app.mode, Mode::Confirm { .. }), "d did not ask: {:?}", app.mode);
    assert!(screen.contains("delete archived board 'old'"), "the question does not name the board:\n{screen}");
    app.handle_key(key('n'), &mut store);
    assert_eq!(archives(dir.path(), "old").len(), 1, "n deleted it");
    assert!(matches!(app.mode, Mode::Boards { .. }), "n did not go back to the picker: {:?}", app.mode);

    // --- d, y deletes it for good
    select(&mut app, &mut store, "old");
    app.handle_key(key('d'), &mut store);
    app.handle_key(key('y'), &mut store);
    assert!(archives(dir.path(), "old").is_empty(), "y did not delete it");
    let screen = render(&app);
    assert!(!screen.lines().any(|l| l.contains("  old ")), "the deleted board is still listed:\n{screen}");

    // --- d on a live board is refused: archive it first
    select(&mut app, &mut store, "work");
    app.handle_key(key('d'), &mut store);
    assert!(!matches!(app.mode, Mode::Confirm { .. }), "d on a live board asked to delete it");
    assert!(render(&app).contains("archive it first"), "no reason given:\n{}", render(&app));
    assert!(boards::path_for("work").is_file(), "a live board was deleted");

    // --- a archives the selected board, after a question naming it
    select(&mut app, &mut store, "work");
    app.handle_key(key('a'), &mut store);
    assert!(render(&app).contains("archive board 'work'"), "the question does not name the board:\n{}", render(&app));
    app.handle_key(key('y'), &mut store);
    assert!(!boards::path_for("work").exists(), "a did not archive it");
    assert_eq!(archives(dir.path(), "work").len(), 1);

    // --- r restores it
    select(&mut app, &mut store, "work");
    app.handle_key(key('r'), &mut store);
    assert!(render(&app).contains("restore archived board 'work'"), "the question does not name the board:\n{}", render(&app));
    app.handle_key(key('y'), &mut store);
    assert!(boards::path_for("work").is_file(), "r did not restore it");
    assert!(archives(dir.path(), "work").is_empty());
    let back = board("work");
    assert_eq!(back.snapshot().unwrap().cards.len(), 1, "the restored board lost its card");
    drop(back);

    // --- the board you are on is not archived from under you
    select(&mut app, &mut store, "default");
    app.handle_key(key('a'), &mut store);
    assert!(!matches!(app.mode, Mode::Confirm { .. }), "asked to archive the board it is on");
    assert!(boards::path_for("default").is_file());
}
