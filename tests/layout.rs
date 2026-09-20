//! Layouts from half a screen down to a third of it (TestBackend), with GitHub configured,
//! 6 agents and 12 cards: FULL (unchanged), RAIL (wide-short), STACK (tall-narrow), bars,
//! Tab views, TINY, and a size sweep.
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use terminal_board::github::{GhSnapshot, Issue, MainCi, Pr};
use terminal_board::herdr::{parse_agents, AgentsState};
use terminal_board::store::{Store, LAYOUTS};
use terminal_board::tui::{draw, pick_shape, App, Focus, Shape, View};

const AGENTS: &str = r#"{"result":{"agents":[
  {"name":"bot-1","agent":"aider","agent_status":"working","pane_id":"w:p1"},
  {"name":"bot-2","agent":"aider","agent_status":"idle","pane_id":"w:p2"},
  {"name":"bot-3","agent":"aider","agent_status":"working","pane_id":"w:p3"},
  {"name":"bot-4","agent":"codex","agent_status":"working","pane_id":"w:p4"},
  {"name":"reviewer","agent":"claude","agent_status":"working","pane_id":"w:p5"},
  {"name":"lead","agent":"claude","agent_status":"idle","pane_id":"w:p6"}
]}}"#;

fn ago(secs: i64) -> String {
    chrono::DateTime::from_timestamp(terminal_board::store::now() - secs, 0).unwrap().to_rfc3339()
}

fn pr(n: i64, title: &str, ci: &str) -> Pr {
    Pr {
        number: n,
        title: title.into(),
        head_ref: format!("fix/{}", n - 1),
        is_draft: false,
        review: "-".into(),
        ci: ci.into(),
        created_at: ago(2 * 3600),
        author: "bot-1".into(),
        closes: vec![n - 1],
        updated_at: String::new(),
    }
}

fn issue(n: i64, title: &str) -> Issue {
    Issue { number: n, title: title.into(), labels: vec!["bug".into()], assignees: vec![], created_at: ago(3 * 3600) }
}

fn setup() -> (tempfile::TempDir, Store, App) {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(5).unwrap();
    let cards = [
        ("docs: write install guide", "todo", None),
        ("widgets: gh#301 csv export drops header", "todo", None),
        ("ops: rotate API tokens", "todo", None),
        ("widgets: gh#305 login form rejects emails", "doing", Some("bot-1")),
        ("widgets: fix flaky upload test", "doing", Some("bot-2")),
        ("docs: screenshots for README", "doing", Some("bot-3")),
        ("widgets: gh#307 search index lags", "review", Some("reviewer")),
        ("ops: backup restore drill", "review", Some("bot-4")),
        ("widgets: dark theme colours", "review", Some("bot-1")),
        ("docs: changelog for 1.1", "done", Some("alice")),
        ("widgets: typo in footer", "done", Some("bot-3")),
        ("ops: renew domain", "done", Some("alice")),
    ];
    for (t, col, owner) in cards {
        let id = s.add(t, "", &[], "alice").unwrap();
        if col != "todo" {
            s.move_to(id, col, owner.unwrap_or("alice")).unwrap();
        }
    }
    s.set_github(Some("acme/widgets")).unwrap();
    s.save_github(&Ok(GhSnapshot {
        repo: "acme/widgets".into(),
        fetched_at: terminal_board::store::now(),
        issues_open: 10,
        prs: vec![pr(306, "fix login form validation", "FAIL"), pr(308, "speed up search indexing", "ok")],
        issues: vec![
            issue(305, "login form rejects plus-addresses"),
            issue(307, "search index lags behind writes"),
            issue(301, "csv export drops the header row"),
            issue(310, "dark mode contrast on buttons"),
            issue(311, "retry uploads with backoff"),
        ],
        merged_today: vec![],
        main_ci: Some(MainCi { state: "ok".into(), workflow: "ci".into(), created_at: ago(40 * 60) }),
    }))
    .unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(&s);
    app.agents = AgentsState::Agents(parse_agents(AGENTS, None).unwrap());
    (dir, s, app)
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn row_of(screen: &str, needle: &str) -> usize {
    screen.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle} missing:\n{screen}"))
}

fn col_of(screen: &str, needle: &str) -> usize {
    let l = screen.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle} missing:\n{screen}"));
    l[..l.find(needle).unwrap()].chars().count()
}

fn count(screen: &str, pat: &str) -> usize {
    screen.lines().filter(|l| l.contains(pat)).count()
}

#[test]
fn auto_shape_by_size() {
    for (w, h, shape) in [
        (40, 12, Shape::Focus),
        (50, 14, Shape::Focus),
        (30, 10, Shape::Focus),
        (126, 22, Shape::ThirdH),
        (200, 24, Shape::ThirdH),
        (260, 26, Shape::ThirdH),
        (50, 70, Shape::ThirdV),
        (42, 73, Shape::ThirdV),
        (60, 73, Shape::ThirdV),
        (126, 41, Shape::HalfH),
        (160, 50, Shape::HalfH),
        (95, 35, Shape::HalfH),
        (70, 70, Shape::HalfV),
        (90, 60, Shape::HalfV),
        (85, 60, Shape::HalfV),
    ] {
        assert_eq!(pick_shape("auto", w, h), shape, "{w}x{h}");
    }
    assert_eq!(pick_shape("third-v", 200, 50), Shape::ThirdV);
    assert_eq!(pick_shape("third-h", 60, 60), Shape::ThirdH);
    assert_eq!(pick_shape("focus", 200, 60), Shape::Focus);
}

