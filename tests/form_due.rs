//! The due date in the full-screen edit form.
//!
//! The form is the only place a person sets a date without typing a command, so it has to
//! behave exactly like `tb edit --due`: the same parser, the same refusal text, the same
//! stale-form rule the title and description already follow, and nothing written when the
//! date is refused.
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

/// A board with one card, and the form open on it.
fn open_form(due: Option<&str>) -> (tempfile::TempDir, Store, App) {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_tz("UTC").unwrap();
    let id = s.add("permits: renewal", "the fire permit", &[], "alice").unwrap();
    if let Some(d) = due {
        let date = terminal_board::store::due::DueDate::parse(d, "x").unwrap();
        s.set_due(id, date.as_ref(), "alice").unwrap();
    }
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(&s);
    app.handle_key(key(KeyCode::Char('e')), &mut s);
    assert!(matches!(app.mode, Mode::Edit(_)), "the form is open");
    (dir, s, app)
}

/// The field is prefilled, the form draws it, and tab reaches it.
#[test]
fn the_form_shows_the_date_the_card_has() {
    let (_d, _s, app) = open_form(Some("2026-10-09"));
    let screen = render(&app, 140, 40);
    assert!(screen.contains("Due  (YYYY-MM-DD, empty for none)"), "the field is labelled and says what it takes:\n{screen}");
    assert!(screen.contains("2026-10-09"), "prefilled with the card's date:\n{screen}");
    assert!(screen.contains("Title") && screen.contains("Description"), "the other two fields are still there");
    // the field is a real input, in tab order between the title and the description
    let due_row = screen.lines().position(|l| l.contains("Due  (YYYY-MM-DD")).expect("the Due label");
    let title_row = screen.lines().position(|l| l.contains("Title")).expect("the Title label");
    let desc_row = screen.lines().position(|l| l.contains("Description")).expect("the Description label");
    assert!(title_row < due_row && due_row < desc_row, "the date sits between the title and the description:\n{screen}");
    // a card with no date opens with an empty field, and nothing is invented into it
    let (_d, _s, app) = open_form(None);
    let screen = render(&app, 140, 40);
    assert!(screen.contains("Due  (YYYY-MM-DD, empty for none)"), "{screen}");
    assert!(!screen.contains("2026-"), "an undated card shows an empty date field:\n{screen}");
}

#[test]
fn a_date_is_set_changed_and_cleared_from_the_form() {
    // set one on a card that has none
    let (_d, mut s, mut app) = open_form(None);
    app.handle_key(key(KeyCode::Tab), &mut s);
    typed(&mut app, &mut s, "2026-10-09");
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(app.mode, Mode::Normal, "saved and closed");
    assert_eq!(s.card(1).unwrap().due.as_deref(), Some("2026-10-09"));
    assert_eq!(app.status.clone().map(|s| s.0).as_deref(), Some("#1 saved"));
    // the change is in the card's history, exactly as the CLI records it
    let last = s.show(1).unwrap().events.pop().unwrap();
    assert_eq!((last.kind.as_str(), last.text.as_str(), last.actor.as_str()), ("due", "none -> 2026-10-09", "alice"));

    // change it
    app.handle_key(key(KeyCode::Char('e')), &mut s);
    app.handle_key(key(KeyCode::Tab), &mut s);
    for _ in 0..10 {
        app.handle_key(key(KeyCode::Backspace), &mut s);
    }
    typed(&mut app, &mut s, "2026-11-02");
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(s.card(1).unwrap().due.as_deref(), Some("2026-11-02"));

    // clear it: an empty field is `--due none`
    app.handle_key(key(KeyCode::Char('e')), &mut s);
    app.handle_key(key(KeyCode::Tab), &mut s);
    for _ in 0..10 {
        app.handle_key(key(KeyCode::Backspace), &mut s);
    }
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(s.card(1).unwrap().due, None, "an empty field clears the date");
    let last = s.show(1).unwrap().events.pop().unwrap();
    assert_eq!(last.text.as_str(), "2026-11-02 -> none");
    // the board shows what changed
    assert!(!render(&app, 140, 40).contains("due 2026"), "the card line has no date any more");
}

