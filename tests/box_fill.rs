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
    s.set_layout("auto").unwrap();
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

/// The cards of column `ci` seen in `rows`, by their `#id` (the column's `n` cards hold
/// consecutive ids from `first_id(ci)`) — an id survives where a narrow title is cut away.
fn tokens(rows: &[String], ci: usize, n: usize) -> Vec<usize> {
    let first = first_id(ci);
    (0..n)
        .filter(|k| {
            let id = format!("#{}", first + k);
            rows.iter().any(|r| r.match_indices(&id).any(|(at, _)| !r[at + id.len()..].starts_with(|c: char| c.is_ascii_digit())))
        })
        .collect()
}

/// The id of column `ci`'s first card: the board adds them column by column.
fn first_id(ci: usize) -> usize {
    1 + COUNTS[..ci].iter().map(|(_, _, n)| n).sum::<usize>()
}

const SIZES: [(u16, u16); 6] = [(60, 30), (59, 30), (80, 24), (100, 40), (127, 75), (200, 60)];

/// Every layout the `L` key cycles through (focus draws one card, no boxes: skipped).
const LAYOUTS: [&str; 5] = ["auto", "third-h", "third-v", "half-h", "half-v"];

/// The rows inside the panel box titled `name` (` GITHUB`, ` AGENTS`), if one is drawn.
fn panel_rows(screen: &[Vec<char>], name: &str) -> Vec<String> {
    let lines: Vec<String> = screen.iter().map(|l| l.iter().collect()).collect();
    let mut out = Vec::new();
    for (y, l) in lines.iter().enumerate() {
        let Some(x) = l.find(&format!("┌ {name}")).or_else(|| l.find(&format!("┏ {name}"))) else { continue };
        let x = l[..x].chars().count();
        for row in &lines[y + 1..] {
            let cs: Vec<char> = row.chars().collect();
            if matches!(cs.get(x), Some('└' | '┗')) {
                break;
            }
            out.push(cs[x + 1..].iter().take_while(|c| !matches!(c, '│' | '┃')).collect());
        }
        break;
    }
    out
}

#[test]
fn every_box_shows_whole_cards_the_same_number_and_no_gap_before_more() {
    let (_d, s) = owners_board();
    for layout in LAYOUTS {
        let app = app_for(&s);
        s.set_layout(layout).unwrap();
        let mut app = app;
        app.reload(&s);
        for (w, h) in SIZES {
            check(&app, layout, w, h);
        }
    }
}

fn check(app: &App, layout: &str, w: u16, h: u16) {
    let screen = render(app, w, h);
    let all = text(&screen);
    let at = format!("{layout} {w}x{h}");
    let drawn: Vec<(usize, &str)> = app.drawn_styles.borrow().clone();
    if drawn.is_empty() {
        return; // the focus view: one card, no column boxes
    }
    // ONE card form per frame: no column drawn in a different form from its neighbours
    let forms: std::collections::BTreeSet<&str> = drawn.iter().map(|(_, f)| *f).collect();
    assert!(forms.len() <= 1, "{at}: mixed card forms in one frame {drawn:?}:\n{all}");
    let mut shown_where_hidden = Vec::new();
    let mut any_hidden = false;
    for (ci, (col, _, n)) in COUNTS.into_iter().enumerate() {
        if !drawn.iter().any(|(c, _)| *c == ci) {
            continue; // a section folded to its header line (it carries the count)
        }
        let rows = inner_rows(app, &screen, ci);
        let seen = tokens(&rows, ci, n);
        let hidden = n - seen.len();
        if rows.len() >= 6 {
            // several cards a box, not one
            assert!(seen.len() >= 3.min(n), "{at}: {col} shows {} card(s), wants at least 3:\n{all}", seen.len());
        }
        // a card is WHOLE: a card box's top border is followed by its content, never
        // straight by its bottom border (a title with its info line cut away)
        for (k, row) in rows.iter().enumerate() {
            if row.starts_with(['┌', '┏']) {
                let next = rows.get(k + 1).map(String::as_str).unwrap_or("");
                assert!(next.starts_with(['│', '┃']), "{at}: a {col} card is cut to its title (row {k} of the box):\n{all}");
            }
        }
        if hidden > 0 {
            any_hidden = true;
            // the box's last row says so, and no row of the box is left empty
            let last = rows.last().unwrap();
            assert!(last.contains('+'), "{at}: {col} hides {hidden} card(s) but its last row is {last:?}:\n{all}");
            for (k, row) in rows.iter().enumerate() {
                assert!(!row.trim().is_empty(), "{at}: {col} hides {hidden} card(s) yet row {k} of its box is empty:\n{all}");
            }
            shown_where_hidden.push((col, seen.len()));
        }
    }
    // equal boxes hold an equal number of cards
    let counts: Vec<usize> = shown_where_hidden.iter().map(|(_, n)| *n).collect();
    assert!(counts.windows(2).all(|p| p[0] == p[1]), "{at}: boxes with cards out of sight show different numbers {shown_where_hidden:?}:\n{all}");
    // a panel under the 2x2 grid is its content, never padding, while cards are hidden
    // (a grid across the whole width: its panels are below it, not beside it as in the rail)
    let r = app.col_rects.get();
    let grid = r[0].y != r[2].y && r[1].x + r[1].width == w;
    if any_hidden && grid {
        for name in ["GITHUB", "AGENTS"] {
            for (k, row) in panel_rows(&screen, name).iter().enumerate() {
                assert!(!row.trim().is_empty(), "{at}: the {name} panel pads row {k} with nothing while cards are hidden:\n{all}");
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
        seen.extend(tokens(&inner_rows(&app, &screen, 0), 0, 20).into_iter().map(|k| k + 1));
        app.handle_key(down, &mut s);
    }
    let missed: Vec<usize> = (1..=20).filter(|i| !seen.contains(i)).collect();
    assert!(missed.is_empty(), "never reached with the down arrow: {missed:?}");
}