#[test]
fn half_screen_126x41_is_unchanged() {
    let (_d, _s, app) = setup();
    let screen = render(&app, 126, 41);
    // boxed 4-row cards: the title is inside the box, under a plain top border
    let r = row_of(&screen, "#1 write install guide");
    let above = screen.lines().nth(r - 1).unwrap();
    assert!(above.contains("┏━━") || above.contains("┌──"), "4-row box:\n{screen}");
    for want in ["MAIN CI  ", "BRANCH / ISSUE", "LABELS", "┌ AGENTS", "bot-4", "lead", "o TODO (3)", "o DOING (3/5)"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
}

#[test]
fn third_height_rail_126x24_and_200x24() {
    for (w, h) in [(126u16, 24u16), (200, 24), (260, 26)] {
        let (_d, _s, app) = setup();
        let screen = render(&app, w, h);
        let gh_x = col_of(&screen, "GITHUB · widgets");
        assert!(gh_x > col_of(&screen, "o DONE today"), "{w}x{h}: the rail is right of the columns:\n{screen}");
        assert!(screen.contains("ISSUES") || screen.contains("issues 10"), "{w}x{h}: tiles or stat lines:\n{screen}");
        let rows = count(&screen, "PR    gh#3") + count(&screen, "ISSUE gh#3");
        assert!(rows >= 2, "{w}x{h}: >= 2 table rows:\n{screen}");
        assert!(screen.contains("AGENTS"), "{w}x{h}");
        let agents = ["bot-1", "bot-2", "bot-3", "bot-4", "reviewer", "lead"].iter().filter(|a| screen.contains(&format!(" {a} "))).count();
        assert!(agents >= 3, "{w}x{h}: >= 3 agent rows:\n{screen}");
        // columns keep boxed cards (dense 3-row boxes carry the title in the border)
        assert!(screen.contains("┌ #") || screen.contains("┏ #") || screen.contains("┃┌──"), "{w}x{h}: boxed cards:\n{screen}");
    }
}

#[test]
fn third_width_stack_42_60_85() {
    for (w, h) in [(42u16, 73u16), (60, 73), (50, 70)] {
        let (_d, _s, app) = setup();
        let screen = render(&app, w, h);
        let order: Vec<usize> =
            ["o TODO (3)", "o DOING (3/5)", "o REVIEW (3)", "o DONE today (3)", "GITHUB", "AGENTS"].iter().map(|n| row_of(&screen, n)).collect();
        assert!(order.windows(2).all(|p| p[0] < p[1]), "{w}x{h}: stacked top to bottom {order:?}:\n{screen}");
        assert!(screen.contains("┌ #") || screen.contains("┏ #") || screen.contains("┃┌──"), "{w}x{h}: boxed cards:\n{screen}");
        let gh_rows = count(&screen, "PR    gh#3") + count(&screen, "ISSUE gh#3");
        assert!(gh_rows >= 3, "{w}x{h}: >= 3 github rows:\n{screen}");
        for a in ["bot-1", "bot-2", "bot-3", "bot-4", "reviewer", "lead"] {
            assert!(screen.contains(a), "{w}x{h}: agent {a}:\n{screen}");
        }
    }
}

#[test]
fn stack_github_rows_are_focusable() {
    let (_d, mut s, mut app) = setup();
    render(&app, 60, 73);
    // walk down through every card, then into GITHUB
    for _ in 0..12 {
        app.handle_key(key(KeyCode::Down), &mut s);
        render(&app, 60, 73);
    }
    assert_eq!(app.focus, Focus::Github, "down past the last card");
    app.handle_key(key(KeyCode::Down), &mut s);
    let screen = render(&app, 60, 73);
    assert!(screen.contains("PR    gh#306"));
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert!(matches!(app.mode, terminal_board::tui::Mode::GhItem { pr: true, number: 306 }), "{:?}", app.mode);
}

#[test]
fn medium_bars_when_panels_do_not_fit() {
    let (_d, _s, app) = setup();
    let screen = render(&app, 95, 35);
    assert!(screen.contains(" GITHUB acme/widgets · 10 issues") && screen.contains("tab >"), "{screen}");
    assert!(screen.contains(" AGENTS 4 working · 2 idle"), "{screen}");
}

#[test]
fn tiny_tab_pages_through_the_panels() {
    let (_d, mut s, mut app) = setup();
    let screen = render(&app, 30, 10);
    assert!(screen.contains("GITHUB") && screen.contains("AGENTS"), "bars even when tiny:\n{screen}");
    app.handle_key(key(KeyCode::Tab), &mut s);
    assert_eq!(app.view, View::Github);
    let screen = render(&app, 30, 10);
    assert!(screen.contains("GITHUB") && screen.contains("gh#306"), "github view:\n{screen}");
    app.handle_key(key(KeyCode::Tab), &mut s);
    assert_eq!(app.view, View::Agents);
    let screen = render(&app, 30, 10);
    assert!(screen.contains("bot-1"), "agents view:\n{screen}");
    app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT), &mut s);
    assert_eq!(app.view, View::Github);
    app.handle_key(key(KeyCode::Tab), &mut s);
    app.handle_key(key(KeyCode::Tab), &mut s);
    assert_eq!((app.view, app.focus), (View::Board, Focus::Columns), "tab wraps to the board");
    app.handle_key(key(KeyCode::Tab), &mut s);
    app.handle_key(key(KeyCode::Esc), &mut s);
    assert_eq!(app.view, View::Board, "esc returns");
    // a focused bar opens its view with enter
    let screen = render(&app, 95, 35);
    assert!(screen.contains(" GITHUB acme/widgets"));
    app.focus = Focus::Columns;
    app.col = 0;
    app.row[0] = 2;
    app.handle_key(key(KeyCode::Down), &mut s);
    assert_eq!(app.focus, Focus::Github);
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(app.view, View::Github);
}

