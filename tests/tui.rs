mod common;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use terminal_board::herdr::{parse_agents, AgentsState};
use terminal_board::plain::{fit, meta, meta_fit};
use terminal_board::store::Seed;
use terminal_board::store::Store;
use ratatui::style::{Color, Modifier};
use terminal_board::tui::{draw, palette, App, Mode, GREEN, RED, TODO_RED_DARK, TODO_RED_LIGHT};

fn text(buf: &Buffer) -> String {
    let w = buf.area.width as usize;
    buf.content
        .chunks(w)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn seeded() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    common::seed(&s, "alice").unwrap();
    (dir, s)
}

const BOX: &str = "─│┌┐└┘┏┓┗┛━┃├┤┬┴┼";

/// Every cell sits on the theme bg; frames are plain fg; accents are NAMED ANSI colours only
/// (Rgb only for the two TODO reds); red cells are only warning text. Returns the number of red cells.
fn assert_palette(buf: &Buffer, theme: &str) -> usize {
    let p = palette(theme);
    let todo = if theme == "light" { TODO_RED_LIGHT } else { TODO_RED_DARK };
    let allowed = [p.fg, Color::Red, Color::Green, todo, Color::Blue, Color::Yellow, Color::Magenta, Color::Cyan];
    let mut red = 0;
    for y in 0..buf.area.height {
        let mut run = String::new();
        let mut runs = Vec::new();
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            assert_eq!(c.bg, p.bg, "cell ({x},{y}) {:?} bg in {theme}", c.symbol());
            let rgb_ok = c.fg == TODO_RED_DARK || c.fg == TODO_RED_LIGHT;
            assert!(rgb_ok || !matches!(c.fg, Color::Rgb(..) | Color::Indexed(_)), "cell ({x},{y}) uses {:?}", c.fg);
            assert!(allowed.contains(&c.fg), "cell ({x},{y}) {:?} has {:?} in {theme}", c.symbol(), c.fg);
            if BOX.contains(c.symbol()) {
                let frames = [p.fg, todo, Color::Blue, Color::Yellow, Color::Green];
                assert!(frames.contains(&c.fg), "frame cell ({x},{y}) {:?} is {:?}", c.symbol(), c.fg);
            }
            if !BOX.contains(c.symbol()) && !c.symbol().trim().is_empty() && c.fg != p.fg && c.fg != RED {
                let header_row = (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .contains("o TODO ");
                assert!(
                    (c.fg == GREEN && c.symbol() == "*") || header_row,
                    "coloured text at ({x},{y}) {:?} {:?}: only red warnings, the green '*' and column headers may be coloured",
                    c.symbol(),
                    c.fg
                );
            }
            if c.fg == RED {
                red += 1;
                run.push_str(c.symbol());
            } else if !run.is_empty() {
                runs.push(std::mem::take(&mut run));
            }
        }
        if !run.is_empty() {
            runs.push(run);
        }
        for r in runs {
            let t = r.trim();
            let ok = t.starts_with("x b") || t.starts_with("! ") || t == "!" || t == "FAIL";
            assert!(ok, "red non-warning on row {y}: {r:?}");
        }
    }
    red
}

/// Each column: frame, dot and header text in the column colour (= its card colour); thick when focused.
fn assert_frames(screen: &str, buf: &Buffer, focused: &str, theme: &str) {
    let todo = if theme == "light" { TODO_RED_LIGHT } else { TODO_RED_DARK };
    for (name, colour) in [("TODO", todo), ("DOING", Color::Blue), ("REVIEW", Color::Yellow), ("DONE", Color::Green)] {
        let needle = format!("o {name} ");
        let (row, line) = screen.lines().enumerate().find(|(_, l)| l.contains(&needle)).expect(name);
        let x = line[..line.find(&needle).unwrap()].chars().count();
        let (x, y) = (x as u16, row as u16);
        let corner = &buf[(x - 2, y)];
        assert_eq!(corner.symbol(), if name == focused { "┏" } else { "┌" }, "{name} corner");
        assert_eq!(corner.fg, colour, "{name} column frame = its card colour");
        assert_eq!(buf[(x - 2, y + 1)].fg, colour, "{name} column edge = its card colour");
        assert_eq!(buf[(x, y)].fg, colour, "{name} dot colour");
        assert_eq!(buf[(x + 2, y)].fg, colour, "{name} header text colour");
    }
}

