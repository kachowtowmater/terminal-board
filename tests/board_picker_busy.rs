//! The board picker (`B`) on a board another `tb` has open: `a` and `d` refuse at once, by
//! name, instead of waiting out the move lock (up to 10 s, the picker frozen and every key
//! typed meanwhile landing on the re-read list). And `d` on the board a bare `tb` opens says
//! "delete", not "archive".
//!
//! One test function: the picker reads the boards directory under `$HOME`, and `HOME` is
//! process-wide.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::time::{Duration, Instant};
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

#[test]
fn a_held_board_is_refused_at_once_by_name_and_d_on_the_default_board_says_delete() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", dir.path());
    for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD", "TB_LOCK_WAIT_MS"] {
        std::env::remove_var(k);
    }
    board("default").add("the default board", "", &[], "alice").unwrap();
    board("work").add("ops: rotate tokens", "", &[], "alice").unwrap();
    // the picker runs on `spare`, so neither `default` nor `work` is "the board you are on"
    let mut store = board("spare");
    store.add("a card", "", &[], "alice").unwrap();
    let mut app = App::new(store.snapshot().unwrap(), "alice");
    app.reload(&store);

    // --- another tb has `work` open (a Store holds its lifetime lock for as long as it lives;
    // `flock` is per open file, so this one counts as another process's)
    let held = board("work");
    for k in ['a', 'd'] {
        select(&mut app, &mut store, "work");
        let t = Instant::now();
        app.handle_key(key(k), &mut store);
        let took = t.elapsed();
        let msg = status(&app);
        // the move lock waits 10 s by default: a refusal well under that did not wait on it
        assert!(took < Duration::from_secs(2), "{k} on a held board took {took:?} to refuse: {msg}");
        assert!(matches!(app.mode, Mode::Boards { .. }), "{k}: left the picker: {:?}", app.mode);
        assert!(boards::path_for("work").is_file(), "{k} removed a board another tb has open");
        assert!(msg.contains("'work'") && msg.contains("open in another tb"), "{k}: the refusal does not name the board: {msg}");
        assert!(!msg.contains('/') && !msg.contains(".db"), "{k}: the refusal carries a file path: {msg}");
        assert!(msg.chars().count() <= 90, "{k}: the refusal does not fit a 100-column status line: {msg}");
    }
    drop(held);

    // --- `d` on the board a bare `tb` opens says delete; `a` still says archive
    select(&mut app, &mut store, "default");
    app.handle_key(key('d'), &mut store);
    let msg = status(&app);
    assert!(msg.contains("delete another board") && !msg.contains("archive another board"), "d on the default board: {msg}");
    assert!(boards::path_for("default").is_file());
    select(&mut app, &mut store, "default");
    app.handle_key(key('a'), &mut store);
    assert!(status(&app).contains("archive another board"), "a on the default board: {}", status(&app));

    // --- no longer held: `a` works again, at once
    select(&mut app, &mut store, "work");
    app.handle_key(key('a'), &mut store);
    assert!(!boards::path_for("work").exists(), "a did not archive it once nobody held it: {}", status(&app));
    assert!(status(&app).contains("archived 'work'"), "{}", status(&app));
}
