//! The AGENTS panel says who is on THIS board and what each one is on (gh#84): the board names
//! them (card owners, reviewers, recent actors), herdr only adds a live status to a pane of
//! exactly that name, and every other pane is counted as elsewhere.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use terminal_board::contract;
use terminal_board::herdr::{parse_agents, Agent, AgentsState};
use terminal_board::store::{Store, LAYOUTS};
use terminal_board::tui::{draw, App, Focus, Mode};

/// Three panes are somebody on the board below (`dev-1`, `dev-2`, `rev-1`). The other three
/// are near misses: `dev-10` and `lead-docs` START with an actor's name (`dev`, `lead`), and
/// the unnamed pane's LABEL and terminal title say `builder`, which is an actor too.
const AGENTS: &str = r#"{"result":{"agents":[
  {"name":"dev-1","agent":"aider","agent_status":"working","pane_id":"w:p1"},
  {"name":"dev-2","agent":"aider","agent_status":"idle","pane_id":"w:p2"},
  {"name":"rev-1","agent":"claude","agent_status":"working","pane_id":"w:p3"},
  {"name":"dev-10","agent":"codex","agent_status":"working","pane_id":"w:p4"},
  {"name":"lead-docs","agent":"claude","agent_status":"blocked","pane_id":"w:p5"},
  {"agent":"aider","agent_status":"working","pane_id":"w:p6"}
]}}"#;
const PANES: &str = r#"{"result":{"panes":[
  {"agent":"aider","agent_status":"working","label":"builder · model-x · aider · fix the importer","pane_id":"w:p6","terminal_title_stripped":"builder"}
]}}"#;

fn agents() -> Vec<Agent> {
    parse_agents(AGENTS, Some(PANES)).unwrap()
}

/// `filer` adds seven cards and holds none. DOING, top to bottom: #2 dev-1, #3 dev-2 (its pane
/// is idle), #5 dev, #6 builder, #7 lead. REVIEW: #4, also dev's, claimed by rev-1.
fn board() -> (tempfile::TempDir, Store) {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(9).unwrap();
    for t in [
        "docs: write install guide",
        "widgets: gh#305 login form rejects emails",
        "widgets: fix flaky upload test",
        "ops: backup restore drill",
        "docs: screenshots for README",
        "ops: rotate API tokens",
        "ops: renew domain",
    ] {
        s.add(t, "", &[], "filer").unwrap();
    }
    for (id, who) in [(2, "dev-1"), (3, "dev-2"), (4, "dev"), (5, "dev"), (6, "builder"), (7, "lead")] {
        s.take(id, who).unwrap();
    }
    s.move_to(4, "review", "dev").unwrap();
    assert_eq!(s.next_review("rev-1").unwrap().id, 4);
    s.note(2, "plus-addresses parse now; writing the regression test", "dev-1").unwrap();
    s.note(4, "reading the diff", "rev-1").unwrap();
    (dir, s)
}

fn app_with(s: &Store, state: AgentsState) -> App {
    let mut app = App::new(s.snapshot().unwrap(), "filer");
    app.reload(s);
    app.agents = state;
    app
}

fn no_herdr() -> AgentsState {
    AgentsState::Unavailable("herdr not available".into())
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
}

/// The AGENTS box of a render — its own columns only, so a rail's box comes without the board
/// columns to its left — or the AGENTS bar; with the header line on top when there is one.
fn agents_part(screen: &str) -> String {
    let lines: Vec<Vec<char>> = screen.lines().map(|l| l.chars().collect()).collect();
    let text = |l: &[char]| l.iter().collect::<String>().trim_end().to_string();
    let mut out = Vec::new();
    if lines[0].iter().collect::<String>().contains("TERMINAL BOARD") {
        // without the clock on its right: the time of day is not part of the look
        out.push(text(&lines[0]).split("refreshed").next().unwrap_or_default().trim_end().to_string());
    }
    let title: Vec<char> = " AGENTS ".chars().collect();
    for (i, l) in lines.iter().enumerate() {
        let Some(at) = l.windows(title.len()).position(|w| w == title.as_slice()) else { continue };
        if at == 0 {
            out.push(text(l)); // the 1-line bar
            continue;
        }
        if !matches!(l[at - 1], '┌' | '┏') {
            continue;
        }
        let x = at - 1;
        for row in &lines[i..] {
            out.push(text(&row[x.min(row.len())..]));
            if row.get(x).is_some_and(|c| matches!(c, '└' | '┗')) {
                break;
            }
        }
    }
    out.join("\n")
}