/// The colour of every cell of the first occurrence of `needle`.
fn colours_of(screen: &str, buf: &Buffer, needle: &str) -> Vec<ratatui::style::Color> {
    let (row, line) = screen.lines().enumerate().find(|(_, l)| l.contains(needle)).expect(needle);
    let col = line[..line.find(needle).unwrap()].chars().count();
    (0..needle.chars().count()).map(|i| buf[((col + i) as u16, row as u16)].fg).collect()
}

fn render(app: &App, w: u16, h: u16) -> (String, Buffer) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    (text(&buf), buf)
}

const AGENTS: &str = r#"{"result":{"agents":[
  {"name":"bot-2","agent":"aider","agent_status":"working","pane_id":"w:p5"},
  {"name":"alice","agent":"claude","agent_status":"idle","pane_id":"w:p2"},
  {"agent":"claude","agent_status":"done","pane_id":"w:p1"},
  {"name":"bot-4","agent":"aider","agent_status":"working","pane_id":"w:p7"}
]}}"#;
const PANES: &str = r#"{"result":{"panes":[
  {"agent":"aider","agent_status":"working","label":"builder 2 · model-x · aider · fix #327","pane_id":"w:p5"},
  {"agent":"claude","agent_status":"done","label":"lead (claude)","pane_id":"w:p1"},
  {"agent":"aider","agent_status":"working","label":"builder 4 · model-x · aider · fix login","pane_id":"w:p7"}
]}}"#;

#[test]
fn board_renders_columns_cards_and_agents() {
    let (_d, s) = seeded();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.agents = AgentsState::Agents(parse_agents(AGENTS, Some(PANES)).unwrap());
    let (screen, buf) = render(&app, 140, 45);
    for want in [
        "TERMINAL BOARD · default · 10 cards · 4 agents (2 working, 2 idle)",
        "lead ",
        "fix login",
        "TODO (4)",
        "DOING (2/3)",
        "REVIEW (1)",
        "DONE today (3)",
        "#1 gh#308 csv export",
        "renew domain",
        "login form rejects",
        "\"patch applied",
        "AGENTS",
        "! idle, holds card",
        "a add",
        "q quit",
    ] {
        assert!(screen.contains(want), "missing {want:?} in:\n{screen}");
    }
    assert!(assert_palette(&buf, "dark") > 0);
    assert_frames(&screen, &buf, "TODO", "dark");
    // agents frame plain, working mark green, gh refs plain
    assert!(colours_of(&screen, &buf, "AGENTS").iter().all(|c| *c == palette("dark").fg));
    assert!(colours_of(&screen, &buf, "┌ AGENTS").iter().all(|c| *c == palette("dark").fg), "agents frame plain");
    // tags are plain text
    for t in ["widgets - ", "admin - ", "ops - "] {
        assert!(colours_of(&screen, &buf, t).iter().all(|c| *c == palette("dark").fg), "{t} plain");
    }
    assert!(colours_of(&screen, &buf, "due ").iter().all(|c| *c == palette("dark").fg), "due is plain");
    assert_eq!(colours_of(&screen, &buf, "* bot-2")[0], GREEN);
    assert_eq!(colours_of(&screen, &buf, "* bot-2")[2], palette("dark").fg, "agent name stays plain");
    assert!(colours_of(&screen, &buf, "gh#309").iter().all(|c| *c == palette("dark").fg), "gh refs plain");
    assert!(colours_of(&screen, &buf, "gh#327").iter().all(|c| *c == palette("dark").fg));
    assert!(!screen.contains("aging"));
    // unselected card titles are plain fg
    for t in ["retry backoff", "renew domain", "search index lags", "dark theme"] {
        assert!(colours_of(&screen, &buf, t).iter().all(|c| *c == palette("dark").fg), "{t}");
    }
    for (needle, colour) in [
        ("x blocked by #7", RED),
        ("! idle, holds card", RED),
    ] {
        assert!(colours_of(&screen, &buf, needle).iter().all(|c| *c == colour), "{needle} not {colour:?}");
    }
    // the title of a card with a warning stays monochrome
    assert!(colours_of(&screen, &buf, "vendor quote").iter().all(|c| *c == palette("dark").fg));
    // only single-width glyphs
    assert!(!screen.contains('⚠') && !screen.contains('●'));
    assert!(!screen.contains("lead ("));
}

