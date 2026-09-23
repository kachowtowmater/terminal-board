//! The height allocation, swept as a PROPERTY rather than a list of shapes.
//!
//! Every column on the board is drawn by one of three allocators — `draw_sections` for the
//! stacked layouts, `draw_board` for the side-by-side ones, `draw_grid` for `half-v`'s 2x2
//! — and every round of review of #131 found the same bug again in whichever one had not
//! been looked at yet. Four instances, each found by a hand-written shape. So this file
//! stops writing shapes and sweeps the space instead:
//!
//! 1. a CARTESIAN of the four column counts x 6 layouts x 4 cursor positions x 4 sizes;
//! 2. a RANDOMIZED sweep with a FIXED seed, so a failure is always reproducible;
//! 3. a `half-v`-targeted sweep over 16 heights x 8 widths, since the grid's two rows make
//!    it the allocator with the least room to be right by accident.
//!
//! THE PROPERTY, in every one of them:
//!
//! > **No column shows a second card while a populated column shows none.** First cards
//! > come before second cards, whatever the cursor is on and whatever the layout.
//!
//! A card counts as SHOWN when its `#id` is on the screen. A box with only a meta line in
//! it (`  0m`) is not a card a person can name, and counting the box instead of the id is
//! how instance four survived a sweep that was already running.
//!
//! Two deliberate exclusions, both pre-existing on `main` and neither caused by the height
//! allocation: the FOCUS layout, which draws one card by design, and anything under 40
//! columns wide, where cards are cut too far to be identified at all.
mod common;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use terminal_board::herdr::AgentsState;
use terminal_board::store::due::DueDate;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App};

const LAYOUTS: [&str; 6] = ["auto", "focus", "third-h", "third-v", "half-h", "half-v"];
const COLUMNS: [&str; 4] = ["todo", "doing", "review", "done"];
/// Below this width the cards are cut too far to identify, on `main` as much as here.
const MIN_WIDTH: u16 = 40;

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// What makes one card cost more rows than another. **Not the title**: a title is drawn
/// into the box border and wraps only in the unboxed list, so title length alone does not
/// change a column's height in this codebase. The real cost comes from the lines a card
/// earns — a note, a block reason, a due mark — which is what these mixes are built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cost {
    Plain,
    Note,
    Blocked,
    Due,
    Everything,
}

const COSTS: [Cost; 5] = [Cost::Plain, Cost::Note, Cost::Blocked, Cost::Due, Cost::Everything];

/// A board with `counts[c]` cards in each column, each column's cards carrying `costs[c]`.
fn board(counts: [usize; 4], costs: [Cost; 4]) -> (tempfile::TempDir, Store) {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(99).unwrap();
    for (ci, col) in COLUMNS.iter().enumerate() {
        for i in 1..=counts[ci] {
            let id = s.add(&format!("{col}{i}"), "", &[], "alice").unwrap();
            if *col != "todo" {
                s.move_to(id, col, "alice").unwrap();
            }
            let cost = costs[ci];
            if matches!(cost, Cost::Note | Cost::Everything) {
                s.note(id, "picked this up and got the first half done", "alice").unwrap();
            }
            if matches!(cost, Cost::Blocked | Cost::Everything) {
                s.block(id, Some("waiting on the other team"), "alice").unwrap();
            }
            if matches!(cost, Cost::Due | Cost::Everything) {
                let date = DueDate::parse(&format!("2026-11-{:02}", (i % 27) + 1), "x").unwrap();
                s.set_due(id, date.as_ref(), "alice").unwrap();
            }
        }
    }
    (dir, s)
}

/// Which column each card ended up in, read from the board rather than assumed: a blocked
/// card can be shown somewhere other than where it was added, and a sweep that assumed
/// otherwise would be checking a board that does not exist.
fn truth(app: &App) -> (Vec<(i64, usize)>, [usize; 4]) {
    let mut map = Vec::new();
    let mut counts = [0usize; 4];
    for c in &app.snap.cards {
        if let Some(ci) = COLUMNS.iter().position(|n| *n == c.column) {
            map.push((c.id, ci));
            counts[ci] += 1;
        }
    }
    (map, counts)
}

