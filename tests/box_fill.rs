//! Inside every column box: whole cards, no empty rows while cards are hidden, `+N more` on
//! the box's last row, and the same number of cards in every box.
//!
//! Reported by the owner from a real board of 20 TODO / 4 DOING / 7 REVIEW / 77 DONE on a
//! small screen (~60x30): DOING's card was drawn without its info line while the others were
//! whole, three boxes had an empty row before `+N more` and DONE did not, and every box
//! showed exactly one card. "The boxes being even showing the same amount of cards", and
//! "anything that goes over becomes read more by pressing down arrow".
//!
//! Everything here is read off the rendered screen and `App::col_rects` (where each column
//! was drawn), so this file compiles against the version before it and fails there by
//! ASSERTION.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use terminal_board::herdr::AgentsState;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App};

/// Named here rather than imported, so this file compiles against the version before it.
const MAX_VISIBLE_CARDS: usize = 10;

const COUNTS: [(&str, char, usize); 4] = [("todo", 'T', 20), ("doing", 'D', 4), ("review", 'R', 7), ("done", 'X', 77)];

/// The owner's shape: 20 / 4 / 7 / 77, the first six TODO cards blocked, and every DOING
/// card with a note (so DOING's cards are the tallest, as on the real board). Each title
/// starts with a token (`T01`, `D03`, …) that names the card on screen.
fn owners_board() -> (tempfile::TempDir, Store) {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(5).unwrap();
    for (col, p, n) in COUNTS {
        for i in 1..=n {
            let id = s.add(&format!("{p}{i:02} a title long enough to be cut on a small screen"), "", &[], "alice").unwrap();
            if col == "todo" && i <= 6 {
                s.block(id, Some("waiting on the other team"), "alice").unwrap();
            }
            if col != "todo" {
                // fixture only: nothing reaches done except from review, so seed it forced
                if col == "done" { s.move_to_forced(id, col, "alice") } else { s.move_to(id, col, "alice") }.unwrap();
            }
            if col == "doing" {
                s.note(id, "still working on it, tests running", "alice").unwrap();
            }
        }
    }
    (dir, s)
}

fn app_for(s: &Store) -> App {
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(s);
    app.agents = AgentsState::Unavailable("herdr not available".into());
    app
}

fn render(app: &App, w: u16, h: u16) -> Vec<Vec<char>> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().flat_map(|c| c.symbol().chars().next()).collect()).collect()
}

fn text(screen: &[Vec<char>]) -> String {
    screen.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// The rows inside column `ci`'s box (its frame stripped), as drawn this frame.
fn inner_rows(app: &App, screen: &[Vec<char>], ci: usize) -> Vec<String> {
    let r = app.col_rects.get()[ci];
    if r.height < 3 || r.width < 3 {
        return Vec::new();
    }
    (r.y + 1..r.y + r.height - 1)
        .map(|y| screen[y as usize][(r.x + 1) as usize..(r.x + r.width - 1) as usize].iter().collect())
        .collect()
}

/// The card tokens (`T01`…) of column prefix `p` seen in `rows`.
fn tokens(rows: &[String], p: char, n: usize) -> Vec<usize> {
    (1..=n).filter(|i| rows.iter().any(|r| r.contains(&format!("{p}{i:02} ")) || r.contains(&format!("{p}{i:02}…")))).collect()
}

const SIZES: [(u16, u16); 6] = [(60, 30), (59, 30), (80, 24), (100, 40), (127, 75), (200, 60)];

#[test]
fn every_box_shows_whole_cards_the_same_number_and_no_gap_before_more() {
    let (_d, s) = owners_board();
    let app = app_for(&s);
    for (w, h) in SIZES {
        let screen = render(&app, w, h);
        let all = text(&screen);
        let mut shown_where_hidden = Vec::new();
        for (ci, (col, p, n)) in COUNTS.into_iter().enumerate() {
            let rows = inner_rows(&app, &screen, ci);
            assert!(!rows.is_empty(), "{w}x{h}: {col} has no box:\n{all}");
            let seen = tokens(&rows, p, n);
            let hidden = n - seen.len();
            // several cards a box, not one
            assert!(seen.len() >= 3.min(n), "{w}x{h}: {col} shows {} card(s), wants at least 3:\n{all}", seen.len());
            // a card is WHOLE: a card box's top border is followed by its content, never
            // straight by its bottom border (a title with its info line cut away)
            for (k, row) in rows.iter().enumerate() {
                if row.starts_with(['┌', '┏']) {
                    let next = rows.get(k + 1).map(String::as_str).unwrap_or("");
                    assert!(
                        next.starts_with(['│', '┃']),
                        "{w}x{h}: a {col} card is cut to its title (row {k} of the box):\n{}\n{all}",
                        rows.join("\n")
                    );
                }
            }
            if hidden > 0 {
                // the box's last row says so, and no row above it is left empty
                let last = rows.last().unwrap();
                assert!(last.contains('+'), "{w}x{h}: {col} hides {hidden} card(s) but its last row is {last:?}:\n{all}");
                // The one exception: a column never draws more than MAX_VISIBLE_CARDS (10) at
                // once (an earlier report, pinned in tests/column_caps.rs), so a box with room
                // for more than ten keeps the rows past the tenth card empty.
                let capped = seen.len() == MAX_VISIBLE_CARDS;
                for (k, row) in rows.iter().enumerate().filter(|_| !capped) {
                    assert!(!row.trim().is_empty(), "{w}x{h}: {col} hides {hidden} card(s) yet row {k} of its box is empty:\n{all}");
                }
                shown_where_hidden.push((col, seen.len()));
            }
        }
        // equal boxes hold an equal number of cards
        let counts: Vec<usize> = shown_where_hidden.iter().map(|(_, n)| *n).collect();
        assert!(
            counts.windows(2).all(|p| p[0] == p[1]),
            "{w}x{h}: boxes with cards out of sight show different numbers of cards {shown_where_hidden:?}:\n{all}"
        );
        // a column with fewer cards than that shows all of them
        if let Some(slots) = counts.first() {
            for (ci, (col, p, n)) in COUNTS.into_iter().enumerate() {
                if n <= *slots {
                    let seen = tokens(&inner_rows(&app, &screen, ci), p, n);
                    assert_eq!(seen.len(), n, "{w}x{h}: {col} has room for all {n} cards:\n{all}");
                }
            }
        }
    }
}

/// What does not fit is still reachable with the down arrow, card by card, at the owner's
/// small size.
#[test]
fn the_down_arrow_reaches_every_todo_card_at_60x30() {
    let (_d, mut s) = owners_board();
    let mut app = app_for(&s);
    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..25 {
        let screen = render(&app, 60, 30);
        seen.extend(tokens(&inner_rows(&app, &screen, 0), 'T', 20));
        app.handle_key(down, &mut s);
    }
    let missed: Vec<usize> = (1..=20).filter(|i| !seen.contains(i)).collect();
    assert!(missed.is_empty(), "never reached with the down arrow: {missed:?}");
}