#[test]
fn narrow_hides_agents_and_detail() {
    let (_d, s) = seeded();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    let (wide, _) = render(&app, 120, 40);
    assert!(wide.contains("herdr not available"));
    assert!(wide.contains("> #1"), "detail strip shows the selection");
    // narrower: no detail strip, and AGENTS stays visible (as a 1-line bar)
    let (narrow, _) = render(&app, 95, 40);
    assert!(narrow.contains("TODO (4)") && !narrow.contains("> #1"));
    assert!(narrow.contains(" AGENTS herdr not available   tab >"), "{narrow}");
}

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

#[test]
fn keys_add_move_note_popup() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.handle_key(key(KeyCode::Char('a')), &mut s);
    for c in "admin: renew domain".chars() {
        app.handle_key(key(KeyCode::Char(c)), &mut s);
    }
    app.handle_key(key(KeyCode::Enter), &mut s);
    let c = s.card(1).unwrap();
    assert_eq!((c.tag.as_deref(), c.title.as_str()), (Some("admin"), "renew domain"));
    app.handle_key(key(KeyCode::Char('>')), &mut s);
    assert_eq!(s.card(1).unwrap().column, "doing");
    assert_eq!(app.col, 1, "selection follows the card");
    app.handle_key(key(KeyCode::Char('n')), &mut s);
    for c in "rang".chars() {
        app.handle_key(key(KeyCode::Char(c)), &mut s);
    }
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(s.snapshot().unwrap().last_note.get(&1).unwrap(), "rang");
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(app.mode, Mode::Popup(1));
    let (screen, _) = render(&app, 120, 40);
    assert!(screen.contains("esc close") && screen.contains("me: rang"), "{screen}");
    app.handle_key(key(KeyCode::Esc), &mut s);
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    assert_eq!(s.card(1).unwrap().column, "review");
    assert!(app.handle_key(key(KeyCode::Char('q')), &mut s));
}

/// Meta lines drop whole low-priority parts, keep warnings, and never emit `~`.
#[test]
fn meta_fit_drops_whole_parts_by_priority() {
    let (_d, s) = seeded();
    let snap = s.snapshot().unwrap();
    for c in &snap.cards {
        let full = meta(c, &snap);
        let tokens: Vec<&str> = full.split("  ").next().unwrap().split(" - ").collect();
        for w in 0..45 {
            let (base, warn) = meta_fit(c, &snap, w);
            assert!(!base.contains('~') && !warn.contains('~'));
            if !base.is_empty() {
                assert!(base.split(" - ").all(|t| tokens.contains(&t)), "cut token at {w}: {base:?}");
                let n = base.chars().count() + warn.chars().count() + usize::from(!warn.is_empty());
                assert!(n <= w, "{base:?} {warn:?} > {w}");
            }
            if full.contains("x blocked by") && w >= 15 {
                assert!(warn.contains("x blocked by"), "warning kept at {w}");
            }
        }
    }
    // the 125-column cases from live use (inner meta width ~22)
    let review = snap.cards.iter().find(|c| c.column == "review").unwrap();
    assert_eq!(meta_fit(review, &snap, 30).0, "widgets - rev - 12m - 2/3");
    assert_eq!(meta_fit(review, &snap, 22).0, "rev - 12m - 2/3", "tag dropped first");
    assert_eq!(meta_fit(review, &snap, 12).0, "rev - 12m", "then checklist");
    assert_eq!(meta_fit(review, &snap, 5).0, "rev", "then age; owner last");
    let quote = snap.cards.iter().find(|c| c.title == "vendor quote").unwrap();
    assert_eq!(meta_fit(quote, &snap, 22), ("admin - alice - 2d".into(), String::new()), "no aging warning");
    assert_eq!(fit("patch applied, tests running", 10), "patch app…");
}

#[test]
fn narrow_columns_use_ellipsis_not_tilde() {
    let (_d, s) = seeded();
    let app = App::new(s.snapshot().unwrap(), "alice");
    let (screen, _) = render(&app, 125, 40);
    assert!(!screen.contains('~'), "{screen}");
    assert!(screen.contains('…'), "long title/note cut with ellipsis:\n{screen}");
    assert!(screen.contains("x blocked by #7"));
    for bad in ["- 2/ ", "- 2/│", "- 40m - 2│", " - │"] {
        assert!(!screen.contains(bad), "clipped token {bad:?}:\n{screen}");
    }
}