/// Compare with (or, under `TB_UPDATE_GOLDEN=1`, rewrite) `tests/golden/NAME`.
fn golden(name: &str, got: &str) {
    let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("TB_UPDATE_GOLDEN").is_ok() {
        std::fs::write(&path, format!("{got}\n")).unwrap();
    }
    let want = std::fs::read_to_string(&path).expect("golden file (TB_UPDATE_GOLDEN=1 to create)");
    assert_eq!(got, want.trim_end_matches('\n'), "the AGENTS panel changed ({name})");
}

/// Every shape the board is already tested at: half-h, the rail, the stack, the grid, the
/// bars of a narrow half-h and of the focus view.
const SIZES: [(u16, u16); 6] = [(126, 41), (126, 24), (50, 70), (70, 70), (95, 35), (50, 14)];

fn parts(app: &App) -> String {
    SIZES.iter().map(|(w, h)| format!("===== {w}x{h} =====\n{}", agents_part(&render(app, *w, *h)))).collect::<Vec<_>>().join("\n")
}

#[test]
fn golden_panel_with_herdr() {
    let (_d, s) = board();
    golden("agents_panel.txt", &parts(&app_with(&s, AgentsState::Agents(agents()))));
}

#[test]
fn golden_panel_without_herdr() {
    let (_d, s) = board();
    golden("agents_panel_no_herdr.txt", &parts(&app_with(&s, no_herdr())));
}

#[test]
fn the_board_says_who_and_the_rest_are_counted() {
    let (_d, s) = board();
    let app = app_with(&s, AgentsState::Agents(agents()));
    let screen = render(&app, 160, 50);
    assert!(screen.contains("· 10 agents (7 here, 3 elsewhere)"), "header:\n{screen}");
    let panel = agents_part(&screen);
    let row = |name: &str| panel.lines().find(|l| l.contains(&format!(" {name} "))).unwrap_or_else(|| panic!("no row for {name}:\n{panel}")).to_string();
    // an owner's row: live status, the card, its gh#, its title, its last note and the age
    let dev1 = row("dev-1");
    for want in ["* dev-1", "aider", "working", "#2 ", "gh#305", "login form rejects emails", "\"plus-addresses parse now; writing the regression test\" 0m"] {
        assert!(dev1.contains(want), "{want:?} in {dev1}");
    }
    // a reviewer's row is marked as a review, and quotes the card's last note
    let rev = row("rev-1");
    assert!(rev.contains("#4 ") && rev.contains("review: backup restore drill") && rev.contains("\"reading the diff\""), "{rev}");
    // `dev` owns two open cards: the row is on the one in DOING, not the one waiting in REVIEW
    let dev = row("dev");
    assert!(dev.contains("#5 ") && dev.contains("screenshots for README") && !dev.contains("review"), "{dev}");
    // holds nothing, wrote a card event lately: here, with what it last did
    assert!(row("filer").contains("last created #7 0m"), "{}", row("filer"));
    // rows follow the board: DOING top to bottom, then the reviewer, then who holds nothing
    let order: Vec<usize> = ["dev-1", "dev-2", "dev", "builder", "lead", "rev-1", "filer"]
        .iter()
        .map(|n| panel.lines().position(|l| l.contains(&format!(" {n} "))).unwrap())
        .collect();
    assert!(order.windows(2).all(|p| p[0] < p[1]), "{order:?}\n{panel}");
    // the other panes: one line, counted, worded as "not on this board", never described
    assert!(panel.contains("+3 elsewhere (not on this board)"), "{panel}");
    for hidden in ["dev-10", "lead-docs", "codex", "fix the importer"] {
        assert!(!screen.contains(hidden), "{hidden} is not on this board:\n{screen}");
    }
}