/// Every `#id` a person can read off the screen. A number cut by the column edge (`#16…`)
/// is not an id, and the AGENTS panel names a card of its own (`last moved #43`) beside the
/// board rather than under it, so that phrase is removed before the scan.
fn ids_on_screen(screen: &str) -> std::collections::BTreeSet<i64> {
    let mut board = screen.to_string();
    while let Some(at) = board.find("last moved #") {
        let end = board[at..].find("  ").map(|i| at + i).unwrap_or(board.len());
        board.replace_range(at..end.min(board.len()), "");
    }
    let mut ids = std::collections::BTreeSet::new();
    let chars: Vec<char> = board.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' {
            let digits: String = chars[i + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                if chars.get(i + 1 + digits.len()) != Some(&'…') {
                    if let Ok(id) = digits.parse::<i64>() {
                        ids.insert(id);
                    }
                }
                i += digits.len();
            }
        }
        i += 1;
    }
    ids
}

fn shown_per_column(screen: &str, map: &[(i64, usize)]) -> [usize; 4] {
    let ids = ids_on_screen(screen);
    let mut out = [0usize; 4];
    for (id, ci) in map {
        if ids.contains(id) {
            out[*ci] += 1;
        }
    }
    out
}

fn is_focus_view(screen: &str) -> bool {
    screen.lines().next().is_some_and(|l| l.contains(" TODO ") && l.contains(" · DOING "))
}

/// THE PROPERTY. If any populated column is showing nothing, then nothing anywhere is
/// showing a second card.
fn check(screen: &str, map: &[(i64, usize)], counts: [usize; 4], what: &str) {
    let shown = shown_per_column(screen, map);
    let starved: Vec<usize> = (0..4).filter(|c| counts[*c] > 0 && shown[*c] == 0).collect();
    if starved.is_empty() {
        return;
    }
    for (c, n) in shown.iter().enumerate() {
        assert!(
            *n <= 1,
            "{what}: column {c} shows {n} cards while {starved:?} (populated) show none — first cards come before second cards:\n{screen}"
        );
    }
}

/// Drive one board through every layout, cursor and size, checking the property each time.
fn sweep(counts: [usize; 4], costs: [Cost; 4], layouts: &[&str], sizes: &[(u16, u16)]) {
    let (_d, s) = board(counts, costs);
    for layout in layouts {
        s.set_layout(layout).unwrap();
        for col in 0..4usize {
            let mut app = App::new(s.snapshot().unwrap(), "alice");
            app.reload(&s);
            app.agents = AgentsState::Unavailable("herdr not available".into());
            app.col = col;
            let (map, real) = truth(&app);
            for (w, h) in sizes {
                if *w < MIN_WIDTH {
                    continue;
                }
                let screen = render(&app, *w, *h);
                if is_focus_view(&screen) {
                    continue;
                }
                check(&screen, &map, real, &format!("{counts:?} {costs:?} {layout} {w}x{h} cursor={col}"));
            }
        }
    }
}

/// 1. THE CARTESIAN: every combination of four column counts, over every layout, every
///    cursor position and four sizes. 0 covers an empty column, 1 the cheapest possible
///    claim on the height, 2 a column that can want a second card, and 30 one long enough
///    to take the whole pane if nothing stops it.
#[test]
fn every_shape_of_board_keeps_first_cards_first() {
    let sizes = [(60u16, 20u16), (80, 24), (100, 30), (126, 41)];
    for a in [0usize, 1, 2, 30] {
        for b in [0usize, 1, 2, 30] {
            for c in [0usize, 1, 2, 30] {
                for d in [0usize, 1, 2, 30] {
                    sweep([a, b, c, d], [Cost::Plain; 4], &LAYOUTS, &sizes);
                }
            }
        }
    }
}

/// The same cartesian, thinned, with the columns costing DIFFERENT amounts — what the
/// allocation trades is rows, so a column of plain cards beside one whose cards each carry
/// a note, a block and a due mark is a different board from the same counts.
#[test]
fn a_cheap_column_and_an_expensive_one_both_show_work() {
    let sizes = [(60u16, 20u16), (100, 30), (160, 16)];
    for (i, cheap) in COSTS.iter().enumerate() {
        for dear in COSTS.iter().skip(i + 1) {
            for counts in [[1usize, 1, 1, 1], [30, 1, 1, 1], [1, 30, 1, 30], [1, 1, 30, 2], [2, 30, 0, 1]] {
                sweep(counts, [*cheap, *dear, *cheap, *dear], &LAYOUTS, &sizes);
                sweep(counts, [*dear, *cheap, *dear, *cheap], &LAYOUTS, &sizes);
            }
        }
    }
}

/// A fixed-seed xorshift. Randomised so the sweep is not limited to the shapes someone
/// thought of; seeded so a failure is reproducible forever from the iteration number.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn upto(&mut self, n: u64) -> usize {
        (self.next() % n) as usize
    }
}