#[test]
fn done_column_shows_last_24h_only() {
    let (_d, s) = seeded();
    s.seed(&Seed {
        title: "ancient history",
        desc: "",
        column: "done",
        owner: None,
        age_secs: 3 * 86400,
        due: None,
        checks: &[],
        notes: &[],
    })
    .unwrap();
    let snap = s.snapshot().unwrap();
    assert_eq!(snap.in_column("done").len(), 4, "list still sees every done card");
    assert_eq!(snap.on_board("done").len(), 3);
    let (screen, _) = render(&App::new(snap.clone(), "x"), 140, 40);
    assert!(screen.contains("DONE today (3)") && !screen.contains("ancient history"));
    assert!(!terminal_board::plain::board(&snap).contains("ancient history"));
    assert!(terminal_board::plain::list(&snap).contains("ancient history"));
}

#[test]
fn both_themes_render_and_toggle_persists() {
    let (_d, mut s) = seeded();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.agents = AgentsState::Agents(parse_agents(AGENTS, Some(PANES)).unwrap());
    let (_, buf) = render(&app, 140, 45);
    assert_palette(&buf, "dark");
    assert_eq!(palette("dark").bg, ratatui::style::Color::Black);
    app.handle_key(key(KeyCode::Char('T')), &mut s);
    assert_eq!(s.theme().unwrap(), "light", "T persists to config");
    let (screen, buf) = render(&app, 140, 45);
    assert_palette(&buf, "light");
    assert_frames(&screen, &buf, "TODO", "light");
    // focus moves the thick frame; detail strip and popup frames take the card's column colour
    app.handle_key(key(KeyCode::Right), &mut s);
    let (screen, buf) = render(&app, 140, 45);
    assert_frames(&screen, &buf, "DOING", "light");
    let strip = screen.lines().position(|l| l.contains("> #6 vendor quote")).unwrap();
    assert_eq!(buf[(0, strip as u16)].fg, Color::Black, "detail strip frame plain");
    app.handle_key(key(KeyCode::Enter), &mut s);
    let (screen, buf) = render(&app, 90, 30);
    assert_palette(&buf, "light");
    let corner = colours_of(&screen, &buf, "┏ #6 vendor quote");
    assert_eq!(corner[0], Color::Blue, "popup frame = doing colour");
    let admin = colours_of(&screen, &buf, "admin - alice");
    assert!(admin.iter().all(|c| *c == Color::Black), "popup header tag plain");
    app.handle_key(key(KeyCode::Esc), &mut s);
    app.handle_key(key(KeyCode::Char('T')), &mut s);
    assert_eq!(s.theme().unwrap(), "dark");
}

fn items(s: &Store, id: i64) -> Vec<(i64, String, bool)> {
    s.show(id).unwrap().checklist.into_iter().map(|c| (c.idx, c.text, c.done)).collect()
}

fn typed(app: &mut App, s: &mut Store, text: &str) {
    for c in text.chars() {
        app.handle_key(key(KeyCode::Char(c)), s);
    }
    app.handle_key(key(KeyCode::Enter), s);
}