/// The same parser and the same words as `tb edit --due`: the form refuses and stays open,
/// and nothing at all is written — not the date, not the title typed beside it.
#[test]
fn a_date_the_cli_would_refuse_is_refused_here_in_the_same_words() {
    for bad in ["2026-02-30", "10/09/2026", "tomorrow", "2026-1-5"] {
        let (_d, mut s, mut app) = open_form(Some("2026-10-09"));
        // change the title too: a refused date must not let half the form through
        app.handle_key(key(KeyCode::End), &mut s);
        typed(&mut app, &mut s, " again");
        app.handle_key(key(KeyCode::Tab), &mut s);
        for _ in 0..10 {
            app.handle_key(key(KeyCode::Backspace), &mut s);
        }
        typed(&mut app, &mut s, bad);
        app.handle_key(key(KeyCode::Enter), &mut s);
        assert!(matches!(app.mode, Mode::Edit(_)), "{bad}: the form stays open");
        let (msg, is_error) = app.status.clone().expect("a status line");
        assert!(is_error, "{bad}: shown as an error");
        let same_as_cli =
            terminal_board::store::due::DueDate::parse(bad, "tb edit 1 --due 2026-10-09").unwrap_err().to_string();
        assert_eq!(msg, same_as_cli, "{bad}: the form and the CLI say different things");
        let c = s.card(1).unwrap();
        assert_eq!((c.due.as_deref(), c.title.as_str()), (Some("2026-10-09"), "renewal"), "{bad}: nothing was written");
        assert_eq!(s.show(1).unwrap().events.iter().filter(|e| e.kind == "edit" || e.kind == "due").count(), 1, "{bad}");
    }
}

/// The stale-form rule the title and description follow covers the date too: a date the
/// person changed that somebody else changed while the form was open is refused, never
/// overwritten. A date they did NOT touch is left alone, so the form never puts an old
/// value back.
#[test]
fn the_stale_form_rule_covers_the_date() {
    let (_d, mut s, mut app) = open_form(Some("2026-10-09"));
    app.handle_key(key(KeyCode::Tab), &mut s);
    for _ in 0..10 {
        app.handle_key(key(KeyCode::Backspace), &mut s);
    }
    typed(&mut app, &mut s, "2026-11-02");
    // somebody else re-dates the card while the form is open
    let date = terminal_board::store::due::DueDate::parse("2026-12-24", "x").unwrap();
    s.set_due(1, date.as_ref(), "bob").unwrap();
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert!(matches!(app.mode, Mode::Edit(_)), "the form stays open");
    let (msg, is_error) = app.status.clone().expect("a status line");
    assert!(is_error && msg == "#1 changed while you were editing — the due date has a newer value; reopen with e", "{msg}");
    assert_eq!(s.card(1).unwrap().due.as_deref(), Some("2026-12-24"), "the other person's date stands");

    // a date the person did NOT touch is not written back, even though it moved
    let (_d, mut s, mut app) = open_form(Some("2026-10-09"));
    let date = terminal_board::store::due::DueDate::parse("2026-12-24", "x").unwrap();
    s.set_due(1, date.as_ref(), "bob").unwrap();
    app.handle_key(key(KeyCode::End), &mut s);
    typed(&mut app, &mut s, " again");
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(app.mode, Mode::Normal, "the title edit goes through");
    let c = s.card(1).unwrap();
    assert_eq!((c.due.as_deref(), c.title.as_str()), (Some("2026-12-24"), "renewal again"), "the untouched date is untouched");
}