/// 2. THE RANDOMISED SWEEP, seeded. Counts, costs, layout, cursor and pane size are all
///    drawn; the seed is a constant, so the run is the same on every machine and in CI.
#[test]
fn randomised_boards_keep_first_cards_first() {
    let mut rng = Rng(0x5EED_0109_1311_ABCD);
    for round in 0..300 {
        let counts = [0usize; 4].map(|_| [0, 1, 1, 2, 3, 9, 30, 64][rng.upto(8)]);
        let costs = [Cost::Plain; 4].map(|_| COSTS[rng.upto(5)]);
        let layout = LAYOUTS[rng.upto(6)];
        let col = rng.upto(4);
        let w = (MIN_WIDTH as usize + rng.upto(161)) as u16;
        let h = (10 + rng.upto(51)) as u16;
        let (_d, s) = board(counts, costs);
        s.set_layout(layout).unwrap();
        let mut app = App::new(s.snapshot().unwrap(), "alice");
        app.reload(&s);
        app.agents = AgentsState::Unavailable("herdr not available".into());
        app.col = col;
        let (map, real) = truth(&app);
        let screen = render(&app, w, h);
        if is_focus_view(&screen) {
            continue;
        }
        check(&screen, &map, real, &format!("round {round}: {counts:?} {costs:?} {layout} {w}x{h} cursor={col}"));
    }
}

/// 3. THE GRID, swept on its own: `half-v` splits the body into two rows of two columns, so
///    a row's height is set by the dearer of its two columns and everything the body gives
///    away — including a panel collapsing to its 1-line bar — comes out of that split. The
///    fourth instance lived at exactly 80/1/1/1, 160x15..18, with the cursor on REVIEW.
#[test]
fn the_half_v_grid_keeps_first_cards_first() {
    // 8 widths x 16 heights
    let sizes: Vec<(u16, u16)> =
        [40u16, 60, 80, 100, 126, 160, 180, 200].iter().flat_map(|w| (10u16..=40).step_by(2).map(move |h| (*w, h))).collect();
    for counts in [[80usize, 1, 1, 1], [1, 1, 1, 80], [1, 80, 1, 80], [80, 80, 1, 1], [3, 0, 0, 40], [1, 2, 3, 4]] {
        for costs in [[Cost::Plain; 4], [Cost::Everything, Cost::Plain, Cost::Plain, Cost::Plain], [Cost::Plain, Cost::Due, Cost::Note, Cost::Blocked]] {
            sweep(counts, costs, &["half-v"], &sizes);
        }
    }
}

// ---------------------------------------------------------------------------------------
// EVENNESS (card #113) — the report that came AFTER #131. Starvation ("everyone gets a
// first card") was fixed; this pins the next promise: once everyone has a first card,
// growth is round-robin, so a long column can no longer keep taking every card after that
// while a short one sits at two or three. Same harness, same screen-reading, same
// exclusions (FOCUS draws one card by design; under 40 columns cards are cut too far to
// read on `main` as much as here) — only the property differs.
//
// THE PROPERTY:
//
// > No column shows two or more cards than another column that still has cards hidden.
//
// A column with nothing left hidden is exempt on purpose — it has handed its unused share
// back, and there is nothing left for it to be behind on.

/// THE EVENNESS PROPERTY.
fn check_evenness(screen: &str, map: &[(i64, usize)], counts: [usize; 4], what: &str) {
    let shown = shown_per_column(screen, map);
    for a in 0..4 {
        if counts[a] == 0 {
            continue;
        }
        for b in 0..4 {
            if b == a || counts[b] == 0 {
                continue;
            }
            let hidden_b = counts[b] > shown[b];
            assert!(
                !(hidden_b && shown[a] >= shown[b] + 2),
                "{what}: column {a} shows {} cards while column {b} shows {} and still has {} hidden — growth is round-robin, not first-come-first-grown:\n{screen}",
                shown[a],
                shown[b],
                counts[b] - shown[b]
            );
        }
    }
}

/// Drive one board through every layout, cursor and size, checking EVENNESS each time —
/// the sibling of `sweep`, which checks starvation over the same boards.
fn sweep_evenness(counts: [usize; 4], costs: [Cost; 4], layouts: &[&str], sizes: &[(u16, u16)]) {
    let (_d, s) = board(counts, costs);
    for layout in layouts {
        s.set_layout(layout).unwrap();
        for col in 0..4usize {
            let mut app = App::new(s.snapshot().unwrap(), "alice");
            app.reload(&s);
            app.agents = AgentsState::Unavailable("herdr not available".into());
            app.col = col;
            let (map, real) = truth(&app);
            for (w, h) in sizes {
                if *w < MIN_WIDTH {
                    continue;
                }
                let screen = render(&app, *w, *h);
                if is_focus_view(&screen) {
                    continue;
                }
                check_evenness(&screen, &map, real, &format!("{counts:?} {costs:?} {layout} {w}x{h} cursor={col}"));
            }
        }
    }
}