#[test]
fn every_size_keeps_enabled_panels_and_never_panics() {
    let (_d, s, mut app) = setup();
    for pref in LAYOUTS {
        s.set_layout(pref).unwrap();
        app.reload(&s);
        app.agents = AgentsState::Agents(parse_agents(AGENTS, None).unwrap());
        for w in (10..=260).step_by(7) {
            for h in (4..=75).step_by(7) {
                let screen = render(&app, w, h);
                if h >= 6 && pref == "auto" {
                    assert!(screen.contains("GITHUB") || screen.contains("GITHU"), "{pref} {w}x{h} lost GITHUB:\n{screen}");
                    assert!(screen.contains("AGENTS") || screen.contains("AGENT"), "{pref} {w}x{h} lost AGENTS:\n{screen}");
                }
            }
        }
    }
    for view in [View::Github, View::Agents] {
        app.view = view;
        for w in (10..=260).step_by(13) {
            for h in (4..=75).step_by(9) {
                render(&app, w, h);
            }
        }
    }
}

/// `cargo test --test layout -- --ignored --nocapture render_samples` prints the reference renders.
#[test]
#[ignore]
fn render_samples() {
    let (_d, _s, app) = setup();
    for (w, h, n) in [(126u16, 22u16, 22usize), (50, 70, 70), (70, 70, 45), (50, 14, 14), (126, 41, 41)] {
        println!("===== {w}x{h} =====");
        for l in render(&app, w, h).lines().take(n) {
            println!("{}", l.trim_end());
        }
    }
}

/// Card styles in a render: "dense" (title in the top border) or "full" (title inside).
fn card_styles(screen: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    for id in 1..=12 {
        let pat = format!("#{id} ");
        for l in screen.lines() {
            let mut from = 0;
            while let Some(i) = l[from..].find(&pat) {
                let at = from + i;
                let before: String = l[..at].chars().rev().take(2).collect::<Vec<_>>().into_iter().rev().collect();
                match before.as_str() {
                    "┌ " | "┏ " => out.push("dense"),
                    "│ " | "┃ " => out.push("full"),
                    _ => {}
                }
                from = at + pat.len();
            }
        }
    }
    out
}

#[test]
fn one_card_style_per_render() {
    // plenty of room: every column uses the 4-row boxes
    for (w, h) in [(126u16, 22u16), (200, 60)] {
        let (_d, _s, app) = setup();
        let styles = card_styles(&render(&app, w, h));
        assert!(styles.len() >= 8 && styles.iter().all(|s| *s == "full"), "{w}x{h}: {styles:?}");
    }
    // one crowded column (TODO with 7 cards): every column switches to the dense boxes
    for (w, h) in [(126u16, 22u16), (95, 22), (126, 41), (60, 73), (42, 73)] {
        let (_d, s, mut app) = setup();
        for i in 0..4 {
            s.add(&format!("docs: extra card {i}"), "", &[], "alice").unwrap();
        }
        app.reload(&s);
        let screen = render(&app, w, h);
        let styles = card_styles(&screen);
        assert!(styles.len() >= 4, "{w}x{h}: {styles:?}:\n{screen}");
        assert!(styles.iter().all(|s| *s == "dense"), "{w}x{h}: mixed styles {styles:?}:\n{screen}");
    }
}

#[test]
fn narrow_cards_move_gh_to_the_meta_line() {
    let (_d, _s, app) = setup();
    // 126x22: rail columns are ~19 wide, so the title gets the full width
    let screen = render(&app, 126, 22);
    assert!(screen.contains("#2 csv export"), "{screen}");
    assert!(screen.contains("gh#301 · "), "gh ref on the meta line:\n{screen}");
    // wide cards keep gh#N in the title line
    let screen = render(&app, 200, 60);
    assert!(screen.contains("#2 gh#301 csv export drops header"), "{screen}");
}

#[test]
fn panel_titles_drop_whole_tokens() {
    let (_d, s, mut app) = setup();
    let long = "someorganisation/a-rather-long-repository-name";
    s.set_github(Some(long)).unwrap();
    s.save_github(&Ok(GhSnapshot { repo: long.into(), fetched_at: terminal_board::store::now(), ..Default::default() })).unwrap();
    app.reload(&s);
    let screen = render(&app, 126, 24);
    assert!(screen.contains(" GITHUB · a-rather-long-repository-name "), "owner dropped, repo whole:\n{screen}");
    assert!(!screen.contains("someorganisation"), "{screen}");
    // enough room: owner/repo and the time stay
    let screen = render(&app, 260, 26);
    // the tidy block shows the repo name; the time sits right-aligned in the border
    assert!(screen.contains(" GITHUB · a-rather-long-repository-name "), "{screen}");
    let l = screen.lines().find(|l| l.contains("GITHUB · a-rather")).unwrap();
    assert!(l.contains(':'), "time in the border when there is room: {l}");
    // the agents title drops its counts before cutting
    let screen = render(&app, 42, 73);
    let l = screen.lines().find(|l| l.contains("AGENTS")).unwrap();
    assert!(l.contains(" AGENTS · 4 working · 2 idle ") || l.contains(" AGENTS · 4 working ") || l.contains(" AGENTS "), "{l}");
}

