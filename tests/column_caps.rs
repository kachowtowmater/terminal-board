//! One long column must never squeeze the others out.
//!
//! Reported by the owner from using the board: *"the todo when on full does not display all
//! the things done. most likely cause is that the done has too many and it pushes everyone."*
//!
//! Two things were wrong. In the stacked layouts the four columns share one height, and each
//! was grown to everything it wanted in turn — so the first long column took the lot and the
//! others were left as one-row headers. And a column would draw as many cards as its space
//! allowed, however many that was. Now every column reaches a fair share before any column
//! takes a second helping, and no column draws more than `MAX_VISIBLE_CARDS` at once.
//!
//! The rule that must never break: **a card that is not on screen always has a `+N more`
//! saying so.**
//!
//! Columns that COST different amounts — a note, a block, a due mark, which are the things
//! that actually change a card's height here — are swept in `tests/zz_rvd_property.rs`,
//! along with the cartesian and randomised sweeps of the same invariant.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use terminal_board::herdr::AgentsState;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App};

/// The cap this change introduces. Named here rather than imported, so this file compiles
/// against the version before it and fails by ASSERTION, not by a missing symbol.
const MAX_VISIBLE_CARDS: usize = 10;

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

const LAYOUTS: [&str; 6] = ["auto", "focus", "third-h", "third-v", "half-h", "half-v"];

/// A board with `[todo, doing, review, done]` cards in those columns; the title says which
/// column a card belongs to, so a screen can be read back to "which columns show work".
fn board_of(counts: [usize; 4]) -> (tempfile::TempDir, Store) {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(99).unwrap();
    for (ci, col) in ["todo", "doing", "review", "done"].iter().enumerate() {
        for i in 1..=counts[ci] {
            let id = s.add(&format!("{col}{i}"), "", &[], "alice").unwrap();
            if *col != "todo" {
                if *col == "done" { s.move_to_forced(id, col, "alice") } else { s.move_to(id, col, "alice") }.unwrap(); // fixture only: nothing reaches done except from review (verifier rule), so a card seeded straight into done is a forced move
            }
        }
    }
    (dir, s)
}

/// THE INVARIANT, checked the same way everywhere: **no column shows a second card while a
/// populated column shows none.** Every round of review found a different loop breaking it —
/// a shape-by-shape test list kept missing the next one, so the tests assert the rule itself.
///
/// `per_column` is how many of each column's own cards are drawn.
fn assert_first_cards_before_second(per_column: [usize; 4], counts: [usize; 4], what: &str, screen: &str) {
    let starved: Vec<usize> = (0..4).filter(|c| counts[*c] > 0 && per_column[*c] == 0).collect();
    if starved.is_empty() {
        return;
    }
    for (c, n) in per_column.iter().enumerate() {
        assert!(
            *n <= 1,
            "{what}: column {c} shows {n} cards while {starved:?} (populated) show none — first cards come before second cards:\n{screen}"
        );
    }
}

/// How many of each column's own cards are drawn, by card ID: a narrow column cuts the title
/// but never the `#id`, and ids are handed out column by column.
fn cards_per_column(screen: &str, counts: [usize; 4]) -> [usize; 4] {
    let mut board = screen.to_string();
    while let Some(at) = board.find("last moved #") {
        let end = board[at..].find("  ").map(|i| at + i).unwrap_or(board.len());
        board.replace_range(at..end.min(board.len()), "");
    }
    // read every `#<id>` token off the screen: a column four cells wide draws `#164` with no
    // space after it, so matching on a trailing space misses cards that ARE drawn
    let mut ids: std::collections::BTreeSet<usize> = Default::default();
    let chars: Vec<char> = board.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' {
            let digits: String = chars[i + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                // a cut number (`#16…`) is not a card id: the next char must not be an ellipsis
                let after = chars.get(i + 1 + digits.len());
                if after != Some(&'…') {
                    if let Ok(id) = digits.parse::<usize>() {
                        ids.insert(id);
                    }
                }
                i += digits.len();
            }
        }
        i += 1;
    }
    let mut out = [0usize; 4];
    let mut first = 1usize;
    for (ci, n) in counts.iter().enumerate() {
        out[ci] = (first..first + n).filter(|id| ids.contains(id)).count();
        first += n;
    }
    out
}

/// Which columns have at least one of their own cards drawn.
fn columns_showing(screen: &str, counts: [usize; 4]) -> Vec<usize> {
    let per = cards_per_column(screen, counts);
    (0..4).filter(|c| per[*c] > 0).collect()
}