/// The biggest risk of reading WHO from the board: a loose name match would hand a board
/// actor some other pane's live status. `dev` is not `dev-1` or `dev-10`, `lead` is not
/// `lead-docs`, and a pane LABELLED `builder` is not the actor `builder`.
#[test]
fn a_near_miss_name_gets_no_live_status() {
    let (_d, s) = board();
    let app = app_with(&s, AgentsState::Agents(agents()));
    let panel = agents_part(&render(&app, 160, 50));
    for name in ["dev", "builder", "lead"] {
        let row = panel.lines().find(|l| l.contains(&format!(" {name} "))).unwrap_or_else(|| panic!("no row for {name}:\n{panel}"));
        for status in ["working", "blocked", "idle", "aider", "codex", "claude"] {
            assert!(!row.contains(status), "{name} took a near miss's {status}: {row}");
        }
        assert!(row.contains(&format!(" {name:<14} -       -        #")), "no live status reads `-`: {row}");
    }
    let v = serde_json::to_value(contract::agents(&agents(), &s.snapshot().unwrap())).unwrap();
    let list = v.as_array().unwrap();
    for name in ["dev", "builder", "lead"] {
        let a = list.iter().find(|a| a["name"] == name).unwrap_or_else(|| panic!("{name} missing: {v}"));
        assert_eq!((a["harness"].as_str(), a["status"].as_str(), a["pane_id"].as_str()), (Some("-"), Some("-"), Some("")), "{a}");
        assert!(a["job"].is_null() && a["card_id"].is_i64(), "{a}");
    }
    // the near misses themselves hold nothing here
    for pane in ["w:p4", "w:p5", "w:p6"] {
        let a = list.iter().find(|a| a["pane_id"] == pane).unwrap();
        assert!(a["card_id"].is_null() && a["on_board"] == false, "{a}");
    }
    // an exact name is live, whatever its case
    let shout = parse_agents(r#"{"result":{"agents":[{"name":"DEV","agent":"aider","agent_status":"working","pane_id":"w:p9"}]}}"#, None).unwrap();
    let v = serde_json::to_value(contract::agents(&shout, &s.snapshot().unwrap())).unwrap();
    let dev = v.as_array().unwrap().iter().find(|a| a["name"] == "dev").unwrap().clone();
    assert_eq!((dev["status"].as_str(), dev["pane_id"].as_str(), dev["card_id"].as_i64()), (Some("working"), Some("w:p9"), Some(5)), "{dev}");
}

#[test]
fn without_herdr_the_boards_actors_are_still_listed() {
    let (_d, s) = board();
    for state in [no_herdr(), AgentsState::Pending, AgentsState::Agents(vec![])] {
        let app = app_with(&s, state.clone());
        let screen = render(&app, 160, 50);
        assert!(screen.contains("· 7 agents (7 here, 0 elsewhere)"), "{state:?}:\n{screen}");
        let panel = agents_part(&screen);
        for gone in ["herdr not available", "checking herdr", "no agent panes", "elsewhere ("] {
            assert!(!panel.contains(gone), "{state:?}: {gone:?}:\n{panel}");
        }
        let dev1 = panel.lines().find(|l| l.contains(" dev-1 ")).expect(&panel);
        assert!(dev1.contains("-       -        #2 ") && dev1.contains("\"plus-addresses parse now"), "{state:?}: {dev1}");
        assert!(panel.contains("review: backup restore drill"), "{state:?}:\n{panel}");
    }
}

/// Nobody holds or reviews a card, the last card event is over an hour old, and herdr shows
/// nothing: the header, the panel and the bar read exactly as they did before.
#[test]
fn a_board_nobody_is_on_reads_as_before() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    s.add("docs: write install guide", "", &[], "filer").unwrap();
    let c = rusqlite::Connection::open(dir.path().join("b.db")).unwrap();
    c.execute("UPDATE events SET ts = ts - 3600", []).unwrap();
    for (state, panel_says, bar_says) in [
        (no_herdr(), "herdr not available", " AGENTS herdr not available   tab >"),
        (AgentsState::Pending, "checking herdr...", " AGENTS checking herdr...   tab >"),
        (AgentsState::Agents(vec![]), "no agent panes in herdr", " AGENTS 0 working · 0 idle   tab >"),
    ] {
        let app = app_with(&s, state);
        let wide = render(&app, 126, 41);
        assert!(wide.contains("· 1 cards · 0 agents (0 working, 0 idle)"), "{wide}");
        assert!(agents_part(&wide).contains(&format!("│ {panel_says}")), "{wide}");
        assert!(render(&app, 95, 35).contains(bar_says), "{bar_says}");
        assert!(render(&app, 50, 70).contains("AGENTS · 0 working · 0 idle"), "the compact title");
    }
    assert!(contract::agents(&[], &s.snapshot().unwrap()).is_empty());
    // one second inside the hour: the filer is here
    c.execute("UPDATE events SET ts = ts + 1", []).unwrap();
    let app = app_with(&s, no_herdr());
    assert!(render(&app, 126, 41).contains("· 1 agents (1 here, 0 elsewhere)"));
}