#[test]
fn popup_checklist_cursor_toggle_add_delete() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = s.add("card", "", &["one".into(), "two".into(), "three".into()], "me").unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(app.mode, Mode::Popup(id));
    let (screen, _) = render(&app, 120, 40);
    assert!(screen.contains("up/down select  enter check  a add  d delete  n note  esc close"), "{screen}");
    // cursor down, enter toggles item 2
    app.handle_key(key(KeyCode::Down), &mut s);
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(items(&s, id)[1], (2, "two".into(), true));
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert!(!items(&s, id)[1].2, "enter toggles back");
    app.handle_key(key(KeyCode::Enter), &mut s);
    // the cursor row is reversed
    let (screen, buf) = render(&app, 120, 40);
    let row = screen.lines().position(|l| l.contains("[x] 2 two")).unwrap();
    let col = screen.lines().nth(row).unwrap().find("[x] 2").unwrap();
    assert!(buf[(col as u16, row as u16)].modifier.contains(ratatui::style::Modifier::REVERSED));
    // a adds item 4 and moves the cursor to it
    app.handle_key(key(KeyCode::Char('a')), &mut s);
    typed(&mut app, &mut s, "four");
    assert_eq!(app.mode, Mode::Popup(id));
    assert_eq!(items(&s, id).last().unwrap(), &(4, "four".into(), false));
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert!(items(&s, id)[3].2, "cursor is on the new item");
    // esc cancels an add
    app.handle_key(key(KeyCode::Char('a')), &mut s);
    app.handle_key(key(KeyCode::Char('x')), &mut s);
    app.handle_key(key(KeyCode::Esc), &mut s);
    assert_eq!(items(&s, id).len(), 4);
    // d deletes item 2 and renumbers
    app.handle_key(key(KeyCode::Up), &mut s);
    app.handle_key(key(KeyCode::Up), &mut s);
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    assert_eq!(
        items(&s, id),
        vec![(1, "one".into(), false), (2, "three".into(), false), (3, "four".into(), true)]
    );
    // delete at the end keeps the cursor in range
    app.handle_key(key(KeyCode::Down), &mut s);
    app.handle_key(key(KeyCode::Down), &mut s);
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    assert_eq!(items(&s, id).len(), 2);
    assert!(app.cursor < 2);
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert!(items(&s, id)[app.cursor].2);
    // history records adds and deletes
    let texts: Vec<String> = s.show(id).unwrap().events.into_iter().map(|e| e.text).collect();
    assert!(texts.contains(&"+ four".to_string()) && texts.contains(&"- two".to_string()), "{texts:?}");
    // n and esc still work
    app.handle_key(key(KeyCode::Char('n')), &mut s);
    typed(&mut app, &mut s, "hi");
    assert_eq!(app.mode, Mode::Popup(id));
    app.handle_key(key(KeyCode::Esc), &mut s);
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn popup_a_and_d_never_touch_the_board() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = s.add("empty", "", &[], "me").unwrap();
    s.take(id, "me").unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.handle_key(key(KeyCode::Right), &mut s);
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(app.mode, Mode::Popup(id));
    // no items: enter and d do nothing
    app.handle_key(key(KeyCode::Enter), &mut s);
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    assert_eq!(app.mode, Mode::Popup(id));
    assert_eq!(s.card(id).unwrap().column, "doing", "popup d is not board done");
    // a adds a check item, not a card
    app.handle_key(key(KeyCode::Char('a')), &mut s);
    typed(&mut app, &mut s, "first");
    assert_eq!(s.list().unwrap().len(), 1, "popup a is not board add");
    assert_eq!(items(&s, id), vec![(1, "first".into(), false)]);
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    assert!(items(&s, id).is_empty());
    assert_eq!(s.card(id).unwrap().column, "doing");
    assert_eq!(s.list().unwrap().len(), 1);
}

#[test]
fn board_without_problems_has_no_warning_colour() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    for t in ["admin: renew domain", "widgets: gh#5 tidy", "ops: outline"] {
        s.add(t, "", &["one".into()], "me").unwrap();
    }
    s.take(2, "me").unwrap();
    s.done(3, "me").unwrap();
    s.add("later", "", &[], "me").unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.agents = AgentsState::Agents(parse_agents(AGENTS, Some(PANES)).unwrap());
    for theme in ["dark", "light"] {
        s.set_theme(theme).unwrap();
        app.snap = s.snapshot().unwrap();
        let (screen, buf) = render(&app, 140, 40);
        assert_eq!(assert_palette(&buf, theme), 0, "no problems, no red/amber:\n{screen}");
        assert_frames(&screen, &buf, "TODO", theme);
    }
}

#[test]
fn wip_full_doing_count_is_reversed() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(1).unwrap();
    s.add("only", "", &[], "me").unwrap();
    s.next("me").unwrap();
    let app = App::new(s.snapshot().unwrap(), "me");
    let (screen, buf) = render(&app, 120, 30);
    let (row, line) = screen.lines().enumerate().find(|(_, l)| l.contains("DOING (1/1)")).unwrap();
    let x = line[..line.find("1/1").unwrap()].chars().count() as u16;
    for i in 0..3 {
        let c = &buf[(x + i, row as u16)];
        assert!(c.modifier.contains(Modifier::REVERSED), "count reversed when full");
        assert_eq!(c.fg, Color::Blue, "count keeps the column colour");
    }
    assert_frames(&screen, &buf, "TODO", "dark");
    assert_palette(&buf, "dark");
}

fn many(n: usize) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    for i in 1..=n {
        s.add(&format!("task {i}"), "", &[], "me").unwrap();
    }
    (dir, s)
}