#[test]
fn arrows_are_spatial_in_every_layout() {
    // RAIL (126x22): GITHUB and AGENTS sit to the right of DONE
    let (_d, mut s, mut app) = setup();
    render(&app, 126, 22);
    app.handle_key(key(KeyCode::Right), &mut s);
    app.handle_key(key(KeyCode::Right), &mut s);
    app.handle_key(key(KeyCode::Right), &mut s);
    app.handle_key(key(KeyCode::Down), &mut s);
    render(&app, 126, 22);
    assert_eq!((app.focus, app.col, app.row[3]), (Focus::Columns, 3, 1));
    app.handle_key(key(KeyCode::Right), &mut s);
    assert_eq!(app.focus, Focus::Github, "-> from DONE reaches GITHUB");
    render(&app, 126, 22);
    app.handle_key(key(KeyCode::Left), &mut s);
    assert_eq!((app.focus, app.col, app.row[3]), (Focus::Columns, 3, 1), "<- returns to the same card");
    render(&app, 126, 22);
    app.handle_key(key(KeyCode::Right), &mut s);
    let rows = app.gh_rows();
    for _ in 0..rows {
        app.handle_key(key(KeyCode::Down), &mut s);
        render(&app, 126, 22);
    }
    assert_eq!(app.focus, Focus::Agents, "down past the last GITHUB row");
    app.handle_key(key(KeyCode::Up), &mut s);
    assert_eq!((app.focus, app.gh_sel), (Focus::Github, rows - 1), "up lands on the last GITHUB row");
    render(&app, 126, 22);
    app.handle_key(key(KeyCode::Down), &mut s);
    render(&app, 126, 22);
    app.handle_key(key(KeyCode::Left), &mut s);
    assert_eq!(app.focus, Focus::Columns, "<- from AGENTS goes back to the columns");

    // FULL (126x41): panels are below, so -> from DONE does nothing and down reaches GITHUB
    let (_d, mut s, mut app) = setup();
    render(&app, 126, 41);
    app.col = 3;
    app.row[3] = 2;
    app.handle_key(key(KeyCode::Right), &mut s);
    assert_eq!((app.focus, app.col), (Focus::Columns, 3), "nothing to the right");
    app.handle_key(key(KeyCode::Down), &mut s);
    assert_eq!(app.focus, Focus::Github);
    render(&app, 126, 41);
    app.handle_key(key(KeyCode::Left), &mut s);
    assert_eq!(app.focus, Focus::Github, "GITHUB is below the columns: <- is not a way back");

    // STACK (60x73): <-/-> jump between sections, -> from the last does nothing
    let (_d, mut s, mut app) = setup();
    render(&app, 60, 73);
    app.handle_key(key(KeyCode::Right), &mut s);
    assert_eq!(app.col, 1);
    app.handle_key(key(KeyCode::Right), &mut s);
    app.handle_key(key(KeyCode::Right), &mut s);
    app.handle_key(key(KeyCode::Right), &mut s);
    render(&app, 60, 73);
    assert_eq!((app.focus, app.col), (Focus::Columns, 3));
    app.handle_key(key(KeyCode::Left), &mut s);
    assert_eq!(app.col, 2);
}


// ---------- round 25: the five named views ----------

fn render_buf(app: &App, w: u16, h: u16) -> (String, ratatui::buffer::Buffer) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer().clone();
    let text = b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n");
    (text, b)
}

/// Clock times (HH:MM and HH:MM:SS) become ##:## so a golden render is stable.
fn normalize(screen: &str) -> String {
    let mut c: Vec<char> = screen.chars().collect();
    let d = |x: char| x.is_ascii_digit();
    let mut i = 0;
    while i + 5 <= c.len() {
        if d(c[i]) && d(c[i + 1]) && c[i + 2] == ':' && d(c[i + 3]) && d(c[i + 4]) {
            for k in [0, 1, 3, 4] {
                c[i + k] = '#';
            }
            if i + 8 <= c.len() && c[i + 5] == ':' && d(c[i + 6]) && d(c[i + 7]) {
                c[i + 6] = '#';
                c[i + 7] = '#';
            }
        }
        i += 1;
    }
    c.into_iter().collect::<String>().lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
}

#[test]
fn half_h_126x41_golden() {
    let (_d, _s, app) = setup();
    let got = normalize(&render(&app, 126, 41));
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/half_h_126x41.txt");
    if std::env::var("TB_UPDATE_GOLDEN").is_ok() {
        std::fs::write(path, format!("{got}\n")).unwrap();
    }
    let want = std::fs::read_to_string(path).expect("golden file (TB_UPDATE_GOLDEN=1 to create)");
    assert_eq!(got, want.trim_end_matches('\n'), "the half-h look changed");
}