/// The header drops `(K here, L elsewhere)` whole, the way it drops the clock: at no width
/// does a piece of it show.
#[test]
fn header_count_is_whole_or_gone() {
    let (_d, s) = board();
    let app = app_with(&s, AgentsState::Agents(agents()));
    let count = "(7 here, 3 elsewhere)";
    let (mut whole, mut gone) = (0, 0);
    for w in 30u16..=200 {
        let screen = render(&app, w, 41);
        let head = screen.lines().next().unwrap();
        if !head.contains("TERMINAL BOARD") {
            continue; // the focus view has its own counts line
        }
        if head.contains(count) {
            whole += 1;
        } else {
            gone += 1;
            assert!(!head.contains('(') && !head.contains("here") && !head.contains("elsew"), "{w}: a piece of the count: {head:?}");
        }
    }
    assert!(whole > 0 && gone > 0, "both cases are covered: {whole} whole, {gone} gone");
    // the width that holds the line but not the count after it: the count goes, the rest stays
    let base = " TERMINAL BOARD · default · 7 cards · 10 agents";
    let w = (base.chars().count() + count.chars().count()) as u16; // one short of both
    let forced = {
        s.set_layout("half-h").unwrap();
        let app = app_with(&s, AgentsState::Agents(agents()));
        render(&app, w, 41)
    };
    assert_eq!(forced.lines().next().unwrap(), base, "{w} wide");
}

/// Nothing in front of a row's card id is ever cut: not the name, not the status word, not
/// `review`; and a line without a card id (`last created #7`, `+3 elsewhere`) is never cut at
/// all. Behind the id only the card title and the quoted note may end in `…`.
fn assert_rows_are_whole(ctx: &str, panel: &str) {
    for line in panel.lines().filter(|l| !l.contains("AGENTS") && !l.contains("TERMINAL BOARD") && !l.contains('─') && !l.contains('━')) {
        let body = line.trim_matches(|c| matches!(c, '│' | '┃' | ' '));
        let toks: Vec<&str> = body.split_whitespace().collect();
        let id_at = toks.iter().position(|t| t.starts_with('#') && t.len() > 1 && !body.contains("last ")).unwrap_or(toks.len());
        for t in &toks[..id_at] {
            assert!(!t.contains('…'), "{ctx}: {t:?} is cut in front of the card id: {line}");
        }
    }
}