/// (x, y) of the first cell of `needle`.
fn pos(screen: &str, needle: &str) -> (u16, u16) {
    let (row, line) = screen.lines().enumerate().find(|(_, l)| l.contains(needle)).expect(needle);
    (line[..line.find(needle).unwrap()].chars().count() as u16, row as u16)
}

#[test]
fn cards_render_in_their_own_coloured_boxes() {
    let (_d, s) = seeded();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    app.col = 1; // DOING focused; selected = #6 vendor quote
    let (screen, buf) = render(&app, 140, 45);
    // unselected TODO card #2: box corner two cells left of the title, one row up; column colour
    let (x, y) = pos(&screen, "#2 gh#309");
    let corner = &buf[(x - 2, y - 1)];
    assert_eq!((corner.symbol(), corner.fg), ("┌", TODO_RED_DARK), "todo card box");
    assert_eq!(buf[(x - 2, y)].symbol(), "│");
    assert_eq!(buf[(x - 1, y)].symbol(), " ", "1 space padding");
    // box height = content lines + 2: title, meta -> bottom border 2 rows below the title
    assert_eq!(buf[(x - 2, y + 2)].symbol(), "└");
    // selected DOING card: thick box in blue, bold title, no reversed video
    let (x, y) = pos(&screen, "#6 vendor quote");
    let corner = &buf[(x - 2, y - 1)];
    assert_eq!((corner.symbol(), corner.fg), ("┏", Color::Blue), "selected card thick");
    assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
    assert!(!buf[(x, y)].modifier.contains(Modifier::REVERSED));
    // doing card with a note: title, meta, note -> 3 content lines
    let (x, y) = pos(&screen, "#5 gh#327");
    assert_eq!((buf[(x - 2, y - 1)].symbol(), buf[(x - 2, y - 1)].fg), ("┌", Color::Blue));
    assert_eq!(buf[(x - 2, y + 3)].symbol(), "└");
    // review / done boxes in their colours
    let (x, y) = pos(&screen, "#7 gh#314");
    assert_eq!(buf[(x - 2, y - 1)].fg, Color::Yellow);
    let (x, y) = pos(&screen, "#8 gh#325");
    assert_eq!(buf[(x - 2, y - 1)].fg, Color::Green);
    assert_frames(&screen, &buf, "DOING", "dark");
    assert_palette(&buf, "dark");
}

#[test]
fn scrolling_keeps_selection_visible_with_20_cards() {
    let (_d, mut s) = many(20);
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    let (screen, _) = render(&app, 120, 40);
    assert!(screen.contains("#1 task 1") && !screen.contains("#20 task 20"));
    assert!(screen.contains(" more"), "bottom hint:\n{screen}");
    for target in [15usize, 20, 1] {
        while app.row[0] + 1 < target {
            app.handle_key(key(KeyCode::Down), &mut s);
        }
        while app.row[0] + 1 > target {
            app.handle_key(key(KeyCode::Up), &mut s);
        }
        let (screen, buf) = render(&app, 120, 40);
        // too many to fit: dense boxes carry the title in their (thick, when selected) top border
        let (x, y) = pos(&screen, &format!("#{target} task {target}"));
        assert_eq!(buf[(x - 2, y)].symbol(), "┏", "selected #{target} boxed thick, title in the border:\n{screen}");
        if target > 1 {
            let above = screen.lines().take(y as usize).any(|l| l.contains(" more"));
            assert!(above, "a '+N more' hint above the selection when scrolled:\n{screen}");
        }
        if target == 20 {
            assert!(!screen.contains("#1 task 1 "), "scrolled past the top");
        }
    }
    // tiny terminals never panic
    for h in 1..20 {
        for w in [20u16, 60, 120] {
            let _ = render(&app, w, h);
        }
    }
}

#[test]
fn compact_fallback_at_small_heights() {
    let (_d, mut s) = many(6);
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    app.handle_key(key(KeyCode::Down), &mut s);
    // 3+ rows inside a column: cards stay boxed, dense (title in the top border)
    let (screen, buf) = render(&app, 120, 18);
    let (x, y) = pos(&screen, "#2 task 2");
    assert_eq!(buf[(x - 2, y)].symbol(), "┏", "dense selected box:\n{screen}");
    // barely any room: the selected card's title line still shows, with +N more around it
    s.set_layout("half-h").unwrap(); // pinned: the four columns even when tiny
    app.reload(&s);
    let (screen, buf) = render(&app, 90, 9);
    let (x, y) = pos(&screen, "#2 task 2");
    assert!(buf[(x, y)].modifier.contains(Modifier::BOLD), "{screen}");
    assert!(screen.contains("+1 more") && screen.contains("+4 more"), "{screen}");
}

