//! Panes are split EVENLY, not by what they hold.
//!
//! Reported by the owner from a real board of 20 TODO / 4 DOING / 4 REVIEW / 60 DONE: the
//! 2x2 grid sized its rows by demand, so DONE's long pile won the height — TODO's twenty
//! cards were cut to a handful while REVIEW sat half empty. What he asked for: "everything
//! to look like default but spaced out evenly... anything that goes over becomes read more
//! by pressing down arrow key".
//!
//! THE INVARIANT pinned here, read off the rendered screen rather than any internal value
//! (so this file compiles against the version before it and fails there by ASSERTION):
//!
//! > The two rows of the grid differ in height by at most one row, the stacked sections
//! > differ by at most one row, and the columns of a row differ in width by at most one
//! > cell — whatever the columns hold.
//!
//! The overflow promise that comes with it — a column with more cards than fit says
//! `+N more`, and the down arrow scrolls into them — is pinned at the owner's shape too.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use terminal_board::herdr::AgentsState;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App};

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// The owner's board: 20 TODO, 4 DOING, 4 REVIEW, 60 DONE. A few TODO cards are blocked,
/// as on the real board, so the columns do not all cost the same per card.
fn owners_board() -> (tempfile::TempDir, Store) {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(5).unwrap();
    for (col, n) in [("todo", 20), ("doing", 4), ("review", 4), ("done", 60)] {
        for i in 1..=n {
            let id = s.add(&format!("{col} card {i}"), "", &[], "alice").unwrap();
            if col == "todo" && i <= 6 {
                s.block(id, Some("waiting on the other team"), "alice").unwrap();
            }
            if col != "todo" {
                s.move_to(id, col, "alice").unwrap();
            }
        }
    }
    (dir, s)
}

fn app_for(s: &Store, layout: &str) -> App {
    s.set_layout(layout).unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(s);
    app.agents = AgentsState::Unavailable("herdr not available".into());
    app
}

/// Every full-width pane frame on the screen, top to bottom, as `(first line, last line)`:
/// a pane's frame starts at column 0 (card boxes sit inside it, from column 1), so a corner
/// in column 0 is always a pane edge — a board column, a stacked section or a panel.
fn pane_frames(screen: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut open = None;
    for (y, line) in screen.lines().enumerate() {
        match line.chars().next() {
            Some('┌' | '┏') if open.is_none() => open = Some(y),
            Some('└' | '┗') => {
                if let Some(top) = open.take() {
                    out.push((top, y));
                }
            }
            _ => {}
        }
    }
    out
}

/// The widths of the panes whose top edge is on line `y`, read from their top-left corners.
fn pane_widths(screen: &str, y: usize) -> Vec<usize> {
    let line: Vec<char> = screen.lines().nth(y).unwrap().chars().collect();
    let starts: Vec<usize> = line.iter().enumerate().filter(|(_, c)| matches!(c, '┌' | '┏')).map(|(i, _)| i).collect();
    let mut w: Vec<usize> = starts.windows(2).map(|p| p[1] - p[0]).collect();
    w.push(line.len() - starts.last().unwrap());
    w
}

fn spread(v: &[usize]) -> usize {
    v.iter().max().unwrap() - v.iter().min().unwrap()
}

