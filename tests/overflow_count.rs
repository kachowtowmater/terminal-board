//! Card #108: the owner reported a TODO column at near-full-screen size saying `+12 more`
//! for a 12-card column — more hidden cards than the column could possibly be hiding, and
//! growing the terminal did not change the count. Verified NOT reproducible on main after
//! PRs #131/#147 (round-robin growth off a real, non-stale column height): these tests pin
//! that the `+N more` hint always counts EXACTLY the cards whose title is not drawn, at
//! three sizes spanning half, third and minimised shapes, including a case where a genuinely
//! small column (third-v, short) still must show a correct (non-zero, non-total) count.
mod common;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App};

/// 12 short, uniquely titled TODO cards and nothing else — so every title that appears
/// anywhere in the render can only have come from that card actually being drawn.
fn setup_12_todo() -> (tempfile::TempDir, Store, App) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(5).unwrap();
    for i in 1..=12 {
        s.add(&format!("card {i}: a moderately descriptive title here"), "", &[], "alice").unwrap();
    }
    let app = App::new(s.snapshot().unwrap(), "alice");
    (dir, s, app)
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// How many of the 12 TODO cards actually have their title drawn somewhere on screen, and
/// the hidden count the `+N more` hint (if any) reports. Scans for `+<digits>` anywhere in
/// the render (the hint sits to the right of the column's own left border glyph, so it is
/// never at the start of a line) so it still parses the shortened forms `more_hint` falls
/// back to at narrow widths (`+N more`, `+N`, `+`).
fn shown_and_hint(screen: &str) -> (usize, Option<usize>) {
    let shown = (1..=12).filter(|n| screen.contains(&format!("card {n}:"))).count();
    let chars: Vec<char> = screen.chars().collect();
    let hint = (0..chars.len()).find_map(|i| {
        if chars[i] != '+' || !chars.get(i + 1)?.is_ascii_digit() {
            return None;
        }
        let digits: String = chars[i + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<usize>().ok()
    });
    (shown, hint)
}

/// The overflow count must equal exactly what is not drawn, at three sizes spanning
/// half-h (wide, near full screen), third-h (minimised height) and third-v (minimised
/// width) — the layouts the card calls out by name. `+N more` must never claim more cards
/// hidden than the 12-card column has, and must never say a card is hidden while its title
/// is actually on screen (or vice versa).
#[test]
fn todo_overflow_count_is_exact_at_three_sizes() {
    common::pin_clock();
    for (w, h, label) in [(126u16, 41u16, "half-h 126x41"), (126, 24, "third-h 126x24"), (55, 70, "third-v 55x70")] {
        let (_d, _s, app) = setup_12_todo();
        let screen = render(&app, w, h);
        let (shown, hint) = shown_and_hint(&screen);
        assert!(shown <= 12, "{label}: {shown} distinct titles found, more than the 12 cards that exist");
        match hint {
            Some(hidden) => {
                assert_eq!(hidden, 12 - shown, "{label}: hint says +{hidden} more but {shown} titles are drawn\n{screen}");
                assert!(hidden < 12, "{label}: phantom overflow — hint claims all 12 hidden ({shown} were actually drawn)\n{screen}");
            }
            None => assert_eq!(shown, 12, "{label}: no overflow hint shown but only {shown}/12 titles are drawn\n{screen}"),
        }
    }
}

/// Enlarging the terminal must let the column show more, never hold the count still or
/// make it worse — the exact defect reported ("we are on full screen almost, still the
/// same"). third-v goes from a short pane to a tall one; the hidden count must shrink (never
/// grow or stay put) as height grows, down to the MAX_VISIBLE_CARDS design floor (12 - 10 =
/// 2: a column never draws more than 10 cards, however tall the pane — that cap is
/// intentional, not the bug, so a fully-grown pane still legitimately says "+2 more").
#[test]
fn growing_the_pane_shrinks_the_overflow_count() {
    common::pin_clock();
    let mut last_hidden = 12;
    for h in [30u16, 45, 70, 100] {
        let (_d, _s, app) = setup_12_todo();
        let screen = render(&app, 55, h);
        let (shown, hint) = shown_and_hint(&screen);
        let hidden = hint.unwrap_or(0);
        assert_eq!(hidden, 12 - shown, "55x{h}: hint/drawn mismatch (+{hidden} more, {shown} drawn)\n{screen}");
        assert!(hidden <= last_hidden, "55x{h}: overflow count grew from {last_hidden} to {hidden} as the pane got taller\n{screen}");
        last_hidden = hidden;
    }
    assert_eq!(last_hidden, 2, "a fully-grown 55x100 pane should sit at the MAX_VISIBLE_CARDS floor (10 shown, 2 more), not above or below it");
}