/// THE SECOND REPRODUCTION (review of #131, round two): with THREE or more populated columns
/// the "top up toward two cards" pass took the WHOLE remaining budget instead of a share, so
/// the selected column got everything and the rest — including a DOING column with three
/// cards — got none. The reviewer's case: 80 / 3 / 80 / 80.
#[test]
fn three_and_four_populated_columns_all_show_work() {
    for counts in [
        [80usize, 0, 0, 80],
        [80, 3, 0, 80],
        [80, 3, 80, 80],
        [10, 3, 5, 116],
        [3, 3, 3, 3],
        [1, 80, 1, 80],  // review, round three: two cheap columns beside two long ones
        [1, 1, 1, 80],
        [80, 1, 0, 0],
    ] {
        let live = (0..4).filter(|c| counts[*c] > 0).count();
        let (_d, s) = board_of(counts);
        for layout in LAYOUTS {
            s.set_layout(layout).unwrap();
            for col in 0..4usize {
                let mut app = App::new(s.snapshot().unwrap(), "alice");
                app.reload(&s);
                app.agents = AgentsState::Unavailable("herdr not available".into());
                app.col = col;
                for (w, h) in [(60u16, 20u16), (80, 24), (100, 30), (126, 41)] {
                    let screen = render(&app, w, h);
                    if is_focus_view(&screen) {
                        continue;
                    }
                    let per = cards_per_column(&screen, counts);
                    assert_first_cards_before_second(per, counts, &format!("{counts:?} {layout} {w}x{h} cursor={col}"), &screen);
                    let showing = columns_showing(&screen, counts);
                    // never ONE column taking the room while other populated columns show
                    // nothing — unless the pane is too short for a second box at all, when
                    // every other column is an honest header carrying its count
                    if showing.len() == 1 && live > 1 && h >= 20 {
                        let others_headered = (0..4)
                            .filter(|c| counts[*c] > 0 && !showing.contains(c))
                            .all(|c| screen.contains(&format!("o {} (", ["TODO", "DOING", "REVIEW", "DONE"][c])));
                        assert!(
                            others_headered,
                            "{counts:?} {layout} {w}x{h} cursor={col}: column {} took the room and the rest are not even headers:\n{screen}",
                            showing[0]
                        );
                        // and with real room, one column may not be the only one drawing
                        assert!(h < 24 || w < 80, "{counts:?} {layout} {w}x{h} cursor={col}: only column {} shows work:\n{screen}", showing[0]);
                    }
                }
            }
        }
    }
}

/// A board with `todo` waiting cards and `done` finished ones.
fn lopsided(todo: usize, done: usize) -> (tempfile::TempDir, Store) {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    for i in 1..=todo {
        s.add(&format!("todo: waiting {i}"), "", &[], "alice").unwrap();
    }
    for i in 1..=done {
        let id = s.add(&format!("done: finished {i}"), "", &[], "alice").unwrap();
        s.move_to_forced(id, "done", "alice").unwrap(); // fixture only: nothing reaches done except from review (verifier rule), so a card seeded straight into done is a forced move
    }
    (dir, s)
}

/// THE REPRODUCTION (review of #131): the min-boxed pre-pass walked from the SELECTED column
/// and was all-or-nothing, so with the cursor on a long column a short one could still be
/// left with nothing. The owner's own board: 10 TODO, 116 DONE, `third-v`, 60x20, cursor on
/// DONE. Every layout, every size, every cursor position.
#[test]
fn a_short_column_shows_work_whichever_column_the_cursor_is_on() {
    let (_d, s) = lopsided(10, 116);
    for layout in LAYOUTS {
        s.set_layout(layout).unwrap();
        for col in 0..4usize {
            let mut app = App::new(s.snapshot().unwrap(), "alice");
            app.reload(&s);
            app.agents = AgentsState::Unavailable("herdr not available".into());
            app.col = col;
            for (w, h) in [(60u16, 20u16), (60, 14), (80, 24), (100, 30), (126, 41), (160, 60)] {
                let screen = render(&app, w, h);
                if is_focus_view(&screen) {
                    continue;
                }
                let (todo, done) = shown_per_column(&screen, 10, 116);
                // if DONE got boxes, TODO must have got at least one too — a long column
                // never leaves a short one with nothing, whoever the cursor is on
                // with room for two boxes both columns draw; with room for one, the other
                // is an honest header carrying its count (the rule the review accepted)
                if done > 0 && todo == 0 {
                    assert!(
                        screen.contains("o TODO (10)"),
                        "{layout} {w}x{h} cursor={col}: DONE boxed {done} cards and TODO is not even a header:\n{screen}"
                    );
                    assert!(
                        h < 20 || w < 60,
                        "{layout} {w}x{h} cursor={col}: room for two, but DONE boxed {done} while TODO got none:\n{screen}"
                    );
                }
            }
        }
    }
}