/// Narrow rows give up whole fields, the least useful first: the status word, the note, the
/// note's age, the title (the one field that is cut), the card id last — and `review` stays
/// with the id. Checked at every width of every layout.
#[test]
fn narrow_rows_give_up_whole_fields_in_order() {
    let (_d, s) = board();
    for pref in LAYOUTS {
        s.set_layout(pref).unwrap();
        for state in [AgentsState::Agents(agents()), no_herdr()] {
            let app = app_with(&s, state);
            for w in (30u16..=200).step_by(2) {
                for h in [16u16, 24, 41, 56, 73] {
                    let panel = agents_part(&render(&app, w, h));
                    let ctx = format!("{pref} {w}x{h}");
                    assert_rows_are_whole(&ctx, &panel);
                    for row in panel.lines().filter(|l| l.contains(" rev-1 ")) {
                        // the reviewer's card id never shows without the word that marks it
                        assert!(!row.contains("#4") || row.contains("review #4") || row.contains("review: "), "{ctx}: an unmarked review: {row}");
                    }
                    for row in panel.lines().filter(|l| l.contains(" dev-1 ") && l.contains('"')) {
                        assert!(row.contains("#2 "), "{ctx}: a note without its card id: {row}");
                    }
                }
            }
        }
    }
    // the order, on one row of the stack: all of it, then without the status word, then
    // without the note, then the id and a cut title
    s.set_layout("third-v").unwrap();
    let app = app_with(&s, AgentsState::Agents(agents()));
    let row = |w: u16| agents_part(&render(&app, w, 73)).lines().find(|l| l.contains(" dev-1 ")).map(str::to_string);
    let wide = row(90).unwrap();
    assert!(wide.contains("working") && wide.contains("#2 \"plus-addresses") && wide.contains("0m login form"), "{wide}");
    // what a row still shows, as steps: 4 = status word, 3 = note, 2 = the note's age, 1 = a
    // title, 0 = the card id alone. Narrower never shows more, and every step is taken.
    let mut steps = Vec::new();
    for w in (30u16..=90).rev() {
        let Some(r) = row(w) else { continue };
        assert!(r.contains("#2"), "{w}: the card id is the last to go: {r}");
        let step = if r.contains("working") {
            4
        } else if r.contains('"') {
            3
        } else if r.contains(" 0m ") || r.ends_with(" 0m") || r.contains(" 0m│") {
            2
        } else if r.contains("login") {
            1
        } else {
            0
        };
        assert!(r.contains('"') || step < 3, "{w}: {r}");
        assert!(step < 4 || r.contains('"'), "{w}: the status word never outlives the note: {r}");
        assert!(steps.last().is_none_or(|last| step <= *last), "{w}: a narrower row shows more ({steps:?} then {step}): {r}");
        steps.push(step);
    }
    assert!(steps.contains(&4) && steps.contains(&3) && steps.iter().any(|s| *s < 3), "the row steps down between 90 and 30 columns: {steps:?}");
}

/// A panel with fewer rows than actors still ends in the count.
#[test]
fn a_clipped_panel_still_ends_in_the_count() {
    let (_d, mut s) = board();
    let id = s.add("ops: one card more than the panel has rows", "", &[], "filer").unwrap();
    s.take(id, "dev-4").unwrap();
    let app = app_with(&s, AgentsState::Agents(agents()));
    // half-h shows at most 8 rows: 7 of the 8 actors, then the rest as one line
    let panel = agents_part(&render(&app, 126, 45));
    assert!(panel.contains(" rev-1 ") && !panel.contains(" filer "), "{panel}");
    assert!(panel.lines().rev().nth(1).unwrap().contains("+1 more here · +3 elsewhere"), "{panel}");
}

fn press(app: &mut App, s: &mut Store, c: KeyCode) {
    app.handle_key(KeyEvent::new(c, KeyModifiers::NONE), s);
}