#[test]
fn focus_view_picks_my_doing_card_then_top_todo_then_selection() {
    // my DOING card first
    let (_d, mut s, mut app) = setup();
    app.actor = "bot-2".into();
    let screen = render(&app, 50, 14);
    assert!(screen.starts_with(" TODO 3 · DOING 3/5 · REVIEW 3 · DONE 3"), "{screen}");
    assert!(screen.contains("fix flaky upload test") && screen.contains("o DOING"), "{screen}");
    // nothing in DOING for alice: the top TODO card
    app.actor = "alice".into();
    let screen = render(&app, 50, 14);
    assert!(screen.contains("write install guide") && screen.contains("o TODO"), "{screen}");
    // no TODO and nothing of mine: the selection
    for id in 1..=3 {
        s.move_to(id, "done", "alice").unwrap();
    }
    app.reload(&s);
    app.col = 2;
    app.row[2] = 1;
    let screen = render(&app, 50, 14);
    assert!(screen.contains("backup restore drill") && screen.contains("o REVIEW"), "{screen}");
    // GITHUB / AGENTS bars under the card
    assert!(screen.contains(" GITHUB ") && screen.contains(" AGENTS "), "{screen}");
}

#[test]
fn focus_view_keys() {
    let (_d, mut s, mut app) = setup();
    let id = s.add("docs: tidy up", "", &["one".into(), "two".into()], "alice").unwrap();
    s.move_to(id, "doing", "alice").unwrap();
    app.reload(&s);
    let (screen, _) = render_buf(&app, 50, 14);
    assert!(screen.contains("tidy up") && screen.contains("[ ] 1 one") && screen.contains("[ ] 2 two"), "{screen}");
    // enter ticks the first open item, then the next
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert!(s.show(id).unwrap().checklist[0].done);
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert!(s.show(id).unwrap().checklist[1].done);
    // <- -> walk the cards of the column (wrapping); up/down switch column
    let col = app.col;
    let first = app.selected().unwrap().id;
    app.handle_key(key(KeyCode::Right), &mut s);
    assert_eq!(app.col, col);
    assert_ne!(app.selected().unwrap().id, first);
    app.handle_key(key(KeyCode::Down), &mut s);
    assert_eq!(app.col, (col + 1) % 4);
    let screen = render(&app, 50, 14);
    assert!(screen.contains("o REVIEW"), "{screen}");
    // d / n / a still work on the focus card
    let rid = app.selected().unwrap().id;
    app.handle_key(key(KeyCode::Char('d')), &mut s);
    if matches!(app.mode, terminal_board::tui::Mode::Confirm { .. }) {
        app.handle_key(key(KeyCode::Char('y')), &mut s); // its issue is still open on GitHub
    }
    assert_eq!(s.card(rid).unwrap().column, "done");
    // tab pages to the github view
    app.handle_key(key(KeyCode::Tab), &mut s);
    assert_eq!(app.view, View::Github);
}

/// Columns of the tidy GITHUB block: (#number x, status end x) for every row.
fn tidy_geometry(screen: &str) -> (Vec<usize>, Vec<usize>) {
    let rows: Vec<&str> = screen.lines().filter(|l| l.contains("PR gh#3") || l.contains("ISSUE gh#3")).collect();
    let hash = rows.iter().map(|l| l.chars().position(|c| c == '#').unwrap()).collect();
    let end = rows.iter().map(|l| l.trim_end().trim_end_matches('│').trim_end().chars().count()).collect();
    (hash, end)
}

#[test]
fn tidy_github_block_aligns_at_62_48_40() {
    for w in [62u16, 48, 40] {
        let (_d, _s, app) = setup();
        let screen = render(&app, w, 70);
        assert_eq!(app.last_shape.get(), Shape::ThirdV, "{w}");
        let (hash, end) = tidy_geometry(&screen);
        assert!(hash.len() >= 4, "{w}: rows:\n{screen}");
        assert!(hash.windows(2).all(|p| p[0] == p[1]), "{w}: #number column aligned {hash:?}:\n{screen}");
        assert!(end.windows(2).all(|p| p[0] == p[1]), "{w}: status right-aligned {end:?}:\n{screen}");
        let issues = screen.lines().find(|l| l.contains("ISSUES ")).unwrap();
        let prs = screen.lines().find(|l| l.contains("PRS  ")).unwrap();
        assert_eq!(issues.find("ISSUES"), prs.find("PRS"), "{w}: labels aligned");
        if w >= 58 {
            assert!(issues.contains("MERGED") && prs.contains("MAIN CI"), "{w}: two columns:\n{screen}");
        } else {
            assert!(!issues.contains("MERGED") && screen.contains("MERGED"), "{w}: own rows:\n{screen}");
        }
        assert!(screen.lines().any(|l| l.contains("───") && l.contains("│ ─")), "{w}: divider:\n{screen}");
        assert!(screen.contains("CI FAIL") && screen.contains("FAIL") && screen.contains("free") && screen.contains("board"), "{w}:\n{screen}");
    }
}

