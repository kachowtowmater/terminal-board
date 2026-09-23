//! Card #104: the `a` add prompt can set a due date, via a second step after the title.
//! Same `DueDate::parse` and the same refusal words as `tb add --due` and the `e` form
//! (#107's fix protects any form-shaped UI from silently dropping a field; this prompt is a
//! single footer line, so there is no field to drop, but it must still never overrun its own
//! row), and nothing is written until the date is checked.
//!
//! Assertions read the store and the rendered screen rather than matching `Mode::AddDue`
//! directly, on purpose: on a tree without this card, `a` still creates the card straight
//! from the title (no due-date step exists at all), so every check here fails BY ASSERTION
//! on that tree, never by a compile error over a type that is not there yet.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App, Mode};

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn typed(app: &mut App, s: &mut Store, text: &str) {
    for c in text.chars() {
        app.handle_key(key(KeyCode::Char(c)), s);
    }
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn board() -> (tempfile::TempDir, Store, App) {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    let app = App::new(s.snapshot().unwrap(), "alice");
    (dir, s, app)
}

/// `a`, a title, enter moves to a due-date step (nothing written yet), a date, enter — the
/// card is created with it, same as `tb add --due` / the `e` form would store.
#[test]
fn a_due_date_is_set_via_the_second_step() {
    let (_d, mut s, mut app) = board();
    app.handle_key(key(KeyCode::Char('a')), &mut s);
    typed(&mut app, &mut s, "ops: renew the cert");
    assert_eq!(s.list().unwrap().len(), 0, "nothing written while the title is still being typed");
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(s.list().unwrap().len(), 0, "the title alone must not create the card — a due date has not been offered yet");
    let screen = render(&app, 100, 30);
    assert!(screen.contains("due for") && screen.contains("YYYY-MM-DD"), "a due-date step is on screen:\n{screen}");
    assert!(screen.contains("renew the cert"), "the typed title carries through to the due step:\n{screen}");
    typed(&mut app, &mut s, "2026-11-02");
    app.handle_key(key(KeyCode::Enter), &mut s);
    let c = s.card(1).unwrap();
    assert_eq!((c.tag.as_deref(), c.title.as_str(), c.due.as_deref()), (Some("ops"), "renew the cert", Some("2026-11-02")));
}

/// Enter on an empty due-date field creates the card with no date — the same "empty = none"
/// rule the `e` form's due field already follows, and no extra step for the common case of
/// a card with no deadline.
#[test]
fn an_empty_due_step_creates_the_card_with_no_date() {
    let (_d, mut s, mut app) = board();
    app.handle_key(key(KeyCode::Char('a')), &mut s);
    typed(&mut app, &mut s, "ops: renew the cert");
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(s.list().unwrap().len(), 0, "still nothing written at the due step, even with nothing typed into it");
    app.handle_key(key(KeyCode::Enter), &mut s);
    let c = s.card(1).unwrap();
    assert_eq!((c.title.as_str(), c.due), ("renew the cert", None));
}

/// Esc at the due-date step cancels the WHOLE add, title included — the same thing esc does
/// at every other step in this app (the title step, the edit form, note, send-back). It is
/// not a way to skip only the date; enter-with-empty is (the test above).
#[test]
fn esc_at_the_due_step_cancels_the_whole_add() {
    let (_d, mut s, mut app) = board();
    app.handle_key(key(KeyCode::Char('a')), &mut s);
    typed(&mut app, &mut s, "ops: renew the cert");
    app.handle_key(key(KeyCode::Enter), &mut s);
    typed(&mut app, &mut s, "2026-11-02");
    app.handle_key(key(KeyCode::Esc), &mut s);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(s.list().unwrap().len(), 0, "esc cancelled the card, not only the date");
}

/// The same parser and the same words as `tb add --due`: the prompt refuses and stays open
/// with the typed date still there to fix, and nothing at all is written — not the date, not
/// the title either.
#[test]
fn a_date_the_cli_would_refuse_is_refused_here_in_the_same_words() {
    for bad in ["2026-02-30", "10/09/2026", "tomorrow", "2026-1-5"] {
        let (_d, mut s, mut app) = board();
        app.handle_key(key(KeyCode::Char('a')), &mut s);
        typed(&mut app, &mut s, "ops: renew the cert");
        app.handle_key(key(KeyCode::Enter), &mut s);
        typed(&mut app, &mut s, bad);
        app.handle_key(key(KeyCode::Enter), &mut s);
        let screen = render(&app, 100, 30);
        assert!(screen.contains(&format!("{bad}_")), "{bad}: stays at the due step with the typed text kept:\n{screen}");
        let (msg, is_error) = app.status.clone().expect("a status line");
        assert!(is_error, "{bad}: shown as an error");
        let same_as_cli =
            terminal_board::store::due::DueDate::parse(bad, "tb add \"tag: title\" --due 2026-10-09").unwrap_err().to_string();
        assert_eq!(msg, same_as_cli, "{bad}: the add prompt and the CLI say different things");
        assert_eq!(s.list().unwrap().len(), 0, "{bad}: nothing was written, not even the title");
    }
}

/// The date the prompt writes is the date that was typed, byte for byte in the file — the
/// prompt is not a second way to get a date subtly wrong.
#[test]
fn the_prompt_stores_the_date_as_typed() {
    for date in ["2026-10-09", "2026-03-08", "2028-02-29", "2027-01-05"] {
        let (_d, mut s, mut app) = board();
        app.handle_key(key(KeyCode::Char('a')), &mut s);
        typed(&mut app, &mut s, "ops: renew the cert");
        app.handle_key(key(KeyCode::Enter), &mut s);
        typed(&mut app, &mut s, date);
        app.handle_key(key(KeyCode::Enter), &mut s);
        let c = s.card(1).unwrap();
        assert_eq!(c.due.as_deref(), Some(date));
        let raw: Option<String> =
            rusqlite::Connection::open(_d.path().join("b.db")).unwrap().query_row("SELECT due FROM cards WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(raw.as_deref(), Some(date), "the file holds the text that was typed");
    }
}

/// A plain add with no `tag:` and two plain enters (title, then empty date) behaves exactly
/// as `a` always did before this card: the new step changes nothing for someone who never
/// wants a due date.
#[test]
fn a_plain_add_with_no_date_is_unchanged() {
    let (_d, mut s, mut app) = board();
    app.handle_key(key(KeyCode::Char('a')), &mut s);
    typed(&mut app, &mut s, "just a title");
    app.handle_key(key(KeyCode::Enter), &mut s);
    app.handle_key(key(KeyCode::Enter), &mut s);
    let c = s.card(1).unwrap();
    assert_eq!((c.tag.as_deref(), c.title.as_str(), c.due.as_deref()), (None, "just a title", None));
}

/// Never draws outside its row, at any size, with a long title and a date being typed —
/// the render sweep the card's checklist asks for.
#[test]
fn the_prompt_never_draws_outside_its_row_at_any_size() {
    let (_d, mut s, mut app) = board();
    app.handle_key(key(KeyCode::Char('a')), &mut s);
    typed(&mut app, &mut s, "ops: a reasonably long title for the wrap check");
    app.handle_key(key(KeyCode::Enter), &mut s);
    typed(&mut app, &mut s, "2026-1");
    for h in [4u16, 8, 12, 20, 40] {
        for w in [4u16, 8, 12, 20, 40, 60, 79, 80, 81, 120, 200] {
            let screen = render(&app, w, h);
            assert_eq!(screen.lines().count(), h as usize, "{w}x{h}: the render is the size of the terminal");
            for line in screen.lines() {
                assert_eq!(line.chars().count(), w as usize, "{w}x{h}: a row is not {w} cells");
            }
        }
    }
}
