//! Arrow-key focus: columns -> GITHUB -> AGENTS, panel popups, card keys gated by focus.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Modifier;
use ratatui::Terminal;
use terminal_board::github::{GhSnapshot, GhView, Issue, Pr};
use terminal_board::herdr::{parse_agents, AgentsState};
use terminal_board::store::Store;
use terminal_board::tui::{draw, App, Focus, Mode, HELP_LAST_TEXT};

const AGENTS: &str = r#"{"result":{"agents":[
  {"name":"bot-2","agent":"aider","agent_status":"working","pane_id":"w:p5"},
  {"name":"rev","agent":"claude","agent_status":"idle","pane_id":"w:p2"}
]}}"#;

fn pr(n: i64, title: &str) -> Pr {
    Pr {
        number: n,
        title: title.into(),
        head_ref: format!("fix/{n}"),
        is_draft: false,
        review: "-".into(),
        ci: "ok".into(),
        created_at: "2026-09-18T08:00:00Z".into(),
        author: "bot".into(),
        closes: vec![],
    }
}

fn issue(n: i64, title: &str) -> Issue {
    Issue { number: n, title: title.into(), labels: vec!["bug".into()], assignees: vec![], created_at: "2026-09-18T07:00:00Z".into() }
}

fn setup() -> (tempfile::TempDir, Store, App) {
    std::env::set_var("TB_GH", "/nonexistent/gh"); // never reach a real gh
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    common::seed(&s, "alice").unwrap();
    s.set_github(Some("acme/widgets")).unwrap();
    // the snapshot lives in the store's cache, so reloads keep it
    s.save_github(&Ok(GhSnapshot {
        repo: "acme/widgets".into(),
        fetched_at: terminal_board::store::now(),
        issues_open: 2,
        prs: vec![pr(335, "api: rate limit")],
        issues: vec![issue(4200, "search: new facet bug — details"), issue(4199, "older issue")],
        merged_today: vec![],
        main_ci: None,
    }))
    .unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(&s);
    assert!(matches!(app.gh, GhView { snap: Some(_), .. }));
    app.agents = AgentsState::Agents(parse_agents(AGENTS, None).unwrap());
    (dir, s, app)
}