/// 4. THE CARTESIAN, for evenness: every combination of four column counts, over every
///    layout, cursor position and the same four sizes as the starvation cartesian. Same
///    cost everywhere (Plain), because a uniform board already exercises the allocator —
///    cost-driven imbalance gets its own fixture below, scoped to where an allocator can
///    actually act on it.
#[test]
fn every_shape_of_board_grows_evenly() {
    let sizes = [(60u16, 20u16), (80, 24), (100, 30), (126, 41)];
    for a in [0usize, 1, 2, 30] {
        for b in [0usize, 1, 2, 30] {
            for c in [0usize, 1, 2, 30] {
                for d in [0usize, 1, 2, 30] {
                    sweep_evenness([a, b, c, d], [Cost::Plain; 4], &LAYOUTS, &sizes);
                }
            }
        }
    }
}

/// The owner's own report, pinned directly: "todo shows only 3, doing is like 2, review and
/// done is showing like 8 or 10" — four populated columns on a pane tall and narrow enough
/// to stack (`third-v`, `draw_sections`, where all four columns share one height), close to
/// a real terminal size. Large, evenly-costed counts so nothing here is about content, only
/// about the share-out.
#[test]
fn the_owners_report_is_now_even() {
    let sizes = [(70u16, 45u16), (90, 50), (60, 60), (62, 62)];
    for counts in [[40usize, 40, 40, 40], [3, 2, 40, 40], [40, 40, 3, 2], [10, 10, 40, 40]] {
        sweep_evenness(counts, [Cost::Plain; 4], &["third-v"], &sizes);
    }
}

/// EVENNESS, mixed cost — but only where a real allocation decision is made. `third-h`
/// (`draw_rail`), `half-h` (`draw_board`), and `auto` when it picks either, give every
/// column of a row (or the whole board) the exact SAME height by construction — the
/// `Layout::horizontal`/`vertical` split never looks at a column's content, so there is no
/// share to redistribute there at all. A gap between a column of short cards and one of tall
/// cards at that EQUAL height is the cards, not a growth decision, is unrelated to this fix,
/// and is already true on `main`. `third-v` (`draw_sections`) and `half-v` (`draw_grid`) are
/// the two allocators this PR changes, and the only place the round-robin promise is
/// actually made against cost, so this fixture — otherwise identical to the starvation
/// property's `a_cheap_column_and_an_expensive_one_both_show_work` — is swept only there.
#[test]
fn a_cheap_column_and_an_expensive_one_grow_evenly_too() {
    let sizes = [(60u16, 20u16), (100, 30), (160, 16)];
    for (i, cheap) in COSTS.iter().enumerate() {
        for dear in COSTS.iter().skip(i + 1) {
            for counts in [[1usize, 1, 1, 1], [30, 1, 1, 1], [1, 30, 1, 30], [1, 1, 30, 2], [2, 30, 0, 1]] {
                sweep_evenness(counts, [*cheap, *dear, *cheap, *dear], &["third-v", "half-v"], &sizes);
                sweep_evenness(counts, [*dear, *cheap, *dear, *cheap], &["third-v", "half-v"], &sizes);
            }
        }
    }
}

/// 5. THE GRID, for evenness, swept on its own (mirrors `the_half_v_grid_keeps_first_cards_first`):
///    a row holds two columns and shares one height, so evenness there depends on the row
///    growth in `draw_grid` charging for the FLOOR column's own next card, not the row's
///    combined want.
#[test]
fn the_half_v_grid_grows_evenly() {
    let sizes: Vec<(u16, u16)> =
        [40u16, 60, 80, 100, 126, 160, 180, 200].iter().flat_map(|w| (10u16..=40).step_by(2).map(move |h| (*w, h))).collect();
    for counts in [[80usize, 1, 1, 1], [1, 1, 1, 80], [1, 80, 1, 80], [80, 80, 1, 1], [3, 0, 0, 40], [1, 2, 3, 4]] {
        for costs in [[Cost::Plain; 4], [Cost::Everything, Cost::Plain, Cost::Plain, Cost::Plain], [Cost::Plain, Cost::Due, Cost::Note, Cost::Blocked]] {
            sweep_evenness(counts, costs, &["half-v"], &sizes);
        }
    }
}