#[test]
fn half_v_grid_boxes_and_tiles() {
    for (w, h) in [(70u16, 70u16), (90, 60)] {
        let (_d, _s, app) = setup();
        let screen = render(&app, w, h);
        assert_eq!(app.last_shape.get(), Shape::HalfV);
        let (todo, doing) = (row_of(&screen, "o TODO"), row_of(&screen, "o DOING"));
        let (review, done) = (row_of(&screen, "o REVIEW"), row_of(&screen, "o DONE"));
        assert_eq!(todo, doing, "{w}x{h}: TODO | DOING on one row");
        assert_eq!(review, done, "{w}x{h}: REVIEW | DONE on one row");
        assert!(review > todo && col_of(&screen, "o TODO") == col_of(&screen, "o REVIEW"), "{w}x{h}: 2x2");
        assert!(screen.contains("│ #7") || screen.contains("┃ #7") || screen.contains("┌ #7"), "{w}x{h}: boxed cards:\n{screen}");
        assert!(screen.contains("┌ ISSUES") && screen.contains("┌ PRS") && screen.contains("┌ MAIN CI"), "{w}x{h}: 2x2 tiles:\n{screen}");
        assert!(screen.contains("GH#      TITLE") || screen.contains("GH#     TITLE"), "{w}x{h}: tables:\n{screen}");
        assert!(row_of(&screen, "GITHUB") > review && row_of(&screen, "AGENTS") > row_of(&screen, "GITHUB"));
    }
}

#[test]
fn l_cycles_the_six_views() {
    let (_d, mut s, mut app) = setup();
    let mut seen = Vec::new();
    for _ in 0..6 {
        app.handle_key(key(KeyCode::Char('L')), &mut s);
        seen.push(s.layout().unwrap());
    }
    assert_eq!(seen, ["focus", "third-h", "third-v", "half-h", "half-v", "auto"]);
    assert!(render(&app, 160, 50).contains("view: auto"), "footer names the view");
    // old names still work
    s.set_layout("strip").unwrap();
    assert_eq!(s.layout().unwrap(), "third-h");
}

#[test]
fn half_v_spatial_arrows() {
    let (_d, mut s, mut app) = setup();
    render(&app, 70, 70);
    app.handle_key(key(KeyCode::Right), &mut s);
    assert_eq!(app.col, 1, "-> TODO to DOING");
    app.handle_key(key(KeyCode::Left), &mut s);
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down), &mut s);
    }
    assert_eq!((app.col, app.row[2]), (2, 0), "down off TODO's last card: REVIEW");
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down), &mut s);
    }
    assert_eq!(app.focus, Focus::Github, "down off REVIEW: GITHUB");
    // from DONE too
    let (_d, mut s, mut app) = setup();
    render(&app, 70, 70);
    app.col = 3;
    app.row[3] = 2;
    app.handle_key(key(KeyCode::Down), &mut s);
    assert_eq!(app.focus, Focus::Github, "down off DONE: GITHUB");
    // up from REVIEW's first card: TODO
    let (_d, mut s, mut app) = setup();
    render(&app, 70, 70);
    app.col = 2;
    app.handle_key(key(KeyCode::Up), &mut s);
    assert_eq!(app.col, 0);
}

/// Real-sized board (like a live check): 13 cards (6 todo, 3 doing, 1 review, 3 done),
/// 1 PR + 12 issues with long titles, 6 agents.
fn setup_live() -> (tempfile::TempDir, Store, App) {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(5).unwrap();
    let cards = [
        ("widgets: gh#501 signup form loses the draft", "todo", None),
        ("widgets: gh#502 export skips archived rows", "todo", None),
        ("widgets: gh#503 search ignores accents", "todo", None),
        ("widgets: gh#504 paging breaks on page 10", "todo", None),
        ("docs: guide for the new flags", "todo", None),
        ("ops: rotate API tokens", "todo", None),
        ("widgets: gh#510 totals rounded twice", "doing", Some("bot-1")),
        ("widgets: gh#511 upload retries forever", "doing", Some("bot-2")),
        ("widgets: gh#512 slow dashboard query", "doing", Some("bot-3")),
        ("widgets: gh#513 timezone on invoices", "review", Some("reviewer")),
        ("widgets: gh#514 dark mode contrast", "done", Some("bot-4")),
        ("widgets: gh#515 broken footer links", "done", Some("bot-3")),
        ("docs: changelog for 1.2", "done", Some("alice")),
    ];
    for (t, col, owner) in cards {
        let id = s.add(t, "", &[], "alice").unwrap();
        if col != "todo" {
            s.move_to(id, col, owner.unwrap_or("alice")).unwrap();
        }
    }
    s.set_github(Some("acme/widgets")).unwrap();
    let titles = [
        (501, "signup form loses the draft on reload"),
        (502, "export skips rows that were archived"),
        (503, "search ignores accents in customer names"),
        (504, "paging breaks after the tenth page"),
        (510, "invoice totals are rounded twice"),
        (511, "failed uploads are retried forever"),
        (512, "dashboard query takes ten seconds"),
        (513, "invoices show the server timezone"),
        (516, "password reset mail arrives twice"),
        (517, "long names overflow the sidebar"),
        (518, "tooltips stay open after a click"),
        (519, "csv import rejects quoted commas"),
    ];
    s.save_github(&Ok(GhSnapshot {
        repo: "acme/widgets".into(),
        fetched_at: terminal_board::store::now(),
        issues_open: 12,
        prs: vec![pr(520, "round invoice totals once at the end", "run")],
        issues: titles.iter().map(|(n, t)| issue(*n, t)).collect(),
        merged_today: vec![],
        main_ci: Some(MainCi { state: "ok".into(), workflow: "ci".into(), created_at: ago(40 * 60) }),
    }))
    .unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(&s);
    app.agents = AgentsState::Agents(parse_agents(AGENTS, None).unwrap());
    (dir, s, app)
}

