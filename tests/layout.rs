//! Layouts from half a screen down to a third of it (TestBackend), with GitHub configured,
//! 6 agents and 12 cards: FULL (unchanged), RAIL (wide-short), STACK (tall-narrow), bars,
//! Tab views, TINY, and a size sweep.
mod common;
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
            if col == "done" { s.move_to_forced(id, col, owner.unwrap_or("alice")) } else { s.move_to(id, col, owner.unwrap_or("alice")) }.unwrap(); // fixture only: nothing reaches done except from review (verifier rule), so a card seeded straight into done is a forced move
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
    common::pin_clock();
    for (w, h, shape) in [
        (40, 12, Shape::Focus),
        (50, 14, Shape::Focus),
        (30, 10, Shape::Focus),
        (126, 22, Shape::ThirdH),
        (200, 24, Shape::ThirdH),
        (260, 26, Shape::ThirdH),
        // a tall pane keeps the 2x2 grid down to 48 columns (two 24-cell columns: a card still
        // reads), and stacks below that — it stacked from 62 columns down before, which put a
        // 60x30 pane in four one-card sections
        (47, 70, Shape::ThirdV),
        (42, 73, Shape::ThirdV),
        (48, 70, Shape::HalfV),
        (50, 70, Shape::HalfV),
        (60, 73, Shape::HalfV),
        (60, 30, Shape::HalfV),
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
    common::pin_clock();
    let (_d, _s, app) = setup();
    let screen = render(&app, 126, 41);
    // boxed 4-row cards: the title is inside the box, under a plain top border
    let r = row_of(&screen, "#1 write install guide");
    let above = screen.lines().nth(r - 1).unwrap();
    assert!(above.contains("┏━━") || above.contains("┌──"), "4-row box:\n{screen}");
    // (`lead` holds nothing on this board: it is counted in `+1 elsewhere`, not named)
    for want in ["MAIN CI  ", "BRANCH / ISSUE", "LABELS", "┌ AGENTS", "bot-4", "+1 elsewhere", "o TODO (3)", "o DOING (3/5)"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
}

#[test]
fn third_height_rail_126x24_and_200x24() {
    common::pin_clock();
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
    common::pin_clock();
    // the stacked view itself, at the widths it used to be picked for (60 and 50 are auto a
    // 2x2 grid now — `auto_shape_by_size` — so the view is pinned here)
    for (w, h) in [(42u16, 73u16), (60, 73), (50, 70)] {
        let (_d, _s, mut app) = setup();
        app.snap.layout = "third-v".into();
        let screen = render(&app, w, h);
        let order: Vec<usize> =
            ["o TODO (3)", "o DOING (3/5)", "o REVIEW (3)", "o DONE today (3)", "GITHUB", "AGENTS"].iter().map(|n| row_of(&screen, n)).collect();
        assert!(order.windows(2).all(|p| p[0] < p[1]), "{w}x{h}: stacked top to bottom {order:?}:\n{screen}");
        assert!(screen.contains("┌ #") || screen.contains("┏ #") || screen.contains("┃┌──"), "{w}x{h}: boxed cards:\n{screen}");
        let gh_rows = count(&screen, "PR    gh#3") + count(&screen, "ISSUE gh#3");
        assert!(gh_rows >= 3, "{w}x{h}: >= 3 github rows:\n{screen}");
        // everyone on this board, then the one pane that is not
        for a in ["bot-1", "bot-2", "bot-3", "bot-4", "reviewer", "+1 elsewhere"] {
            assert!(screen.contains(a), "{w}x{h}: agent {a}:\n{screen}");
        }
    }
}

#[test]
fn stack_github_rows_are_focusable() {
    common::pin_clock();
    let (_d, mut s, mut app) = setup();
    // the stacked view (60x73 is auto a 2x2 grid now)
    s.set_layout("third-v").unwrap();
    app.reload(&s);
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
    common::pin_clock();
    let (_d, _s, app) = setup();
    let screen = render(&app, 95, 35);
    assert!(screen.contains(" GITHUB acme/widgets · 10 issues") && screen.contains("tab >"), "{screen}");
    assert!(screen.contains(" AGENTS 6 here · 1 elsewhere"), "{screen}");
}

/// An idle agent holding a card is a problem, so its warning is never the part that gets cut:
/// the stacked and rail AGENTS panels keep `idle w/ card (<how long>)` whole at the narrow sizes
/// where the row has no room left for the activity text, and the AGENTS bar says the words
/// before the duration.
#[test]
fn idle_holder_warning_stays_whole_at_narrow_sizes() {
    for (w, h) in [(52u16, 56u16), (52, 60), (52, 80), (54, 56), (140, 16), (140, 20), (140, 28), (144, 16)] {
        let (_d, _s, app) = setup();
        let screen = render(&app, w, h);
        let row = screen.lines().find(|l| l.contains("! bot-2")).unwrap_or_else(|| panic!("{w}x{h}: no idle holder row:\n{screen}"));
        assert!(row.contains("#5 "), "{w}x{h}: the held card: {row}");
        let at = row.find(" · idle w/ card (").unwrap_or_else(|| panic!("{w}x{h}: the warning and its duration are cut: {row}"));
        assert!(row[at..].contains("m)"), "{w}x{h}: the duration is whole: {row}");
    }
    let (_d, _s, app) = setup();
    let bar = render(&app, 95, 35);
    assert!(bar.contains("(! bot-2 idle w/ card (0m))"), "the bar: words first, then the duration:\n{bar}");
    // a narrow bar drops the duration before the words, and never shows half of it
    for w in [48u16, 50, 52, 54] {
        let narrow = render(&app, w, 12);
        let row = narrow.lines().find(|l| l.contains("(! bot-2")).unwrap_or_else(|| panic!("{w}x12: no bar:\n{narrow}"));
        let at = row.find("(! bot-2 idle w/ card").unwrap_or_else(|| panic!("{w}x12: the words are cut: {row}"));
        let after = &row[at + "(! bot-2 idle w/ card".len()..];
        assert!(after.starts_with(" (0m))") || !after.contains('('), "{w}x12: a cut duration: {row}");
    }
    // the full panel shows the duration whole or not at all
    for w in [100u16, 102, 104, 110, 126] {
        let screen = render(&app, w, 35);
        let row = screen.lines().find(|l| l.contains("! idle, holds card")).unwrap_or_else(|| panic!("{w}x35: no holder row:\n{screen}"));
        let after = &row[row.find("! idle, holds card").unwrap() + "! idle, holds card".len()..];
        assert!(after.starts_with(" (0m)") || !after.contains('('), "{w}x35: a cut duration: {row}");
    }
    let wide = render(&app, 126, 41);
    assert!(wide.contains("! idle, holds card (0m)"), "126x41: the duration shows:\n{wide}");
}

/// Push every event of a card (and its clocks) back in time.
fn backdate(dir: &tempfile::TempDir, card: i64, secs: i64) {
    let c = rusqlite::Connection::open(dir.path().join("b.db")).unwrap();
    c.execute("UPDATE events SET ts = ts - ?1 WHERE card_id = ?2", rusqlite::params![secs, card]).unwrap();
    c.execute("UPDATE cards SET column_since = column_since - ?1, created_at = created_at - ?1 WHERE id = ?2", rusqlite::params![secs, card])
        .unwrap();
}

/// The 12-card board with quiet work on it: 1 = an idle holder quiet for 1h20m and a working
/// agent quiet for 2h, with notes; 2 = no notes, 2d and 5h; 3 = two idle holders, 1h20m and 3d.
fn quiet_board(n: usize) -> (tempfile::TempDir, Store, App) {
    let (dir, mut s, mut app) = setup();
    match n {
        1 => {
            s.note(5, "retrying the upload with a smaller chunk size", "bot-2").unwrap();
            s.note(4, "plus-addresses parse now; writing the regression test", "bot-1").unwrap();
            s.note(6, "took 3 of 5 screenshots", "bot-3").unwrap();
            backdate(&dir, 5, 80 * 60 + 20);
            backdate(&dir, 4, 2 * 3600 + 20);
        }
        2 => {
            backdate(&dir, 5, 51 * 3600 + 20);
            backdate(&dir, 6, 5 * 3600 + 20);
        }
        _ => {
            let id = s.add("ops: a very long title for the second idle holder card to test fitting", "", &[], "alice").unwrap();
            s.move_to(id, "doing", "lead").unwrap();
            s.note(5, "short note", "bot-2").unwrap();
            backdate(&dir, 5, 80 * 60 + 20);
            backdate(&dir, id, 72 * 3600 + 20);
        }
    }
    app.reload(&s);
    (dir, s, app)
}

/// Every `quiet` on the screen is whole: followed by a whole duration or by nothing, and a
/// card's meta line never ends in a piece of the word.
fn assert_quiet_is_whole(ctx: &str, screen: &str) {
    const AGES: [&str; 5] = ["1h20m", "2h", "2d", "5h", "3d"];
    const OWNERS: [&str; 4] = ["bot-1", "bot-2", "bot-3", "lead"];
    for line in screen.lines() {
        for seg in line.split(['│', '┃']) {
            let toks: Vec<&str> = seg.split_whitespace().collect();
            for (i, t) in toks.iter().enumerate() {
                if *t == "quiet" {
                    let next = toks.get(i + 1);
                    assert!(next.is_none_or(|a| AGES.contains(a)), "{ctx}: a cut duration after 'quiet': {seg:?}\n{screen}");
                }
            }
            let last = toks.last().copied().unwrap_or("");
            let cut = !last.is_empty() && last.len() < 5 && "quiet".starts_with(last);
            assert!(!(cut && OWNERS.iter().any(|o| toks.contains(o))), "{ctx}: a cut 'quiet': {seg:?}\n{screen}");
        }
    }
}

/// The AGENTS bar shows an idle holder's duration only with room for all of it: the closing
/// `)` and the `tab >` hint are never cut to make space for it.
fn assert_bar_keeps_its_hint(ctx: &str, screen: &str) {
    for line in screen.lines().filter(|l| l.trim_start().starts_with("AGENTS ") && l.contains("idle w/ card (")) {
        assert!(line.trim_end().ends_with("))   tab >"), "{ctx}: the duration cut the hint: {line:?}");
    }
}

/// Quiet work at every size: the `quiet` marker and the idle holder's duration are shown
/// whole, in a shorter whole form, or not at all - never cut mid-token - and they never cost
/// the AGENTS bar its `tab >` hint.
#[test]
fn quiet_work_is_never_cut_at_any_size() {
    const HEIGHTS: [u16; 13] = [10, 12, 14, 16, 20, 24, 28, 35, 41, 45, 56, 60, 80];
    for n in 1..=3 {
        let (_d, s, mut app) = quiet_board(n);
        for w in (30u16..=200).step_by(2) {
            for h in HEIGHTS {
                let screen = render(&app, w, h);
                assert_quiet_is_whole(&format!("board {n} {w}x{h}"), &screen);
                assert_bar_keeps_its_hint(&format!("board {n} {w}x{h}"), &screen);
            }
        }
        // every layout preference, and the focus view
        for pref in LAYOUTS {
            s.set_layout(pref).unwrap();
            app.reload(&s);
            app.agents = AgentsState::Agents(parse_agents(AGENTS, None).unwrap());
            for w in (30u16..=200).step_by(10) {
                for h in [16u16, 28, 45, 73] {
                    let screen = render(&app, w, h);
                    assert_quiet_is_whole(&format!("board {n} {pref} {w}x{h}"), &screen);
                    assert_bar_keeps_its_hint(&format!("board {n} {pref} {w}x{h}"), &screen);
                }
            }
        }
    }
    // with room, both are there in full
    let (_d, _s, app) = quiet_board(1);
    let wide = render(&app, 160, 45);
    assert!(wide.contains("quiet 1h20m") && wide.contains("quiet 2h"), "160x45: the quiet markers:\n{wide}");
    let bar = render(&app, 70, 12);
    assert!(bar.contains("(! bot-2 idle w/ card (1h20m))   tab >"), "70x12: duration and hint:\n{bar}");
    // 56..64 wide: no room for the duration next to the hint, so the hint wins
    for w in [58u16, 60, 62, 64] {
        let screen = render(&app, w, 12);
        let row = screen.lines().find(|l| l.contains("(! bot-2")).unwrap_or_else(|| panic!("{w}x12: no bar:\n{screen}"));
        assert!(row.trim_end().ends_with("(! bot-2 idle w/ card)   tab >"), "{w}x12: the hint is whole: {row:?}");
    }
    // a narrow card keeps the owner and the whole marker, and gives up the tag first
    let narrow = render(&app, 74, 41);
    assert!(narrow.contains("bot-2 - 1h20m quiet 1h20m") || narrow.contains("bot-2 quiet 1h20m"), "74x41: the marker is whole:\n{narrow}");
}

#[test]
fn tiny_tab_pages_through_the_panels() {
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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

    // STACK (60x73, pinned: auto is a 2x2 grid there now): <-/-> jump between sections,
    // -> from the last does nothing
    let (_d, mut s, mut app) = setup();
    s.set_layout("third-v").unwrap();
    app.reload(&s);
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
    common::pin_clock();
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
    common::pin_clock();
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
        s.move_to_forced(id, "done", "alice").unwrap(); // fixture only: nothing reaches done except from review (verifier rule), so a card seeded straight into done is a forced move
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
    common::pin_clock();
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
    common::pin_clock();
    for w in [62u16, 48, 40] {
        let (_d, _s, mut app) = setup();
        // the stacked view's GitHub block (62 and 48 are auto a 2x2 grid now, so it is pinned)
        app.snap.layout = "third-v".into();
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
    common::pin_clock();
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
    common::pin_clock();
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
    common::pin_clock();
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
            if col == "done" { s.move_to_forced(id, col, owner.unwrap_or("alice")) } else { s.move_to(id, col, owner.unwrap_or("alice")) }.unwrap(); // fixture only: nothing reaches done except from review (verifier rule), so a card seeded straight into done is a forced move
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
    common::pin_clock();
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

/// Was `half_v_grid_sized_to_its_cards`, which pinned each grid row to its OWN fuller cell
/// (TODO/DOING 26 rows, REVIEW/DONE 14). The rows now split evenly whatever they hold, so
/// both are as tall as the grid's fullest cell (TODO's six 4-row boxes) — and the panels
/// keep their floors out of what is left.
#[test]
fn half_v_grid_rows_are_even_and_fit_the_fullest_cell() {
    common::pin_clock();
    let (_d, _s, app) = setup_live();
    let screen = render(&app, 63, 73);
    let lines: Vec<&str> = screen.lines().collect();
    let gh = row_of(&screen, "GITHUB");
    let ag = row_of(&screen, "AGENTS");
    assert!(ag - gh >= 14, "GITHUB got {} rows:\n{screen}", ag - gh);
    assert!(lines.len() - ag >= 4, "AGENTS squeezed:\n{screen}");
    // both grid rows the same height: frame + TODO's 6 boxes, the fullest cell
    let review = row_of(&screen, "REVIEW");
    let todo = row_of(&screen, "TODO (6)");
    assert_eq!(review - todo, 2 + 6 * 4, "TODO/DOING row:\n{screen}");
    assert_eq!(gh - review, review - todo, "REVIEW/DONE row as tall as TODO/DOING:\n{screen}");
    // every issue row of the fixture (12 issues + 1 PR) is either drawn or counted
    let drawn = gh_row_titles(&screen).len();
    let more: usize = screen
        .lines()
        .find_map(|l| l.split("+").nth(1).and_then(|r| r.split(' ').next()).and_then(|n| n.parse().ok()).filter(|_| l.contains("more issues")))
        .unwrap_or(0);
    assert_eq!(drawn + more, 13, "{drawn} rows drawn + {more} counted:\n{screen}");
}

#[test]
fn third_v_caps_github_until_every_section_shows_two_boxes() {
    common::pin_clock();
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
    common::pin_clock();
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
    // too narrow for a 30-char title even with every optional column gone: tidy rows. (Any
    // tidy issue row will do: the grid's rows split evenly now, so the grid is twice its
    // fullest cell and the panel shows fewer rows here — the oldest, gh#501, is below its
    // `+N more`.)
    app.snap.layout = "half-v".into();
    let screen = render(&app, 50, 70);
    assert!(!screen.contains(" TITLE ") && screen.contains("ISSUE gh#5"), "\n{screen}");
}

/// `cargo test --test layout -- --ignored --nocapture live_samples` prints the round-26 renders.
#[test]
#[ignore]
fn live_samples() {
    common::pin_clock();
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
    common::pin_clock();
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
    // every view, at a size that auto really maps to it (110x45 is half-h with a column narrow
    // enough to wrap the hint): the TODO hint is readable whole (wrapped, never cut mid-word),
    // and the quiet repo says so wherever the GitHub panel has room for its list
    use terminal_board::tui::{pick_shape, Shape};
    for (w, h, want, shape) in [
        (50u16, 14u16, Shape::Focus, "focus"),
        (110, 22, Shape::ThirdH, "third-h"),
        (46, 60, Shape::ThirdV, "third-v"),
        (110, 45, Shape::HalfH, "half-h, hint wrapped"),
        (140, 45, Shape::HalfH, "half-h"),
        (90, 45, Shape::HalfV, "half-v"),
    ] {
        assert_eq!(pick_shape("auto", w, h), want, "{w}x{h} is the {shape} view");
        let screen = render(&app, w, h);
        let text: String = screen.chars().map(|c| if frame(c) { ' ' } else { c }).collect();
        let words: Vec<&str> = text.split_whitespace().collect();
        if want == Shape::Focus {
            // the focus view has its own one-line empty state
            assert!(screen.contains("press a to add a card"), "{shape}:\n{screen}");
            continue;
        }
        // third-v used to fold an empty section into its header line, so it had no TODO box
        // and no hint. Its sections now split the height evenly whatever they hold, so an
        // empty TODO gets a box like every other view — and owes the same whole hint.
        //
        // the expected text is spelled out here on purpose: the test pins what the user
        // reads, not a constant from the code under test
        for word in "press a to add your first card".split(' ') {
            assert!(words.contains(&word), "{shape}: hint word '{word}' cut or missing:\n{screen}");
        }
        let at = words.iter().position(|w| *w == "press").expect(shape);
        assert_eq!(&words[at..at + 4], ["press", "a", "to", "add"], "{shape}: hint starts whole:\n{screen}");
        assert!(screen.contains("no open issues or PRs") || screen.contains("no open PRs or issues"), "{shape}: quiet-repo hint:\n{screen}");
        assert!(screen.contains("MAIN"), "{shape}: main CI stays visible:\n{screen}");
    }
    // third-h: the rail keeps its stats rows (a quiet repo can still have a failing main CI)
    let screen = render(&app, 110, 22);
    for row in ["ISSUES", "PRS", "MAIN CI", "no open PRs or issues"] {
        assert!(screen.contains(row), "third-h rail keeps '{row}':\n{screen}");
    }
}

/// The words inside the TODO column's box (None when the view draws no box for it). TODO is
/// the first box on the screen; a narrow column cuts its title, so any start of it counts.
fn todo_box_words(screen: &str) -> Option<Vec<String>> {
    let rows: Vec<Vec<char>> = screen.lines().map(|l| l.chars().collect()).collect();
    let (r0, c0) = rows.iter().enumerate().find_map(|(r, row)| row.iter().position(|c| "┏┌".contains(*c)).map(|c| (r, c)))?;
    let c1 = (c0 + 1..rows[r0].len()).find(|&c| "┓┐".contains(rows[r0][c]))?;
    let title: String = rows[r0][c0 + 1..c1].iter().filter(|c| !"━─".contains(**c)).collect();
    if !title.contains('o') || !" o TODO (0)".starts_with(title.trim_end()) {
        return None;
    }
    let inner: Vec<String> = rows[r0 + 1..].iter().take_while(|row| "┃│".contains(row[c0])).map(|row| row[c0 + 1..c1].iter().collect()).collect();
    Some(inner.join(" ").split_whitespace().map(str::to_string).collect())
}

#[test]
fn first_card_hint_is_never_cut_in_any_forced_layout() {
    // a fresh board in every layout preference, swept over widths: the TODO box holds the
    // whole hint, the whole short form, or the plain '-' — never a split or ellipsized word
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    app.reload(&s);
    let long: Vec<&str> = "press a to add your first card".split(' ').collect();
    let short: Vec<&str> = "a: add a card".split(' ').collect();
    let (mut saw_long, mut saw_short, mut saw_dash) = (0, 0, 0);
    for pref in terminal_board::store::LAYOUTS {
        app.snap.layout = pref.into();
        for w in 20u16..=200 {
            for h in [6u16, 8, 12, 16, 20, 30, 45, 60] {
                if terminal_board::tui::pick_shape(pref, w, h) == terminal_board::tui::Shape::Focus {
                    continue; // the focus view has its own one-line empty state
                }
                let screen = render(&app, w, h);
                let Some(words) = todo_box_words(&screen) else {
                    // only third-v (and auto when it picks third-v) draws no TODO box
                    assert_eq!(terminal_board::tui::pick_shape(pref, w, h), terminal_board::tui::Shape::ThirdV, "{pref} {w}x{h}: no TODO box:\n{screen}");
                    continue;
                };
                if words == long {
                    saw_long += 1;
                } else if words == short {
                    saw_short += 1;
                } else if words == ["-"] || words.is_empty() {
                    saw_dash += 1;
                } else {
                    panic!("{pref} {w}x{h}: the TODO box holds a cut hint {words:?}:\n{screen}");
                }
            }
        }
    }
    // the sweep met all three forms, so each branch above was really exercised
    assert!(saw_long > 0, "no size showed the whole hint");
    assert!(saw_short > 0, "no size showed the short form");
    assert!(saw_dash > 0, "no size fell back to the plain '-'");
    // the reported sizes: forced third-h at 64x20 and forced half-h at 58x8
    for (pref, w, h) in [("third-h", 64u16, 20u16), ("half-h", 58, 8), ("half-v", 28, 12)] {
        app.snap.layout = pref.into();
        let screen = render(&app, w, h);
        let words = todo_box_words(&screen).unwrap_or_else(|| panic!("{pref} {w}x{h}: no TODO box:\n{screen}"));
        assert!(words == long || words == short || words == ["-"], "{pref} {w}x{h}: {words:?}");
    }
}

/// A GitHub page of `np` PRs and `ni` issues from a repo with 60 open issues (20 = a full
/// page); `busy` makes one PR fail CI and one a draft. Returns the counts the tiles show.
fn setup_page(np: i64, ni: i64, busy: bool) -> (tempfile::TempDir, Store, App, terminal_board::github::Factory) {
    let (dir, s, mut app) = setup();
    let snap = GhSnapshot {
        repo: "acme/widgets".into(),
        fetched_at: terminal_board::store::now(),
        issues_open: if np == 0 && ni == 0 { 0 } else { 60 },
        prs: (0..np)
            .map(|k| Pr { is_draft: busy && k == 2, ..pr(400 + 2 * k, "speed up search indexing", if busy && k == 1 { "FAIL" } else { "ok" }) })
            .collect(),
        issues: (0..ni).map(|k| issue(301 + 2 * k, "csv export drops the header row")).collect(),
        merged_today: vec![],
        main_ci: Some(MainCi { state: "ok".into(), workflow: "ci".into(), created_at: ago(40 * 60) }),
    };
    let counts = terminal_board::github::factory(&snap, &s.list().unwrap(), terminal_board::store::now());
    s.save_github(&Ok(snap)).unwrap();
    app.reload(&s);
    (dir, s, app, counts)
}

/// The GitHub panel's head: its rows from the title down to the first PR / issue row.
fn gh_head(screen: &str) -> String {
    let from = screen.lines().position(|l| l.contains("GITHUB")).unwrap_or(0);
    screen.lines().skip(from + 1).take_while(|l| !l.contains("gh#")).collect::<Vec<_>>().join("\n")
}

/// Is `tok` in `text` as a whole token (no letter, digit or `#` on either side)?
fn whole(text: &str, tok: &str) -> bool {
    let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '#');
    text.match_indices(tok).any(|(i, _)| edge(text[..i].chars().next_back()) && edge(text[i + tok.len()..].chars().next()))
}

/// Issue #34: a full page (20 items) is labelled `newest` — and the label is the only thing
/// that gives way. Next to a 19-item page (same text lengths, no label) at every width:
/// no row moves; every count and note the 19-item page shows whole, the full page shows
/// whole (`(1 draft)`, `1 failing CI`, the unclaimed count, MERGED, MAIN …); and a label is
/// either whole or absent, never cut.
#[test]
fn full_page_labels_fit_at_small_sizes() {
    common::pin_clock();
    let rows = |screen: &str| -> Vec<usize> {
        screen.lines().enumerate().filter(|(_, l)| ["GITHUB", "GH# ", "AGENTS", "o TODO", "MERGED"].iter().any(|m| l.contains(m))).map(|(i, _)| i).collect()
    };
    let (mut long, mut terse, mut bare) = (0, 0, 0);
    for (np, ni, busy) in [(20, 20, false), (20, 20, true), (20, 5, true), (5, 20, true)] {
        let (_d1, s1, mut plain, c1) = setup_page(np.min(19), ni.min(19), busy);
        let (_d2, s2, mut full, c2) = setup_page(np, ni, busy);
        for layout in ["auto", "half-v"] {
            for (s, app) in [(&s1, &mut plain), (&s2, &mut full)] {
                s.set_layout(layout).unwrap();
                app.reload(s);
                app.agents = AgentsState::Agents(parse_agents(AGENTS, None).unwrap());
            }
            for h in [30u16, 41, 60] {
                for w in (60u16..=170).filter(|w| layout == "auto" || *w < 104) {
                    let (a, b) = (render(&plain, w, h), render(&full, w, h));
                    let at = format!("{np}x{ni} busy={busy} {layout} {w}x{h}");
                    assert_eq!(rows(&a), rows(&b), "{at}: the page label moves no row:\n{b}");
                    let (a, b) = (gh_head(&a), gh_head(&b));
                    // what the 19-item page shows whole -> what the full page must show whole
                    let mut kept: Vec<(String, String)> = ["(1 draft)", "1 failing CI", "(1 FAIL)", "60 open", "ISSUES", "PULL REQUESTS", "MERGED", "none yet", "MAIN", "ok", "40m"]
                        .iter()
                        .map(|t| (t.to_string(), t.to_string()))
                        .collect();
                    kept.push((format!("+{}", c1.new_today), format!("+{}", c2.new_today)));
                    kept.push((c1.unclaimed.to_string(), c2.unclaimed.to_string()));
                    for (was, is) in kept {
                        assert!(!whole(&a, &was) || whole(&b, &is), "{at}: `{is}` cut or lost where the plain page shows `{was}`:\n{a}\n--- full page:\n{b}");
                    }
                    // a label is whole or absent
                    for l in b.lines() {
                        assert!(!l.contains("newe") || whole(l, "newest"), "{at}: 'newest' cut: {l}");
                        assert!(!l.contains("newest 20:") || whole(l, "free"), "{at}: tile label cut: {l}");
                        assert!(!l.contains("/20") || whole(l, "free"), "{at}: terse label cut: {l}");
                        assert!(!l.contains("in newest") || whole(l, "in newest 20)"), "{at}: summary label cut: {l}");
                    }
                    long += usize::from(b.contains("newest"));
                    terse += usize::from(!b.contains("newest") && (b.contains("20+") || b.contains("/20 free")));
                    bare += usize::from(!b.contains("newest") && !b.contains("20+") && !b.contains("/20 free"));
                }
            }
        }
    }
    assert!(long > 800 && terse > 50, "the sweep saw long ({long}) and terse ({terse}) labels; {bare} screens had room for none");

    // the sizes a reviewer found cut, spelled out (20 PRs with a draft and a failing one)
    let (_d, s, mut full, _) = setup_page(20, 20, true);
    let wide = render(&full, 110, 41);
    assert!(wide.contains("PRS  20+ (1 draft) ") && whole(&wide, "free"), "terse PR label, draft note whole:\n{wide}");
    let half = render(&full, 126, 41);
    assert!(half.contains("PRS  20 newest (1 draft) ") && half.contains(" newest 20: +20 · 17 free "), "{half}");
    let roomy = render(&full, 160, 50);
    assert!(roomy.contains("PULL REQUESTS  20 newest (1 draft) "), "{roomy}");
    let line = render(&full, 126, 30);
    assert!(line.contains("17 unclaimed in newest 20) · PRS 20 newest (1 FAIL) · MERGED 0 · MAIN ok"), "{line}");
    let line = render(&full, 64, 30);
    assert!(line.contains(" ISSUES 60 (+20, 17 unclaimed) · PRS 20 (1 FAIL) · MERGED 0 · "), "no room: the line without labels:\n{line}");
    s.set_layout("half-v").unwrap();
    full.reload(&s);
    let line = render(&full, 80, 30);
    assert!(line.contains("17/20 free) · PRS 20+ (1 FAIL) · MERGED 0 · MAIN ok"), "terse labels in a narrow short panel:\n{line}");
    let grid = render(&full, 90, 60);
    assert!(grid.contains("60 open · newest 20: +20 · 17 free ") && grid.contains("20 newest (1 draft) · 1 failing CI "), "{grid}");
    let grid = render(&full, 76, 60);
    assert!(grid.contains("20+ (1 draft) · 1 failing CI ") && grid.contains("60 open · +20 today · 17/20 free "), "{grid}");
    let grid = render(&full, 66, 60);
    assert!(grid.contains("60 open · +20 today · 17 un…") && grid.contains("20 open (1 draft) · 1 fail…"), "no room: the counts stay, the label goes:\n{grid}");

    // a quiet repo is never a page: its empty state is the same whatever the label
    let (_d, s, mut quiet, _) = setup_page(0, 0, false);
    // every width, including those where a full page gets the long, terse or no label
    for (w, h) in [(160u16, 50u16), (126, 41), (110, 41), (104, 41), (90, 60), (76, 60), (66, 60), (61, 30)] {
        let screen = render(&quiet, w, h);
        assert!(screen.contains("no open issues or PRs") || screen.contains("0 issues"), "{w}x{h}: the empty state:\n{screen}");
        assert!(!screen.contains("newest") && !screen.contains("20+") && !screen.contains("/20"), "{w}x{h}: no page label on a quiet repo:\n{screen}");
    }
    s.set_layout("half-v").unwrap();
    quiet.reload(&s);
    assert!(render(&quiet, 80, 60).contains("no open issues or PRs"));
}

/// gh#78: the two GitHub renderings the tile/summary follow-up (#34) left out — the narrow
/// stat block (`tidy_stats`, drawn in third-h/third-v/the rail) and the one-line GITHUB bar
/// (`gh_bar`) — also mark a full 20-item page: long, else terse, else today's text, never cut
/// mid-word. A wide render sweep (all six layouts) checks that a full page is never rendered
/// identically to a 19-item one; three known configurations (from `third_height_rail_...`,
/// `tidy_github_block_aligns_at_62_48_40` and `medium_bars_when_panels_do_not_fit`, all
/// elsewhere in this file) pin the exact bar and stat-block forms, including the "no column
/// moves" invariant the two-col stat block pad (`TIDY_TWO_COL`) depends on.
#[test]
fn stat_block_and_bar_label_a_full_page() {
    common::pin_clock();
    let (_d1, s1, mut plain, _c1) = setup_page(19, 19, true);
    let (_d2, s2, mut full, _c2) = setup_page(20, 20, true);

    // a full page never renders identically to a 19-item one, at any width/height/layout
    // where a labelled form fits; every label seen is whole, never cut mid-word
    let (mut long, mut terse) = (0, 0);
    for layout in LAYOUTS {
        for (s, app) in [(&s1, &mut plain), (&s2, &mut full)] {
            s.set_layout(layout).unwrap();
            app.reload(s);
        }
        for h in [12u16, 20, 30, 41, 60] {
            for w in (40u16..=200).step_by(4) {
                let (a, b) = (render(&plain, w, h), render(&full, w, h));
                let at = format!("{layout} {w}x{h}");
                for l in b.lines() {
                    assert!(!l.contains("newe") || whole(l, "newest"), "{at}: 'newest' cut: {l}");
                    assert!(!l.contains("in newest") || whole(l, "in newest 20"), "{at}: long label cut: {l}");
                }
                long += usize::from(b.contains("newest") && !a.contains("newest"));
                terse += usize::from(b.contains("20+") && !a.contains("20+"));
            }
        }
    }
    assert!(long > 0, "the sweep never saw a long label appear only on the full page");
    assert!(terse > 0, "the sweep never saw a terse label appear only on the full page");

    // the three targeted checks below all use `auto` (the sweep above left it on half-v)
    for (s, app) in [(&s1, &mut plain), (&s2, &mut full)] {
        s.set_layout("auto").unwrap();
        app.reload(s);
    }

    // 1) the one-line GITHUB bar (RAIL falls back to a bar when even 3 rows do not fit;
    //    `medium_bars_when_panels_do_not_fit` pins 95x35 as exactly that size). At 95 cols
    //    only the terse form fits whole; a wider bar (120) fits the long form.
    let bar_plain = render(&plain, 95, 35);
    let bar_full = render(&full, 95, 35);
    assert!(bar_plain.contains(" GITHUB acme/widgets · 60 issues") && bar_plain.contains("19 PR") && !bar_plain.contains("newest") && !bar_plain.contains("20+"), "19-item bar reads as an unlabelled real total:\n{bar_plain}");
    assert!(bar_full.contains("17/20 free") && bar_full.contains("20+ PR"), "20-item bar not terse-labelled at 95 cols:\n{bar_full}");
    let bar_full_wide = render(&full, 160, 14);
    assert!(bar_full_wide.contains("20 PR newest") && bar_full_wide.contains("free in newest 20"), "20-item bar not long-labelled at 160x14:\n{bar_full_wide}");
    let bar_plain_wide = render(&plain, 160, 14);
    assert!(!bar_plain_wide.contains("newest") && !bar_plain_wide.contains("20+"), "19-item bar wrongly labelled at 160x14:\n{bar_plain_wide}");

    // 2) the narrow stat block, two-column form (`tidy_github_block_aligns_at_62_48_40` pins
    //    62/48/40 wide, 70 tall as ThirdV; >=58 wide keeps MERGED/MAIN CI on the ISSUES/PRS rows)
    // (62x70 is auto a 2x2 grid now: the stacked view is pinned for this block)
    plain.snap.layout = "third-v".into();
    full.snap.layout = "third-v".into();
    let stat_plain = render(&plain, 62, 70);
    let stat_full = render(&full, 62, 70);
    let line = |screen: &str, needle: &str| screen.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no {needle} row:\n{screen}")).to_string();
    let (issues_plain, prs_plain) = (line(&stat_plain, "ISSUES "), line(&stat_plain, "PRS  "));
    let (issues_full, prs_full) = (line(&stat_full, "ISSUES "), line(&stat_full, "PRS  "));
    assert!(!issues_plain.contains('/') && !prs_plain.contains("newest") && !prs_plain.contains("20+"), "19-item stat block reads unlabelled: {issues_plain} / {prs_plain}");
    assert!(issues_full.contains("17/20 free") || issues_full.contains("17 free"), "20-item ISSUES row: {issues_full}");
    assert!(prs_full.contains("20 newest") || prs_full.contains("20+"), "20-item PRS row not labelled: {prs_full}");
    // the caveat: MERGED / MAIN CI never move, whichever form won
    assert_eq!(issues_plain.find("MERGED"), issues_full.find("MERGED"), "MERGED moved:\n{issues_plain}\n{issues_full}");
    assert_eq!(prs_plain.find("MAIN CI"), prs_full.find("MAIN CI"), "MAIN CI moved:\n{prs_plain}\n{prs_full}");

    // 3) the narrow stat block, four-row form (< 58 wide: MERGED/MAIN CI get their own rows)
    let stat_full_n = render(&full, 48, 70);
    let prs_n = line(&stat_full_n, "PRS  ");
    assert!(!prs_n.contains("MERGED") && !prs_n.contains("MAIN CI"), "48 cols should still be the 4-row form: {prs_n}");
    assert!(prs_n.contains("20 newest") || prs_n.contains("20+"), "20-item PRS row (4-row form) not labelled: {prs_n}");

    // a fewer-than-20 repo shows no page label at any of these sizes — nothing to mark
    let (_d, _s, few, _) = setup_page(5, 5, false);
    for (w, h) in [(95u16, 35u16), (62, 70), (48, 70)] {
        let screen = render(&few, w, h);
        assert!(!screen.contains("newest") && !screen.contains("20+") && !screen.contains("/20"), "{w}x{h}: no page label without a full page:\n{screen}");
    }
}

// gh#75: the board footer degrades one hint at a time -------------------------------------

/// Every board hint, in the order the footer drops them (first dropped first). The last two
/// are the floor: `shift+arrows move` is the only board action that is not discoverable
/// anywhere else on screen, and `?` is where every dropped hint is documented.
const FOOTER_DROP_ORDER: [(&str, &str); 10] = [
    ("x", "del"),
    ("e", "edit"),
    ("R", "github: pick repo"),
    ("+/-", "limit"),
    ("B", "boards"),
    ("enter", "open"),
    ("q", "quit"),
    ("a", "add"),
    ("shift+arrows", "move"),
    ("?", "help"),
];
/// The floor: the tail of the drop order that is never dropped.
const FOOTER_FLOOR: usize = 2;

/// Columns one hint costs on the footer: ` {key} {desc} `.
fn hint_cols(k: &str, d: &str) -> usize {
    k.chars().count() + d.chars().count() + 3
}

fn footer_row(screen: &str) -> &str {
    screen.lines().next_back().expect("a footer row")
}

/// The board hints actually rendered, in drop order. A hint counts only when it is whole.
fn footer_hints(row: &str) -> Vec<(&'static str, &'static str)> {
    FOOTER_DROP_ORDER.iter().copied().filter(|(k, d)| row.contains(&format!(" {k} {d} "))).collect()
}

/// Every board hint this state can show, in drop order.
fn footer_full(doing: bool, repo: bool) -> Vec<(&'static str, &'static str)> {
    FOOTER_DROP_ORDER.iter().copied().filter(|(k, _)| (*k != "+/-" || doing) && (*k != "R" || !repo)).collect()
}

/// The board in one of the four states of gh#75: DOING selected or not, repo configured or not.
fn footer_board(doing: bool, repo: bool) -> (tempfile::TempDir, Store, App) {
    let (dir, s, mut app) = setup();
    if !repo {
        s.set_github(None).unwrap();
    }
    app.reload(&s);
    app.agents = AgentsState::Agents(parse_agents(AGENTS, None).unwrap());
    app.col = usize::from(doing);
    (dir, s, app)
}

/// One rendered footer: as many whole hints as fit, dropped strictly in order, floor intact.
fn assert_footer_degrades(ctx: &str, row: &str, w: u16, full: &[(&'static str, &'static str)]) {
    // the focus view keeps its own arrow axis and its own (already progressive) footer
    if row.contains(" arrows card/col ") {
        assert!(row.contains(" shift+<> move ") && row.contains(" ? help "), "{ctx}: the focus floor is gone: {row:?}");
        return;
    }
    let shown = footer_hints(row);
    let floor = &full[full.len() - FOOTER_FLOOR..];
    for f in floor {
        assert!(shown.contains(f), "{ctx}: dropped `{} {}`, which is the floor: {row:?}", f.0, f.1);
    }
    // hints go in drop order, so what is left is always a tail of the full list
    assert_eq!(shown, full[full.len() - shown.len()..], "{ctx}: dropped out of order: {row:?}");
    let used: usize = shown.iter().map(|(k, d)| hint_cols(k, d)).sum();
    if shown.len() > FOOTER_FLOOR {
        assert!(used <= w as usize, "{ctx}: {used} columns of hints do not fit: {row:?}");
    }
    // and as many as fit: putting the last-dropped one back would overflow
    if shown.len() < full.len() {
        let (k, d) = full[full.len() - shown.len() - 1];
        assert!(used + hint_cols(k, d) > w as usize, "{ctx}: `{k} {d}` fits and was dropped anyway: {row:?}");
    }
}

#[test]
fn footer_degrades_one_hint_at_a_time() {
    common::pin_clock();
    // the four states of gh#75, with the width the whole footer first fits in
    const STATES: [(bool, bool, u16); 4] = [(false, true, 79), (true, true, 90), (false, false, 100), (true, false, 111)];
    for pref in LAYOUTS {
        for (doing, repo, full_at) in STATES {
            let (_d, s, mut app) = footer_board(doing, repo);
            s.set_layout(pref).unwrap();
            app.reload(&s);
            app.agents = AgentsState::Agents(parse_agents(AGENTS, None).unwrap());
            app.col = usize::from(doing);
            let full = footer_full(doing, repo);
            let mut prev = 0;
            for w in 40u16..=200 {
                let screen = render(&app, w, 41);
                let row = footer_row(&screen);
                let ctx = format!("{pref} doing={doing} repo={repo} {w}x41");
                assert_footer_degrades(&ctx, row, w, &full);
                if row.contains(" arrows card/col ") {
                    continue;
                }
                let n = footer_hints(row).len();
                assert!(n >= prev, "{ctx}: {prev} hints at {} columns, {n} at {w}: a wider board shows fewer", w - 1);
                prev = n;
                // the thresholds measured on 2.0.0: whole footer here, one hint fewer a column back
                if w == full_at {
                    assert_eq!(n, full.len(), "{ctx}: the whole footer fits in {full_at} columns: {row:?}");
                } else if w + 1 == full_at {
                    assert_eq!(n, full.len() - 1, "{ctx}: exactly one hint goes: {row:?}");
                }
            }
        }
    }
    // the everyday case: an 80-column terminal with DOING selected loses `x` and `e`, not six
    let (_d, _s, app) = footer_board(true, true);
    let screen = render(&app, 80, 41);
    assert_eq!(
        footer_row(&screen).trim_end(),
        " a add  enter open  shift+arrows move  +/- limit  B boards  ? help  q quit",
        "80x41 with DOING selected:\n{screen}"
    );
}

/// Every text a tile can be built from: the four tiles under every page label, each part on
/// its own and in the two shapes a tile draws (`TITLE  value` wide, `value · line 2` dense).
fn tile_texts(app: &App) -> Vec<String> {
    use terminal_board::github::PageLabel;
    let s = app.gh.snap.as_ref().expect("a GitHub snapshot");
    let f = terminal_board::github::factory(s, &app.snap.cards, app.snap.now);
    let mut out = Vec::new();
    for label in [PageLabel::Long, PageLabel::Terse, PageLabel::None] {
        for (title, value, line2) in terminal_board::github::tiles_as(s, &f, app.snap.now, label) {
            // a narrow tile shortens this one title whole, so it is a source text too
            let short = if title == "PULL REQUESTS" { "PRS".to_string() } else { title.clone() };
            out.push(format!("{title}  {value}"));
            out.push(format!("{short}  {value}"));
            out.push(format!("{value} · {line2}"));
            out.extend([title, short, value, line2]);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The text segments of the GITHUB panel's head (its six rows below the title, from its own
/// left edge), split on the box-drawing characters so each tile's inner text stands alone.
fn gh_panel_segments(screen: &str) -> Vec<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let Some(top) = lines.iter().position(|l| l.contains("GITHUB")) else { return Vec::new() };
    let left = col_of(screen, "GITHUB").saturating_sub(2);
    lines
        .iter()
        .skip(top + 1)
        .take(6)
        .flat_map(|l| {
            let row: String = l.chars().skip(left).collect();
            row.split(|c| "─│┌┐└┘┏┓┗┛━┃".contains(c)).map(|s| s.trim().to_string()).collect::<Vec<_>>()
        })
        .filter(|s| s.chars().count() >= 4)
        .collect()
}

/// Issue #79: no tile line is a bare prefix of the text it was built from. A line that did
/// not fit is either a shorter whole form (another page label, `PRS` for `PULL REQUESTS`) or
/// visibly shortened with `…` — never silently cut mid-word.
fn assert_no_silent_cut(ctx: &str, screen: &str, sources: &[String]) {
    for seg in gh_panel_segments(screen) {
        if seg.ends_with('…') || sources.contains(&seg) {
            continue;
        }
        let cut = sources.iter().find(|s| s.starts_with(&seg) && s.chars().count() > seg.chars().count());
        assert!(cut.is_none(), "{ctx}: `{seg}` is `{}` silently cut:\n{screen}", cut.unwrap());
    }
}

/// Issue #79: the sweep. Widths 40-200 x every layout, on a quiet repo, a part page and a
/// busy one: a tile line is whole or visibly shortened at every size. At exactly 102
/// columns the wide tile row left 20 columns for the 21-character `no open issues or PRs`,
/// and the last letter went missing with nothing to show for it. The part page (12 PRs /
/// 7 issues, one draft, red MAIN CI) makes the WIDE tile's first line overflow at narrow
/// sizes (`PRS  12 open (1 draft)` is 23 characters, as long as the empty state) — the
/// other fixtures' line 1 always fits, so without it a revert of only the line-1 half of
/// the fix stayed green.
#[test]
fn tile_lines_are_never_silently_cut() {
    common::pin_clock();
    for (np, ni, busy, what) in
        [(0i64, 0i64, false, "quiet repo"), (12, 7, true, "part page"), (20, 20, true, "busy repo")]
    {
        let (_d, s, mut app, _) = setup_page(np, ni, busy);
        let sources = tile_texts(&app);
        for layout in LAYOUTS {
            s.set_layout(layout).unwrap();
            app.reload(&s);
            app.agents = AgentsState::Agents(parse_agents(AGENTS, None).unwrap());
            for h in [24u16, 41, 60] {
                for w in 40u16..=200 {
                    assert_no_silent_cut(&format!("{what} {layout} {w}x{h}"), &render(&app, w, h), &sources);
                }
            }
        }
    }
    // the reported size, spelled out: the sentence is whole, or shortened so it reads as
    // shortened -- never `no open issues or PR`
    let (_d, s, mut app, _) = setup_page(0, 0, false);
    for layout in ["auto", "half-h"] {
        s.set_layout(layout).unwrap();
        app.reload(&s);
        for w in [102u16, 103] {
            let screen = render(&app, w, 41);
            assert!(!screen.contains("no open issues or PR "), "{layout} {w}x41: the empty state cut mid-word:\n{screen}");
            assert!(screen.contains("no open issues or PRs"), "{layout} {w}x41: the empty state is lost:\n{screen}");
        }
    }
}
