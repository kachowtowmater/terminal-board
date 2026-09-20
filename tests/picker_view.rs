//! Repo picker rendering (no gh: the repo list is injected).
mod common;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use terminal_board::github::RepoEntry;
use terminal_board::store::Store;
use terminal_board::tui::{draw, palette, App, Mode, RepoState};

fn ago(secs: i64) -> String {
    chrono::DateTime::from_timestamp(terminal_board::store::now() - secs, 0).unwrap().to_rfc3339()
}

fn repo(n: &str, desc: &str, age: i64, private: bool, own: bool) -> RepoEntry {
    RepoEntry { name_with_owner: n.into(), description: desc.into(), pushed_at: ago(age), is_private: private, own }
}

fn setup() -> (tempfile::TempDir, Store, App) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    common::seed(&s, "me").unwrap();
    s.set_github(Some("alice/widgets")).unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.reload(&s);
    app.repos = RepoState::Loaded(vec![
        repo("alice/notes", "plain-text notes, synced", 4 * 60, true, true),
        repo("alice/widgets", "a tiny widget library", 16 * 60, true, true),
        repo("alice/dotfiles", "", 26 * 60, true, true),
        repo("alice/recipes", "Family recipes, one markdown file per dish, with a very long description that cannot fit", 6 * 86400, false, true),
        repo("zeta-org/factory", "the zeta factory", 3600, false, false),
        repo("acme/tools", "tooling", 2 * 86400, false, false),
    ]);
    app.mode = Mode::Picker { filter: String::new(), sel: 0 };
    (dir, s, app)
}

fn render(app: &App, w: u16, h: u16) -> (String, Buffer) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer().clone();
    let text = b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n");
    (text, b)
}

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn line_of<'a>(screen: &'a str, needle: &str) -> (usize, &'a str) {
    screen.lines().enumerate().find(|(_, l)| l.contains(needle)).unwrap_or_else(|| panic!("{needle}:\n{screen}"))
}

fn col(line: &str, needle: &str) -> usize {
    line[..line.find(needle).unwrap_or_else(|| panic!("{needle} in {line}"))].chars().count()
}