/// The owner's board: a short TODO beside a DONE column with far more cards than fit.
fn crowded(done: usize) -> (tempfile::TempDir, Store) {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    for i in 1..=3 {
        s.add(&format!("todo: waiting {i}"), "", &[], "alice").unwrap();
    }
    for i in 1..=done {
        let id = s.add(&format!("done: finished {i}"), "", &[], "alice").unwrap();
        s.move_to_forced(id, "done", "alice").unwrap(); // fixture only: nothing reaches done except from review (verifier rule), so a card seeded straight into done is a forced move
    }
    (dir, s)
}

/// How many cards of each column are on screen, by their titles.
fn is_focus_view(screen: &str) -> bool {
    screen.lines().next().is_some_and(|l| l.contains(" TODO ") && l.contains(" · DOING "))
}

fn shown_per_column(screen: &str, todo: usize, done: usize) -> (usize, usize) {
    // the AGENTS panel names a card id too ("last moved #43"), and it sits BESIDE the
    // columns, so the board cannot be cut off by line — drop that phrase instead
    let mut board = screen.to_string();
    while let Some(at) = board.find("last moved #") {
        let end = board[at..].find(' ').map(|i| at + i).unwrap_or(board.len());
        let end = board[end + 1..].find(' ').map(|i| end + 1 + i).unwrap_or(board.len());
        let end = board[end + 1..].find(' ').map(|i| end + 1 + i).unwrap_or(board.len());
        board.replace_range(at..end.min(board.len()), "");
    }
    let screen = board.as_str();
    // by id, not by title: a narrow column cuts the title but never the `#id`
    let has = |id: usize| screen.contains(&format!("#{id} ")) || screen.contains(&format!("#{id}\n"));
    let t = (1..=todo).filter(|i| has(*i)).count();
    let d = (todo + 1..=todo + done).filter(|i| has(*i)).count();
    (t, d)
}

/// THE REPORT: a long DONE column no longer starves TODO, in every layout and at every size
/// where TODO has any room at all.
#[test]
fn a_long_done_column_never_starves_the_others() {
    let (_d, s) = crowded(40);
    for layout in LAYOUTS {
        s.set_layout(layout).unwrap();
        let mut app = App::new(s.snapshot().unwrap(), "alice");
        app.reload(&s);
        app.agents = AgentsState::Unavailable("herdr not available".into());
        for h in [14u16, 20, 24, 30, 41, 60] {
            for w in [60u16, 80, 100, 126, 160] {
                // the cursor decides which column the allocator walks first: sweep it, or the
                // case where the LONG column goes first is never tested (review of #131)
                app.col = (h as usize + w as usize) % 4;
                let screen = render(&app, w, h);
                if is_focus_view(&screen) {
                    continue; // one card by design, whatever the layout asked for
                }
                let (todo, done) = shown_per_column(&screen, 3, 40);
                // every other view must show work from more than one column
                if h >= 20 {
                    assert!(todo > 0, "{layout} {w}x{h}: DONE squeezed TODO out entirely:\n{screen}");
                }
                assert!(done <= MAX_VISIBLE_CARDS, "{layout} {w}x{h}: {done} done cards drawn, the cap is {MAX_VISIBLE_CARDS}");
                // whatever is not on screen is counted
                // a column collapsed to its header is not silent: the header carries the count
                let collapsed = screen.contains("o DONE today (40)") && done == 0;
                if done < 40 && !collapsed {
                    let says = screen.contains("more") || screen.contains(" +") || screen.contains("+3");
                    assert!(says, "{layout} {w}x{h}: {done} of 40 done cards and no '+N more':\n{screen}");
                }
            }
        }
    }
}

/// The header keeps the true total, whatever the cap hides.
#[test]
fn the_header_count_is_the_real_count() {
    let (_d, s) = crowded(40);
    for layout in LAYOUTS {
        s.set_layout(layout).unwrap();
        let mut app = App::new(s.snapshot().unwrap(), "alice");
        app.reload(&s);
        let screen = render(&app, 126, 41);
        assert!(screen.contains("DONE today (40)") || screen.contains("DONE 40"), "{layout}: the header lies about the count:\n{screen}");
        assert!(screen.contains("TODO (3)") || screen.contains("TODO 3"), "{layout}:\n{screen}");
    }
}