/// A form nobody changed answers exactly as it did before the field existed: the refusal
/// `nothing to change`, and the form stays open. (Kept deliberately: this PR adds a field,
/// it does not change what Enter means.)
#[test]
fn a_form_saved_unchanged_refuses_as_it_always_did() {
    let (_d, mut s, mut app) = open_form(Some("2026-10-09"));
    let before = s.show(1).unwrap().events.len();
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert!(matches!(app.mode, Mode::Edit(_)), "the form stays open");
    let (msg, is_error) = app.status.clone().expect("a status line");
    assert!(is_error && msg.starts_with("nothing to change"), "{msg}");
    assert_eq!(s.show(1).unwrap().events.len(), before, "no event for a form nobody changed");
    assert_eq!(s.card(1).unwrap().due.as_deref(), Some("2026-10-09"));
    // and a form where ONLY the date changed still saves, without touching the text
    app.handle_key(key(KeyCode::Tab), &mut s);
    for _ in 0..10 {
        app.handle_key(key(KeyCode::Backspace), &mut s);
    }
    typed(&mut app, &mut s, "2026-11-02");
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(s.card(1).unwrap().due.as_deref(), Some("2026-11-02"));
    assert!(!s.show(1).unwrap().events.iter().any(|e| e.kind == "edit"), "the text was not rewritten");
}