#[test]
fn layout_groups_and_alignment_at_100_wide() {
    common::pin_clock();
    let (_d, _s, app) = setup();
    let (screen, _) = render(&app, 104, 40); // popup width = min(100, 104 - 4) = 100
    for want in ["Pick a GitHub repo", "search: _", "off   turn GitHub off", "REPO", "PUSHED", "DESCRIPTION", "6 of 6 repos · * = current", "type to search  up/down select  enter pick  esc cancel"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    let top = line_of(&screen, "Pick a GitHub repo");
    let bottom = line_of(&screen, "type to search");
    let width = top.1.chars().count() - top.1.chars().take_while(|c| *c == ' ').count();
    assert!(width >= 100, "popup spans 100 cols");
    assert!(top.0 < bottom.0);
    // groups: own account first, then orgs alphabetically; repos by recency, names only
    let order = ["alice", "  notes", "  widgets", "  dotfiles", "  recipes", "acme", "  tools", "zeta-org", "  factory"];
    let rows: Vec<usize> = order.iter().map(|n| line_of(&screen, n).0).collect();
    assert!(rows.windows(2).all(|w| w[0] < w[1]), "{rows:?}\n{screen}");
    assert!(!screen.contains("acme/widgets"), "repo rows show the name only");
    // aligned columns: header and cells start at the same x
    let (_, header) = line_of(&screen, "DESCRIPTION");
    let pushed_x = col(header, "PUSHED");
    let desc_x = col(header, "DESCRIPTION");
    for (name, age, desc) in [("notes", "4m", "plain-text"), ("widgets", "16m", "a tiny widget"), ("recipes", "6d", "Family recipes")] {
        let (_, l) = line_of(&screen, &format!("  {name} "));
        assert_eq!(col(l, &format!(" {age} ")) + 1, pushed_x, "{name} PUSHED aligned:\n{l}");
        assert_eq!(col(l, desc), desc_x, "{name} DESCRIPTION aligned");
    }
    let (_, cm) = line_of(&screen, "  widgets ");
    let (_, io) = line_of(&screen, "  notes ");
    assert_eq!(col(cm, "private"), col(io, "private"));
    assert!(cm.contains(" *  ") || cm.contains("*   widgets"), "current repo marked:\n{cm}");
    // long descriptions are cut with an ellipsis
    let (_, mb) = line_of(&screen, "  recipes ");
    assert!(mb.contains('…'), "{mb}");
}

#[test]
fn owner_headers_are_not_selectable() {
    common::pin_clock();
    let (_d, mut s, mut app) = setup();
    let mut seen = Vec::new();
    for _ in 0..10 {
        app.handle_key(key(KeyCode::Down), &mut s);
        let (screen, buf) = render(&app, 104, 40);
        let Mode::Picker { sel, .. } = app.mode.clone() else { panic!() };
        seen.push(sel);
        // the reversed row is always a repo row, never an owner header
        for owner in ["alice", "acme", "zeta-org"] {
            let (y, l) = line_of(&screen, owner);
            let x = col(l, owner) as u16;
            assert!(!buf[(x, y as u16)].modifier.contains(Modifier::REVERSED), "{owner} header selected");
        }
    }
    assert_eq!(seen, [1, 2, 3, 4, 5, 6, 6, 6, 6, 6], "6 repos, headers skipped");
    let (screen, buf) = render(&app, 104, 40);
    let (y, l) = line_of(&screen, "  factory ");
    assert!(buf[(col(l, "factory") as u16, y as u16)].modifier.contains(Modifier::REVERSED));
    assert!(l.contains(">"), "selection marker");
}

#[test]
fn filter_hides_empty_groups_and_offers_owner_repo() {
    common::pin_clock();
    let (_d, mut s, mut app) = setup();
    for c in "WIDG".chars() {
        app.handle_key(key(KeyCode::Char(c)), &mut s);
    }
    let (screen, _) = render(&app, 104, 40);
    assert!(screen.contains("search: WIDG_") && screen.contains("  widgets"), "{screen}");
    assert!(!screen.contains("acme") && !screen.contains("zeta-org"), "empty groups hidden:\n{screen}");
    assert!(screen.contains("1 of 6 repos"));
    assert_eq!(app.mode, Mode::Picker { filter: "WIDG".into(), sel: 1 });
    // nothing matches but it looks like owner/repo
    for _ in 0..4 {
        app.handle_key(key(KeyCode::Backspace), &mut s);
    }
    for c in "someone/else".chars() {
        app.handle_key(key(KeyCode::Char(c)), &mut s);
    }
    let (screen, buf) = render(&app, 104, 40);
    let (y, l) = line_of(&screen, "use someone/else (enter to check)");
    assert!(buf[(col(l, "use") as u16, y as u16)].modifier.contains(Modifier::REVERSED), "fallback row selected");
    assert!(screen.contains("0 of 6 repos"));
}

#[test]
fn background_is_painted_and_states_render() {
    common::pin_clock();
    let (_d, _s, mut app) = setup();
    let (w, h) = (120u16, 44u16);
    let (screen, buf) = render(&app, w, h);
    // popup rect: width min(100, w-4), height min(h-2, rows+10); rows = 3 headers + 6 repos
    let pw = 100u16;
    let ph = (9 + 10).min(h - 2);
    let (px, py) = ((w - pw) / 2, (h - ph) / 2);
    let bg = palette("dark").bg;
    for y in py..py + ph {
        let row: String = (px..px + pw).map(|x| buf[(x, y)].symbol().to_string()).collect();
        for board in ["o TODO", "o DOING", "#1 ", "csv export", "TERMINAL BOARD"] {
            assert!(!row.contains(board), "board bleeds through at row {y}: {row}");
        }
        for x in px..px + pw {
            assert_eq!(buf[(x, y)].bg, bg);
        }
    }
    assert!(screen.contains("Pick a GitHub repo"));
    // loading: centred; error: centred and red (the only colour)
    app.repos = RepoState::Loading;
    let (screen, _) = render(&app, w, h);
    let (_, l) = line_of(&screen, "loading repos…");
    let x = col(l, "loading");
    assert!((x as i32 - (w as i32 / 2 - 7)).abs() <= 3, "centred: {x}");
    app.repos = RepoState::Error("gh not logged in — run 'gh auth login'".into());
    let (screen, buf) = render(&app, w, h);
    let (y, l) = line_of(&screen, "gh not logged in");
    assert_eq!(buf[(col(l, "gh not") as u16, y as u16)].fg, Color::Red);
}