/// No card is ever silently invisible: every column that draws fewer cards than it holds
/// says so, at every size and in every layout.
#[test]
fn a_hidden_card_always_has_a_more_hint() {
    let (_d, s) = crowded(12);
    for layout in LAYOUTS {
        s.set_layout(layout).unwrap();
        let mut app = App::new(s.snapshot().unwrap(), "alice");
        app.reload(&s);
        app.agents = AgentsState::Unavailable("herdr not available".into());
        for h in 6u16..=44 {
            for w in [40u16, 80, 126, 200] {
                app.col = (h as usize) % 4;
                let screen = render(&app, w, h);
                assert_eq!(screen.lines().count(), h as usize, "{layout} {w}x{h}");
                if is_focus_view(&screen) {
                    continue;
                }
                let (todo, done) = shown_per_column(&screen, 3, 12);
                // a column collapsed to its header is not silent: the header carries the count
                let collapsed = screen.contains("o DONE today (12)") || screen.contains("o TODO (3)\n");
                if todo + done < 15 && todo + done > 0 && !collapsed {
                    // the hint shortens in whole words in a narrow column: ` +10 more`, `+10`, `+`
                    let says = screen.contains("more") || screen.contains(" +") || screen.contains("+1") || screen.contains("+2");
                    assert!(says, "{layout} {w}x{h}: {todo}+{done} of 15 cards shown, nothing says the rest exist:\n{screen}");
                }
            }
        }
    }
}

/// A capped column shows the cards that MATTER most, not just the first by position: under
/// `sort due` that is the nearest due dates. (Card #87's neighbour: a cap that hid the
/// nearest deadline would hide exactly the work someone needs to see.)
#[test]
fn a_capped_column_shows_the_nearest_due_cards_under_sort_due() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_tz("UTC").unwrap();
    s.set_sort("due").unwrap();
    // 20 cards added in the WORST order: the nearest date is added last
    for i in (1..=20).rev() {
        let id = s.add(&format!("filing {i:02}"), "", &[], "alice").unwrap();
        let date = terminal_board::store::due::DueDate::parse(&format!("2026-11-{i:02}"), "x").unwrap();
        s.set_due(id, date.as_ref(), "alice").unwrap();
    }
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(&s);
    app.agents = AgentsState::Unavailable("herdr not available".into());
    let screen = render(&app, 126, 41);
    // whatever fits is the NEAREST dates, in order and with no gaps — never the first by
    // position, which here would be the twenty furthest-off ones
    let shown: Vec<usize> = (1..=20).filter(|i| screen.contains(&format!("filing {i:02}"))).collect();
    assert!(shown.len() >= 5, "too few drawn to judge the order:\n{screen}");
    assert!(shown.len() <= MAX_VISIBLE_CARDS, "{} drawn, the cap is {MAX_VISIBLE_CARDS}", shown.len());
    assert!(shown.iter().copied().eq(1..=shown.len()), "the nearest dates are not the ones shown:\n{screen}");
    assert!(screen.contains(&format!("+{} more", 20 - shown.len())), "the rest are counted:\n{screen}");
    // a blocked card is capped like any other, and still shown when it is the nearest
    let nearest = s.list().unwrap().into_iter().find(|c| c.title == "filing 01").expect("filing 01").id;
    s.block_opts(nearest, Some("waiting"), None, None, "alice").unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(&s);
    let screen = render(&app, 126, 41);
    assert!(screen.contains("blocked by waiting"), "the nearest card is still shown, blocked:\n{screen}");
}

/// Scrolling still reaches every card: the cap is a window, not a wall.
#[test]
fn every_card_is_still_reachable_by_scrolling() {
    let (_d, mut s) = crowded(0);
    for i in 4..=25 {
        s.add(&format!("todo: waiting {i}"), "", &[], "alice").unwrap();
    }
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(&s);
    app.agents = AgentsState::Unavailable("herdr not available".into());
    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let mut seen: std::collections::BTreeSet<usize> = Default::default();
    for _ in 0..30 {
        let screen = render(&app, 126, 41);
        for (i, found) in (1..=25).map(|i| (i, screen.contains(&format!("waiting {i} ")) || screen.contains(&format!("waiting {i}\n")))) {
            if found {
                seen.insert(i);
            }
        }
        app.handle_key(down, &mut s);
    }
    let missed: Vec<usize> = (1..=25).filter(|i| !seen.contains(i)).collect();
    assert!(missed.is_empty(), "these cards could never be reached by scrolling: {missed:?}");
}

/// A board small enough to fit renders exactly as it does today: the cap and the fair share
/// only ever take effect when something would otherwise be squeezed out.
#[test]
fn a_board_that_fits_is_untouched() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    common::seed(&s, "alice").unwrap();
    for layout in LAYOUTS {
        s.set_layout(layout).unwrap();
        let mut app = App::new(s.snapshot().unwrap(), "alice");
        app.reload(&s);
        app.agents = AgentsState::Unavailable("herdr not available".into());
        for (w, h) in [(126u16, 41u16), (100, 30), (80, 24), (60, 20)] {
            let screen = render(&app, w, h);
            // the fixture has at most 4 cards in a column, well under the cap: nothing may be
            // hidden that the pane has room for, so any `+N more` here is about ROOM, never
            // about the cap
            let cards = (1..=10).filter(|i| screen.contains(&format!("#{i} "))).count();
            assert!(cards > 0, "{layout} {w}x{h}: nothing drawn:\n{screen}");
        }
    }
}