/// The date the form writes is the date that was typed — the form is not a second way to
/// get a date wrong. (The CLI proves this across time zones; here it is the same store call.)
#[test]
fn the_form_stores_the_date_as_typed() {
    for date in ["2026-10-09", "2026-03-08", "2028-02-29", "2027-01-05"] {
        let (_d, mut s, mut app) = open_form(None);
        app.handle_key(key(KeyCode::Tab), &mut s);
        typed(&mut app, &mut s, date);
        app.handle_key(key(KeyCode::Enter), &mut s);
        assert_eq!(s.card(1).unwrap().due.as_deref(), Some(date));
        let raw: Option<String> =
            rusqlite::Connection::open(_d.path().join("b.db")).unwrap().query_row("SELECT due FROM cards WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(raw.as_deref(), Some(date), "the file holds the text that was typed");
    }
}

/// D1/D2 (review of #125): the form must never draw past its own box. `draw_edit` used to
/// put the description label at a fixed row 8, so at a terminal height of exactly 10 it wrote
/// outside the buffer and killed tb — on an ordinary split pane, from one keystroke.
///
/// This sweeps every size the reviewer swept and more: it drives the draw path directly,
/// because a panic is not something a golden file can catch.
#[test]
fn the_form_never_draws_outside_its_box_at_any_size() {
    let (_d, mut store, mut app) = open_form(Some("2026-10-09"));
    let mut drawn_at = [0usize; 3];
    for field in 0..3u8 {
        // the same form, with the cursor in each field in turn (tab, as a person would)
        while !matches!(&app.mode, Mode::Edit(f) if f.field == field) {
            app.handle_key(key(KeyCode::Tab), &mut store);
        }
        for h in 4u16..=30 {
            for w in [4u16, 8, 12, 20, 40, 60, 79, 80, 81, 120, 200] {
                // must not panic — this is the defect
                let screen = render(&app, w, h);
                assert_eq!(screen.lines().count(), h as usize, "{w}x{h}: the render is the size of the terminal");
                for line in screen.lines() {
                    assert_eq!(line.chars().count(), w as usize, "{w}x{h}: a row is not {w} cells");
                }
                // every field that IS shown is whole: its label and a box under it
                for (name, label) in [(0u8, "Title"), (1, "Due  (YYYY-MM-DD"), (2, "Description")] {
                    if let Some(row) = screen.lines().position(|l| l.contains(label)) {
                        assert!(row + 1 < h as usize, "{w}x{h}: {label} is the last row, so its box is off screen");
                        let under = screen.lines().nth(row + 1).unwrap();
                        assert!(under.contains('┌') || under.contains('┏'), "{w}x{h}: no box under {label}:\n{screen}");
                        drawn_at[name as usize] += 1;
                    }
                }
                // the field being typed into is always one of them, once there is room at all
                if screen.contains("Edit #") {
                    // a narrow pane cuts the label, so match the part that always fits
                    let short = ["Title", "Due", "Desc"][field as usize];
                    assert!(screen.contains(short), "{w}x{h}: field {field} is being typed into but not drawn:\n{screen}");
                }
            }
        }
    }
    assert!(drawn_at.iter().all(|n| *n > 0), "the sweep never drew one of the fields: {drawn_at:?}");
}

/// What the form does when the pane is genuinely too short: it drops fields from the end,
/// keeps the one being typed into, and says which are not on screen.
#[test]
fn a_short_pane_drops_fields_and_says_so() {
    use terminal_board::tui::form_fields_for;
    // 4 rows of interior per field: the whole form needs 12
    assert_eq!(form_fields_for(12, 0), [0, 1, 2], "all three");
    assert_eq!(form_fields_for(11, 0), [0, 1], "no room for the description");
    assert_eq!(form_fields_for(7, 0), [0], "title only");
    assert_eq!(form_fields_for(3, 0), Vec::<u8>::new(), "not even one");
    // the field being typed into is never the one dropped
    assert_eq!(form_fields_for(7, 2), [2], "the description is what the cursor is in");
    assert_eq!(form_fields_for(11, 2), [0, 2]);
    assert_eq!(form_fields_for(12, 2), [0, 1, 2]);

    // and on screen, the ones left out are named
    let (_d, _s, app) = open_form(Some("2026-10-09"));
    let screen = render(&app, 80, 14);
    assert!(screen.contains("Title") && screen.contains("Due  (YYYY-MM-DD"), "{screen}");
    assert!(!screen.contains("Description"), "14 rows is too short for the third field:\n{screen}");
    assert!(screen.contains("description not shown — make the pane taller"), "{screen}");
    // one row taller than the whole form: everything, and no notice
    let screen = render(&app, 80, 20);
    assert!(screen.contains("Description") && !screen.contains("not shown"), "{screen}");

    // card #107: the notice is guarded by `used < pad.height`, so at exactly the heights
    // where the shown fields fill the pad with no row to spare (h=8, pad=4, one field; h=12,
    // pad=8, two fields) a hidden field got no notice at all. This one height (14) missed
    // that gap, so the sweep now covers every height from "too short to draw at all" through
    // "everything fits" — whatever is missing on screen must be named on screen, always.
    for h in 4u16..=30 {
        let (_d, _s, app) = open_form(Some("2026-10-09"));
        let screen = render(&app, 80, h);
        if !screen.contains("Edit #") {
            continue; // too short to draw the form at all (see the width/height guard above)
        }
        let mut hidden = Vec::new();
        if !screen.contains("Title") {
            hidden.push("title");
        }
        if !screen.contains("Due  (YYYY-MM-DD") {
            hidden.push("due");
        }
        if !screen.contains("Description") {
            hidden.push("description");
        }
        if hidden.is_empty() {
            assert!(!screen.contains("not shown"), "h={h}: nothing is hidden but a notice is shown:\n{screen}");
        } else {
            let want = format!("{} not shown", hidden.join(" and "));
            assert!(screen.contains(&want), "h={h}: {} hidden but the notice does not say so:\n{screen}", hidden.join(" and "));
        }
    }
}

/// A card with no due date renders exactly as it did: the field is in the form, which no
/// golden covers, and nothing about the board itself moved.
#[test]
fn a_board_without_dates_renders_as_before() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    common::seed(&s, "alice").unwrap();
    let app = App::new(s.snapshot().unwrap(), "alice");
    let board = render(&app, 126, 41);
    assert!(!board.contains("Due  (YYYY-MM-DD"), "the form's field is only in the form");
    assert!(!board.contains("Edit #"));
    // and the form on such a card opens with an empty date, changing nothing else
    let (_d, _s, app) = open_form(None);
    let screen = render(&app, 140, 40);
    assert!(screen.contains("Edit #1") && screen.contains("Title") && screen.contains("Description"));
    assert!(screen.contains("tab field  left/right home/end move  enter save  esc cancel"), "the footer is unchanged");
}