/// Titles of the GitHub rows in a render (text after `#5NN` up to the next double space).
fn gh_row_titles(screen: &str) -> Vec<String> {
    let mut out = Vec::new();
    for l in screen.lines() {
        for n in [501, 502, 503, 504, 510, 511, 512, 513, 516, 517, 518, 519, 520] {
            let pat = format!("gh#{n} ");
            let Some(i) = l.find(&pat) else { continue };
            // a card line (`#7 gh#510 …`) or a PR reference in a status is not a row title
            if l[..i].ends_with("PR ") || l[..i].ends_with("-> ") {
                continue;
            }
            // a card box renders `#7 gh#510 …` (meta follows): the gh# is not a row title
            let before = l[..i].trim_end();
            if before.len() >= 2
                && (before.as_bytes()[before.len() - 1].is_ascii_digit() && before.contains('#')
                    || before.ends_with('#'))
            {
                continue;
            }
            // a card meta line (`gh#510 · bot-1`) or a compact gh ref (`gh#501 · 0m`)
            // is not a row title either: row titles sit between │/┃ bars with 2+ spaces
            // before the next column; meta lines are inside ┃┃ boxes with a `·` after.
            if l[i + pat.len()..].trim_start().starts_with('·') {
                continue;
            }
            let rest = l[i + pat.len()..].trim_start();
            let title: String = rest.split("  ").next().unwrap_or("").trim_end_matches(['│', '┃', ' ']).to_string();
            out.push(title);
        }
    }
    out
}

#[test]
fn live_sized_board_keeps_one_card_style_and_readable_github() {
    for (w, h) in [(63u16, 73u16), (45, 70), (126, 22), (126, 41), (50, 14)] {
        let (_d, _s, app) = setup_live();
        let screen = render(&app, w, h);
        let styles = app.drawn_styles.borrow().clone();
        assert!(!styles.iter().any(|(_, s)| *s == "compact"), "{w}x{h}: unboxed section {styles:?}\n{screen}");
        let kinds: std::collections::HashSet<_> = styles.iter().map(|(_, s)| *s).collect();
        assert!(kinds.len() <= 1, "{w}x{h}: mixed card styles {styles:?}\n{screen}");
        // wide tables keep a >= 30-char TITLE column, else the tidy rows are used
        for l in screen.lines().filter(|l| l.contains(" TITLE ")) {
            let at = l.find("TITLE").unwrap();
            let next = l[at + 5..].find(|c: char| c.is_ascii_uppercase()).unwrap() + 5;
            assert!(next > 30, "{w}x{h}: TITLE column {next} wide:\n{screen}");
        }
        for t in gh_row_titles(&screen) {
            assert!(t.chars().count() >= 12, "{w}x{h}: GitHub title {t:?} under 12 chars\n{screen}");
        }
    }
}

#[test]
fn half_v_grid_sized_to_its_cards() {
    let (_d, _s, app) = setup_live();
    let screen = render(&app, 63, 73);
    let lines: Vec<&str> = screen.lines().collect();
    let gh = row_of(&screen, "GITHUB");
    let ag = row_of(&screen, "AGENTS");
    assert!(ag - gh >= 14, "GITHUB got {} rows:\n{screen}", ag - gh);
    assert!(lines.len() - ag >= 4, "AGENTS squeezed:\n{screen}");
    // each grid row is as tall as its fuller cell: REVIEW|DONE = frame + DONE's 3 boxes
    let review = row_of(&screen, "REVIEW");
    let todo = row_of(&screen, "TODO (6)");
    assert_eq!(gh - review, 2 + 3 * 4, "REVIEW/DONE row padded:\n{screen}");
    assert_eq!(review - todo, 2 + 6 * 4, "TODO/DOING row:\n{screen}");
    // every issue row of the fixture is visible (12 issues + 1 PR)
    assert_eq!(gh_row_titles(&screen).len(), 13, "\n{screen}");
}

#[test]
fn third_v_caps_github_until_every_section_shows_two_boxes() {
    let (_d, _s, app) = setup_live();
    let screen = render(&app, 45, 70);
    let styles = app.drawn_styles.borrow().clone();
    assert_eq!(styles.len(), 4, "every section boxed: {styles:?}\n{screen}");
    // at least 2 boxed cards per section (or all of them): count card ids per column rect
    let rects = app.col_rects.get();
    let lines: Vec<&str> = screen.lines().collect();
    for (ci, want) in [(0usize, 2usize), (1, 2), (2, 1), (3, 2)] {
        let r = rects[ci];
        let n = lines[r.y as usize..(r.y + r.height) as usize]
            .iter()
            .filter(|l| l.contains("┌ #") || l.contains("┏ #") || l.contains("│ #") || l.contains("┃ #"))
            .count();
        assert!(n >= want, "column {ci}: {n} boxed cards\n{screen}");
    }
}

