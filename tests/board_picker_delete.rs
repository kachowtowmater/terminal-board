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

/// One keypress does it — no y/n, no "archive it first" (the owner: "just let the user press
/// a and or d, don't ask anything follow up"). The picker stays open on the updated list,
/// and one status line says what happened.
#[test]
fn the_picker_archives_restores_and_deletes_on_one_keypress() {
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
    board("scratch").add("a throwaway", "", &[], "alice").unwrap();
    board("old").add("done long ago", "", &[], "alice").unwrap();
    boards::archive("old").unwrap();
    assert_eq!(archives(dir.path(), "old").len(), 1);

    let mut store = board("default");
    let mut app = App::new(store.snapshot().unwrap(), "alice");
    app.reload(&store);
    let in_picker = |app: &App, what: &str| {
        assert!(matches!(app.mode, Mode::Boards { .. }), "{what}: a question or another mode instead of done: {:?}", app.mode);
    };

    // --- the picker lists the archived board, under `archived`
    app.handle_key(key('B'), &mut store);
    let screen = render(&app);
    assert!(screen.contains("archived") && screen.contains("old"), "no archived row in the picker:\n{screen}");

    // --- d on an archived board: deleted at once
    select(&mut app, &mut store, "old");
    app.handle_key(key('d'), &mut store);
    in_picker(&app, "d on an archived board");
    assert!(archives(dir.path(), "old").is_empty(), "d did not delete it");
    let screen = render(&app);
    assert!(screen.contains("deleted 'old'"), "no status line saying so:\n{screen}");
    assert!(!screen.lines().any(|l| l.contains("  old ")), "the deleted board is still listed:\n{screen}");

    // --- d on a LIVE board: deleted at once too (archived and deleted in one go)
    select(&mut app, &mut store, "scratch");
    app.handle_key(key('d'), &mut store);
    in_picker(&app, "d on a live board");
    assert!(!boards::path_for("scratch").exists(), "d did not delete the live board");
    assert!(archives(dir.path(), "scratch").is_empty(), "d left an archive behind");
    assert!(render(&app).contains("deleted 'scratch'"), "no status line:\n{}", render(&app));

    // --- a archives at once
    select(&mut app, &mut store, "work");
    app.handle_key(key('a'), &mut store);
    in_picker(&app, "a");
    assert!(!boards::path_for("work").exists(), "a did not archive it");
    assert_eq!(archives(dir.path(), "work").len(), 1);
    assert!(render(&app).contains("archived 'work'"), "no status line:\n{}", render(&app));

    // --- r restores at once
    select(&mut app, &mut store, "work");
    app.handle_key(key('r'), &mut store);
    in_picker(&app, "r");
    assert!(boards::path_for("work").is_file(), "r did not restore it");
    assert!(archives(dir.path(), "work").is_empty());
    assert!(render(&app).contains("restored 'work'"), "no status line:\n{}", render(&app));
    let back = board("work");
    assert_eq!(back.snapshot().unwrap().cards.len(), 1, "the restored board lost its card");
    drop(back);

    // --- the board you are on (here also the default one) is refused, on the status line
    for k in ['a', 'd'] {
        select(&mut app, &mut store, "default");
        app.handle_key(key(k), &mut store);
        in_picker(&app, "a/d on the board you are on");
        assert!(boards::path_for("default").is_file(), "{k} removed the board it is on");
        assert!(render(&app).contains("switch to another board first"), "{k}: no reason given:\n{}", render(&app));
    }
}