#[test]
fn plus_minus_adjust_wip_only_on_doing() {
    let (_d, mut s) = many(5);
    for _ in 0..3 {
        s.next("me").unwrap();
    }
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    // TODO focused: no effect, no hint
    let (screen, _) = render(&app, 120, 40);
    assert!(!screen.contains("+/- limit"));
    for c in ['+', '=', '-'] {
        app.handle_key(key(KeyCode::Char(c)), &mut s);
    }
    assert_eq!(s.wip().unwrap(), 3);
    // DOING focused
    app.handle_key(key(KeyCode::Right), &mut s);
    let (screen, _) = render(&app, 120, 40);
    assert!(screen.contains("+/- limit") && screen.contains("DOING (3/3)"));
    app.handle_key(key(KeyCode::Char('+')), &mut s);
    assert_eq!(s.wip().unwrap(), 4);
    app.handle_key(key(KeyCode::Char('=')), &mut s);
    assert_eq!(s.wip().unwrap(), 5);
    let (screen, _) = render(&app, 120, 40);
    assert!(screen.contains("DOING (3/5)"), "header updates immediately:\n{screen}");
    for _ in 0..10 {
        app.handle_key(key(KeyCode::Char('-')), &mut s);
    }
    assert_eq!(s.wip().unwrap(), 1, "floor of 1");
    let (screen, buf) = render(&app, 120, 40);
    let (x, y) = pos(&screen, "3/1");
    assert!(buf[(x, y)].modifier.contains(Modifier::REVERSED), "over the limit stays highlighted");
    assert_eq!(s.snapshot().unwrap().in_column("doing").len(), 3, "nothing kicked out");
    assert!(s.next("me").unwrap_err().to_string().contains("doing is full (3/1:"));
    let log: Vec<String> = s.board_events().unwrap().into_iter().map(|e| e.3).collect();
    assert_eq!(log.first().map(String::as_str), Some("wip 3 -> 4"));
    assert_eq!(log.last().map(String::as_str), Some("wip 2 -> 1"));
    assert_eq!(log.len(), 6, "one event per real change, none at the floor: {log:?}");
    // popup / input: +/- are not wip keys
    app.handle_key(key(KeyCode::Enter), &mut s);
    app.handle_key(key(KeyCode::Char('+')), &mut s);
    assert_eq!(s.wip().unwrap(), 1);
    // ceiling
    s.set_wip(99).unwrap();
    app.handle_key(key(KeyCode::Esc), &mut s);
    app.reload(&s);
    app.handle_key(key(KeyCode::Char('+')), &mut s);
    assert_eq!(s.wip().unwrap(), 99);
    assert!(s.set_wip(100).is_err());
}

#[test]
fn todo_is_the_bypass_permissions_red_per_theme() {
    assert_eq!(TODO_RED_DARK, Color::Rgb(255, 107, 128));
    assert_eq!(TODO_RED_LIGHT, Color::Rgb(171, 43, 63));
    assert_eq!(terminal_board::tui::column_colour_in("todo", "dark"), TODO_RED_DARK);
    assert_eq!(terminal_board::tui::column_colour_in("todo", "light"), TODO_RED_LIGHT);
    assert_eq!(terminal_board::tui::column_colour_in("doing", "light"), Color::Blue);
}

#[test]
fn panel_config_hides_panels_and_keys_persist() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.add("x", "", &[], "me").unwrap();
    s.set_panel("github-panel", "hidden").unwrap();
    s.set_panel("agents-panel", "hidden").unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "me");
    let (screen, _) = render(&app, 160, 50);
    assert!(!screen.contains("GITHUB") && !screen.contains("AGENTS"), "both hidden, not even the 'no repo' line:\n{screen}");
    // A / G show them again and remember it
    app.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE), &mut s);
    app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE), &mut s);
    assert!(s.panel("agents-panel").unwrap() && s.panel("github-panel").unwrap());
    app.reload(&s);
    let (screen, _) = render(&app, 160, 50);
    assert!(screen.contains("GITHUB") && screen.contains("AGENTS"), "{screen}");
}