#[test]
fn github_wide_tables_drop_columns_before_the_title() {
    let (_d, _s, mut app) = setup_live();
    app.view = View::Github;
    // wide: every column
    let screen = render(&app, 160, 40);
    assert!(screen.contains("LABELS") && screen.contains("BRANCH"), "\n{screen}");
    // half-v at 63: labels, branch (then age, who) go first, title keeps >= 30
    app.view = View::Board;
    let screen = render(&app, 63, 73);
    assert!(!screen.contains("LABELS") && !screen.contains("BRANCH"), "\n{screen}");
    assert!(screen.contains(" TITLE "), "\n{screen}");
    // too narrow for a 30-char title even with every optional column gone: tidy rows
    app.snap.layout = "half-v".into();
    let screen = render(&app, 50, 70);
    assert!(!screen.contains(" TITLE ") && screen.contains("ISSUE gh#501"), "\n{screen}");
}

/// `cargo test --test layout -- --ignored --nocapture live_samples` prints the round-26 renders.
#[test]
#[ignore]
fn live_samples() {
    let (_d, _s, app) = setup_live();
    for (w, h, from, to) in [(63u16, 73u16, 30usize, 73usize), (45, 70, 1, 40)] {
        println!("===== {w}x{h} lines {from}-{to} =====");
        for l in render(&app, w, h).lines().skip(from - 1).take(to - from + 1) {
            println!("{}", l.trim_end());
        }
    }
}

#[test]
fn thirdv_spare_rows_go_to_github_not_to_a_gap() {
    // the issue's repro: 8 cards, 52x66 — the old split left ~5 blank rows between
    // DONE and GITHUB
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(9).unwrap();
    s.set_github(Some("acme/widgets")).unwrap();
    s.save_github(&Ok(GhSnapshot {
        repo: "acme/widgets".into(),
        fetched_at: terminal_board::store::now(),
        issues_open: 3,
        issues: vec![issue(305, "login form rejects"), issue(307, "search index lags"), issue(301, "csv export drops the header row")],
        prs: vec![pr(306, "fix login form validation", "FAIL"), pr(308, "speed up search indexing", "ok")],
        ..Default::default()
    }))
    .unwrap();
    for i in 0..8 {
        let id = s.add(&format!("widgets: card number {i} to fill the board"), "", &[], "alice").unwrap();
        if i >= 3 {
            s.take(id, "bot-1").unwrap();
        }
        if i >= 6 {
            s.move_to(id, "review", "bot-1").unwrap();
        }
    }
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    app.reload(&s);
    let screen = render(&app, 52, 66);
    // every row between the top and the footer belongs to a section: no fully blank row
    // between DONE's last box and the GITHUB panel
    let blank_run = screen
        .lines()
        .filter(|l| !l.contains("┌") && !l.contains("│") && !l.contains("┃") && !l.contains("┗") && !l.contains("┛") && l.trim().is_empty())
        .count();
    assert!(blank_run == 0, "{blank_run} fully blank rows in the body:\n{screen}");
    // the GITHUB panel actually grew into the spare space
    assert!(screen.contains("GITHUB"), "{screen}");
    // negative control: at 52x56 (the no-gap size from the issue) everything still renders
    let screen56 = render(&app, 52, 56);
    assert!(screen56.contains("GITHUB") && screen56.contains("DONE"), "{screen56}");
}

#[test]
fn first_run_empty_states_show_hints() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    // a fresh board, GitHub connected to a quiet repo
    s.set_github(Some("acme/widgets")).unwrap();
    s.save_github(&Ok(GhSnapshot {
        repo: "acme/widgets".into(),
        fetched_at: terminal_board::store::now(),
        issues_open: 0,
        ..Default::default()
    }))
    .unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    app.reload(&s);
    let frame = |c: char| "─│┌┐└┘┏┓┗┛━┃┃".contains(c);
    // every shape: the TODO hint is readable whole (wrapped, never cut mid-word), and the
    // quiet repo says so wherever the GitHub panel has room for its list
    for (w, h, shape) in [(50u16, 14u16, "focus"), (110, 22, "third-h"), (110, 45, "third-v"), (140, 45, "half-h"), (90, 45, "half-v")] {
        let screen = render(&app, w, h);
        let text: String = screen.chars().map(|c| if frame(c) { ' ' } else { c }).collect();
        let words: Vec<&str> = text.split_whitespace().collect();
        if shape == "focus" {
            // the focus view has its own one-line empty state
            assert!(screen.contains("press a to add a card"), "{shape}:\n{screen}");
        } else {
            for word in terminal_board::tui::FIRST_CARD_HINT.split(' ') {
                assert!(words.contains(&word), "{shape}: hint word '{word}' cut or missing:\n{screen}");
            }
            let at = words.iter().position(|w| *w == "press").expect(shape);
            assert_eq!(&words[at..at + 4], ["press", "a", "to", "add"], "{shape}: hint starts whole:\n{screen}");
            assert!(screen.contains("no open issues or PRs") || screen.contains("no open PRs or issues"), "{shape}: quiet-repo hint:\n{screen}");
            assert!(screen.contains("MAIN"), "{shape}: main CI stays visible:\n{screen}");
        }
    }
    // third-h: the rail keeps its stats rows (a quiet repo can still have a failing main CI)
    let screen = render(&app, 110, 22);
    for row in ["ISSUES", "PRS", "MAIN CI", "no open PRs or issues"] {
        assert!(screen.contains(row), "third-h rail keeps '{row}':\n{screen}");
    }
}