fn render(app: &App) -> (String, ratatui::buffer::Buffer) {
    let mut t = Terminal::new(TestBackend::new(160, 50)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer().clone();
    let text = b.content.chunks(160).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n");
    (text, b)
}

fn keym(c: KeyCode, m: KeyModifiers) -> KeyEvent {
    KeyEvent::new(c, m)
}

/// Render at a small size so the app is in the FOCUS shape.
fn render_small(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    t.backend().buffer().content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn press(app: &mut App, s: &mut Store, c: KeyCode, n: usize) {
    for _ in 0..n {
        app.handle_key(key(c), s);
        render(app); // panels shown are recorded per frame
    }
}

#[test]
fn arrows_walk_columns_github_agents_and_back() {
    let (_d, mut s, mut app) = setup();
    render(&app);
    press(&mut app, &mut s, KeyCode::Down, 3);
    assert_eq!((app.focus, app.col, app.row[0]), (Focus::Columns, 0, 3), "last TODO card");
    press(&mut app, &mut s, KeyCode::Down, 1);
    assert_eq!((app.focus, app.gh_sel), (Focus::Github, 0), "down off the last card -> GITHUB repo row");
    let (screen, _) = render(&app);
    assert!(screen.contains("repo: acme/widgets  (enter to change)"), "{screen}");
    assert!(screen.contains("up/down select  enter open"), "footer for github");
    assert!(screen.contains("┏ GITHUB"), "focused panel is thick:\n{screen}");
    // rows: repo, 1 PR, 2 issues -> 3 downs to the last, one more -> AGENTS
    press(&mut app, &mut s, KeyCode::Down, 1);
    let (screen, buf) = render(&app);
    let (row, line) = screen.lines().enumerate().find(|(_, l)| l.contains("┃ #335 ")).unwrap();
    let x = line[..line.find("#335").unwrap()].chars().count() as u16;
    assert!(buf[(x, row as u16)].modifier.contains(Modifier::REVERSED), "selected PR row reversed");
    press(&mut app, &mut s, KeyCode::Down, 2);
    assert_eq!((app.focus, app.gh_sel), (Focus::Github, 3));
    press(&mut app, &mut s, KeyCode::Down, 1);
    assert_eq!((app.focus, app.ag_sel), (Focus::Agents, 0));
    let (screen, _) = render(&app);
    assert!(screen.contains("enter details") && screen.contains("┏ AGENTS"), "{screen}");
    // left/right do nothing in a panel
    press(&mut app, &mut s, KeyCode::Left, 1);
    assert_eq!(app.focus, Focus::Agents);
    // up: agents -> github (keeps its row) -> ... -> columns at the same card
    press(&mut app, &mut s, KeyCode::Up, 1);
    assert_eq!(app.focus, Focus::Github);
    press(&mut app, &mut s, KeyCode::Up, 4);
    assert_eq!((app.focus, app.col, app.row[0]), (Focus::Columns, 0, 3));
    // tab cycles, shift-tab back, esc returns to the columns
    press(&mut app, &mut s, KeyCode::Tab, 1);
    assert_eq!(app.focus, Focus::Github);
    press(&mut app, &mut s, KeyCode::Tab, 1);
    assert_eq!(app.focus, Focus::Agents);
    press(&mut app, &mut s, KeyCode::Tab, 1);
    assert_eq!(app.focus, Focus::Columns);
    press(&mut app, &mut s, KeyCode::BackTab, 1);
    assert_eq!(app.focus, Focus::Agents);
    press(&mut app, &mut s, KeyCode::Esc, 1);
    assert_eq!(app.focus, Focus::Columns);
    let (screen, _) = render(&app);
    assert!(screen.contains("a add") && screen.contains("shift+arrows move") && screen.contains("? help"), "board footer back");
    // empty column: down goes straight to the panel
    app.col = 2;
    app.row[2] = 0;
    press(&mut app, &mut s, KeyCode::Down, 1);
    assert_eq!(app.focus, Focus::Github, "REVIEW has 1 card: already the last");
}

#[test]
fn card_keys_do_nothing_while_a_panel_has_focus() {
    let (_d, mut s, mut app) = setup();
    render(&app);
    let before: Vec<(i64, String)> = s.list().unwrap().into_iter().map(|c| (c.id, c.column)).collect();
    press(&mut app, &mut s, KeyCode::Tab, 1);
    assert_eq!(app.focus, Focus::Github);
    for c in ['a', 'd', '>', '<', 'n'] {
        press(&mut app, &mut s, KeyCode::Char(c), 1);
        assert_eq!(app.mode, Mode::Normal, "{c} opened nothing");
    }
    let after: Vec<(i64, String)> = s.list().unwrap().into_iter().map(|c| (c.id, c.column)).collect();
    assert_eq!(before, after);
    let notes = s.show(1).unwrap().events.len();
    assert_eq!(notes, s.show(1).unwrap().events.len());
}

#[test]
fn enter_on_repo_row_opens_the_picker() {
    let (_d, mut s, mut app) = setup();
    render(&app);
    press(&mut app, &mut s, KeyCode::Tab, 1);
    press(&mut app, &mut s, KeyCode::Enter, 1);
    assert!(matches!(app.mode, Mode::Picker { .. }));
    assert!(app.picker_preselect || !matches!(app.repos, terminal_board::tui::RepoState::Loading));
    press(&mut app, &mut s, KeyCode::Esc, 1);
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn issue_popup_adds_a_card_once() {
    let (_d, mut s, mut app) = setup();
    render(&app);
    press(&mut app, &mut s, KeyCode::Tab, 1);
    press(&mut app, &mut s, KeyCode::Down, 2); // repo -> PR -> first issue (#4200, newest)
    press(&mut app, &mut s, KeyCode::Enter, 1);
    assert_eq!(app.mode, Mode::GhItem { pr: false, number: 4200 });
    let (screen, _) = render(&app);
    for want in ["issue #4200", "search: new facet bug — details", "state unclaimed", "a add to board  o open in browser  esc close", "https://github.com/acme/widgets/issues/4200"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    let n = s.list().unwrap().len();
    press(&mut app, &mut s, KeyCode::Char('a'), 1);
    let cards = s.list().unwrap();
    assert_eq!(cards.len(), n + 1);
    let c = cards.iter().find(|c| c.gh_ref == Some(4200)).unwrap();
    assert_eq!((c.tag.as_deref(), c.title.as_str(), c.column.as_str()), (Some("widgets"), "new facet bug", "todo"));
    press(&mut app, &mut s, KeyCode::Char('a'), 1);
    assert_eq!(s.list().unwrap().len(), n + 1, "no duplicate");
    assert_eq!(app.status.as_ref().unwrap().0, format!("already on board as #{}", c.id));
    press(&mut app, &mut s, KeyCode::Esc, 1);
    assert_eq!((app.mode.clone(), app.focus), (Mode::Normal, Focus::Github));
    // PR popup
    press(&mut app, &mut s, KeyCode::Up, 1);
    press(&mut app, &mut s, KeyCode::Enter, 1);
    assert_eq!(app.mode, Mode::GhItem { pr: true, number: 335 });
    let (screen, _) = render(&app);
    assert!(screen.contains("PR #335") && screen.contains("branch fix/335"), "{screen}");
}

#[test]
fn agent_popup_jumps_to_the_held_card() {
    let (_d, mut s, mut app) = setup();
    render(&app);
    press(&mut app, &mut s, KeyCode::BackTab, 1);
    assert_eq!(app.focus, Focus::Agents);
    press(&mut app, &mut s, KeyCode::Enter, 1);
    assert_eq!(app.mode, Mode::AgentInfo(0));
    let (screen, _) = render(&app);
    assert!(screen.contains("bot-2") && screen.contains("holds #5") && screen.contains("enter jump to card"), "{screen}");
    press(&mut app, &mut s, KeyCode::Enter, 1);
    assert_eq!((app.mode.clone(), app.focus), (Mode::Normal, Focus::Columns));
    assert_eq!(app.selected().unwrap().id, 5);
}

#[test]
fn unconfigured_github_panel_is_focusable() {
    let (_d, mut s, mut app) = setup();
    s.set_github(None).unwrap();
    app.reload(&s);
    let (screen, _) = render(&app);
    assert!(screen.contains("no repo — enter to pick one"), "{screen}");
    press(&mut app, &mut s, KeyCode::Tab, 1);
    assert_eq!(app.focus, Focus::Github);
    press(&mut app, &mut s, KeyCode::Down, 1);
    assert_eq!(app.focus, Focus::Agents, "one row only");
    press(&mut app, &mut s, KeyCode::Up, 1);
    press(&mut app, &mut s, KeyCode::Enter, 1);
    assert!(matches!(app.mode, Mode::Picker { .. }));
}

#[test]
fn help_is_readable_and_scrollable_in_small_panes() {
    let (_d, mut s, mut app) = setup();
    app.actor = "bot-2".into();
    let _ = render_small(&app, 60, 14); // focus shape
    app.handle_key(key(KeyCode::Char('?')), &mut s);
    let mut at = |off: u16| {
        app.help_scroll = off;
        render_small(&app, 60, 14)
    };
    // every help line is reachable: the LAST group's last row appears after scrolling
    let last_row = HELP_LAST_TEXT;
    let screen0 = at(0);
    assert!(!screen0.contains(last_row), "top of the help at offset 0");
    // scroll to the bottom: the previously hidden rows are visible and readable
    let bottom = at(999);
    assert!(bottom.contains(last_row), "scrolled help shows the last rows");
    // no line is clipped at the right edge: the long description is present (wrapped)
    let tui = terminal_board::tui::HELP_DESC_SAMPLE;
    assert!(screen0.contains(tui) || bottom.contains(tui), "a long description survives at 60 cols");
    // keys scroll: two Down steps from the top bring the second group into view,
    // and Home returns to the top
    app.help_scroll = 0;
    app.handle_key(keym(KeyCode::Down, KeyModifiers::NONE), &mut s);
    app.handle_key(keym(KeyCode::Down, KeyModifiers::NONE), &mut s);
    let mid = render_small(&app, 60, 14);
    assert!(mid.contains("add a card"), "Down scrolls: {mid}");
    app.handle_key(keym(KeyCode::Home, KeyModifiers::NONE), &mut s);
    assert_eq!(app.help_scroll, 0, "Home returns to the top");
    assert!(render_small(&app, 60, 14).contains("select a card"), "back at the top");
}
