//! Arrow-key focus: columns -> GITHUB -> AGENTS, panel popups, card keys gated by focus.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Modifier;
use ratatui::Terminal;
use terminal_board::github::{GhSnapshot, GhView, Issue, Pr};
use terminal_board::herdr::{parse_agents, AgentsState};
use terminal_board::store::Store;
use terminal_board::tui::{draw, App, Focus, Mode};

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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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