#[test]
fn enter_on_a_row_opens_that_actor_and_jumps_to_its_card() {
    let (_d, mut s) = board();
    let mut app = app_with(&s, AgentsState::Agents(agents()));
    render(&app, 160, 50);
    press(&mut app, &mut s, KeyCode::BackTab);
    assert_eq!(app.focus, Focus::Agents);
    for _ in 0..5 {
        press(&mut app, &mut s, KeyCode::Down);
    }
    press(&mut app, &mut s, KeyCode::Enter);
    assert_eq!(app.mode, Mode::AgentInfo(5));
    let screen = render(&app, 160, 50);
    assert!(screen.contains("reviews #4 backup restore drill (review)") && screen.contains("harness claude · status working"), "{screen}");
    press(&mut app, &mut s, KeyCode::Enter);
    assert_eq!((app.mode.clone(), app.focus, app.selected().map(|c| c.id)), (Mode::Normal, Focus::Columns, Some(4)));
    // a board actor without a pane says so instead of guessing (the panel remembers its row:
    // back on rev-1, three up is `dev`)
    press(&mut app, &mut s, KeyCode::BackTab);
    for _ in 0..3 {
        press(&mut app, &mut s, KeyCode::Up);
    }
    press(&mut app, &mut s, KeyCode::Enter);
    let screen = render(&app, 160, 50);
    assert!(screen.contains("no live status") && screen.contains("holds #5 screenshots for README (doing)"), "{screen}");
    press(&mut app, &mut s, KeyCode::Esc);
    // the last row is the elsewhere line: it names the panes, and only names them
    for _ in 0..20 {
        press(&mut app, &mut s, KeyCode::Down);
    }
    press(&mut app, &mut s, KeyCode::Enter);
    assert_eq!(app.mode, Mode::AgentInfo(7));
    let screen = render(&app, 160, 50);
    for want in [" 3 elsewhere ", "dev-10", "lead-docs", "builder"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
}

#[test]
fn agents_json_adds_on_board_and_card_role() {
    let (_d, s) = board();
    let v = serde_json::to_value(contract::agents(&agents(), &s.snapshot().unwrap())).unwrap();
    let got: Vec<(String, bool, Option<String>, Option<i64>)> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["name"].as_str().unwrap().to_string(), a["on_board"].as_bool().unwrap(), a["card_role"].as_str().map(str::to_string), a["card_id"].as_i64()))
        .collect();
    let row = |n: &str, here: bool, role: Option<&str>, card: Option<i64>| (n.to_string(), here, role.map(str::to_string), card);
    assert_eq!(
        got,
        [
            row("dev-1", true, Some("owner"), Some(2)),
            row("dev-2", true, Some("owner"), Some(3)),
            row("dev", true, Some("owner"), Some(5)),
            row("builder", true, Some("owner"), Some(6)),
            row("lead", true, Some("owner"), Some(7)),
            row("rev-1", true, Some("reviewer"), Some(4)),
            row("filer", true, None, None),
            row("dev-10", false, None, None),
            row("lead-docs", false, None, None),
            row("builder", false, None, None),
        ]
    );
    // every field that was there is still there, with its type
    let rev = &v[5];
    assert_eq!((rev["harness"].as_str(), rev["status"].as_str(), rev["pane_id"].as_str()), (Some("claude"), Some("working"), Some("w:p3")));
    assert_eq!(rev["last_note"], "reading the diff");
    assert!(rev["last_event_at"].is_i64() && rev["job"].is_null());
    assert_eq!(v[9]["job"], "fix the importer");
}

/// `tb agents` prints the same set in the same order, with a fake herdr on the path.
#[test]
fn tb_agents_lists_the_board_first() {
    let (dir, _s) = board();
    std::fs::write(dir.path().join("agents.json"), AGENTS).unwrap();
    std::fs::write(dir.path().join("panes.json"), PANES).unwrap();
    let herdr = dir.path().join("herdr");
    let script = format!(
        "#!/bin/sh\n[ \"$1 $2\" = \"agent list\" ] && exec cat '{0}/agents.json'\n[ \"$1 $2\" = \"pane list\" ] && exec cat '{0}/panes.json'\nexit 1\n",
        dir.path().display()
    );
    std::fs::write(&herdr, script).unwrap();
    std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
    let tb = |args: &[&str], with_herdr: bool| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).env("TB_DB", dir.path().join("b.db")).env("TB_GH", "/nonexistent/gh").env("TB_AS", "tester");
        if with_herdr {
            c.env_remove("TB_NO_HERDR").env("HERDR_ENV", "1").env("HERDR_BIN_PATH", &herdr);
        } else {
            c.env("TB_NO_HERDR", "1");
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    };
    let out = tb(&["agents"], true);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 10, "{out}");
    let cols = |l: &str| l.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    assert_eq!(cols(lines[0])[..5], ["dev-1", "aider", "working", "w:p1", "#2"]);
    assert_eq!(cols(lines[2])[..5], ["dev", "-", "-", "-", "#5"], "a near miss lends nothing");
    assert_eq!(cols(lines[5])[..6], ["rev-1", "claude", "working", "w:p3", "review", "#4"]);
    assert_eq!(cols(lines[6])[..5], ["filer", "-", "-", "-", "-"]);
    for l in &lines[7..] {
        assert!(l.contains("(not on this board)"), "{l}");
    }
    assert!(lines[..7].iter().all(|l| !l.contains("not on this board")), "{out}");
    // without herdr: the board's actors, and nothing else
    let out = tb(&["agents"], false);
    assert_eq!(out.lines().count(), 7, "{out}");
    let v: serde_json::Value = serde_json::from_str(&tb(&["agents", "--json"], false)).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 7);
    assert!(v.as_array().unwrap().iter().all(|a| a["on_board"] == true && a["status"] == "-"), "{v}");
}