/// The owner's exact case: the 2x2 grid at 127x75 (and wherever else `half-v` is used)
/// gives TODO/DOING and REVIEW/DONE the same height, and the two columns of each row the
/// same width. On the demand-sized allocator this failed at 127x75 with a 30-row top row
/// and a 32-row bottom row, DONE's pile taking the difference.
#[test]
fn the_owners_board_splits_the_grid_rows_evenly() {
    let (_d, s) = owners_board();
    let mut sizes = vec![("auto", 127u16, 75u16)];
    for w in [70u16, 100, 127, 160] {
        for h in (30u16..=80).step_by(5) {
            sizes.push(("half-v", w, h));
        }
    }
    for (layout, w, h) in sizes {
        let app = app_for(&s, layout);
        let screen = render(&app, w, h);
        let frames = pane_frames(&screen);
        assert!(frames.len() >= 2, "{layout} {w}x{h}: expected the grid's two rows:\n{screen}");
        let rows: Vec<usize> = frames[..2].iter().map(|(a, b)| b - a + 1).collect();
        assert!(
            spread(&rows) <= 1,
            "{layout} {w}x{h}: grid rows are {} and {} lines tall — they must differ by at most one, whatever the columns hold:\n{screen}",
            rows[0],
            rows[1]
        );
        for (top, _) in &frames[..2] {
            let widths = pane_widths(&screen, *top);
            assert_eq!(widths.len(), 2, "{layout} {w}x{h}: a grid row has two columns:\n{screen}");
            assert!(spread(&widths) <= 1, "{layout} {w}x{h}: column widths {widths:?} must differ by at most one:\n{screen}");
        }
    }
}

/// The same promise for the stacked sections (`third-v`) and for the four columns side by
/// side (`half-h`, `third-h`): equal extents, whatever the columns hold.
#[test]
fn the_owners_board_splits_stacked_sections_and_side_by_side_columns_evenly() {
    let (_d, s) = owners_board();
    for (w, h) in [(50u16, 70u16), (60, 73), (42, 60), (62, 45)] {
        let app = app_for(&s, "third-v");
        let screen = render(&app, w, h);
        let frames = pane_frames(&screen);
        assert!(frames.len() >= 4, "third-v {w}x{h}: expected four boxed sections:\n{screen}");
        let sections: Vec<usize> = frames[..4].iter().map(|(a, b)| b - a + 1).collect();
        assert!(
            spread(&sections) <= 1,
            "third-v {w}x{h}: sections are {sections:?} lines tall — they must differ by at most one:\n{screen}"
        );
    }
    for (layout, w, h) in [("half-h", 100u16, 40u16), ("half-h", 200, 60), ("half-h", 126, 41), ("third-h", 126, 24), ("third-h", 97, 22)] {
        let app = app_for(&s, layout);
        let screen = render(&app, w, h);
        let frames = pane_frames(&screen);
        let widths = pane_widths(&screen, frames[0].0);
        let cols = &widths[..4];
        assert!(spread(cols) <= 1, "{layout} {w}x{h}: column widths {cols:?} must differ by at most one:\n{screen}");
    }
}

/// What does not fit is still reachable: at the owner's shape, TODO says `+N more`, and the
/// down arrow walks the cursor into the hidden cards until every one has been on screen —
/// at the owner's size and on taller panes, where a column has room for more than the ten
/// cards it draws at once (there the window used to stop one card short of the last).
#[test]
fn overflow_says_more_and_the_down_arrow_reaches_every_hidden_card() {
    let (_d, mut s) = owners_board();
    for (layout, w, h) in [("auto", 127u16, 75u16), ("half-v", 127, 100), ("half-h", 126, 60), ("third-v", 60, 150)] {
        let mut app = app_for(&s, layout);
        let first = render(&app, w, h);
        assert!(first.contains(" more"), "{layout} {w}x{h}: TODO has 20 cards; the ones not drawn must say `+N more`:\n{first}");
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let mut seen = std::collections::BTreeSet::new();
        let mut last = String::new();
        for _ in 0..25 {
            let screen = render(&app, w, h);
            for i in 1..=20 {
                if screen.contains(&format!("todo card {i} ")) || screen.contains(&format!("todo card {i}┐")) || screen.contains(&format!("todo card {i}┓")) {
                    seen.insert(i);
                }
            }
            app.handle_key(down, &mut s);
            last = screen;
        }
        let missed: Vec<usize> = (1..=20).filter(|i| !seen.contains(i)).collect();
        assert!(missed.is_empty(), "{layout} {w}x{h}: these TODO cards were never reached with the down arrow: {missed:?}\n{last}");
    }
}
