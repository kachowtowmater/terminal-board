//! The board picker overlay (`B`): what it lists, switching the running board in place,
//! `esc`, and small panes.
//!
//! The disk-driven scenarios share ONE test function, in order: the picker reads the boards
//! directory under `$HOME`, and `HOME` is process-wide. The render sweep below never reads
//! it (its rows are injected), so it is safe beside them.
mod common;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::path::Path;
use std::process::Command;
use terminal_board::boards::{self, BoardRow};
use terminal_board::store::Store;
use terminal_board::tui::{draw, App, Mode};

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// A board in `$HOME/.local/state/terminal-board/boards/<name>.db`, as `tb NAME` opens it.
fn board(name: &str) -> Store {
    Store::open(&boards::path_for(name)).unwrap().named(name)
}

fn home(dir: &Path) {
    std::env::set_var("HOME", dir);
    for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD"] {
        std::env::remove_var(k);
    }
}

/// What `tb boards --json` reports for the same HOME: (name, default, counts).
fn cli_boards(dir: &Path) -> Vec<(String, bool, [usize; 4])> {
    let out = Command::new(env!("CARGO_BIN_EXE_tb"))
        .arg("boards")
        .arg("--json")
        .env("HOME", dir)
        .env("TB_AS", "alice")
        .env("TB_NO_HERDR", "1")
        .env_remove("TB_DB")
        .env_remove("TTYBOARD_DB")
        .env_remove("TB_BOARD")
        .env_remove("TTYBOARD_BOARD")
        .output()
        .unwrap();
    assert!(out.status.success(), "tb boards --json: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|b| {
            let n = |k: &str| b[k].as_u64().unwrap() as usize;
            (b["name"].as_str().unwrap().to_string(), b["default"].as_bool().unwrap(), [n("todo"), n("doing"), n("review"), n("done")])
        })
        .collect()
}

fn rows_of(app: &App) -> Vec<(String, bool, [usize; 4])> {
    app.boards.iter().map(|b| (b.name.clone(), b.is_default, b.counts)).collect()
}

#[test]
fn board_picker_lists_switches_in_place_and_cancels() {
    common::pin_clock();
    let dir = tempfile::tempdir().unwrap();
    home(dir.path());

    // two boards, deliberately with DIFFERENT settings: the picker must carry them over
    let def = board("default");
    common::seed(&def, "alice").unwrap();
    def.set_github(Some("alice/widgets")).unwrap();
    def.set_panel("agents-panel", "hidden").unwrap();
    def.set_wip(3).unwrap();
    let work = board("work");
    work.add("ops: rotate tokens", "", &[], "alice").unwrap();
    work.set_panel("agents-panel", "shown").unwrap();
    work.set_wip(7).unwrap();
    drop(def);
    drop(work);

    let mut store = board("default");
    let mut app = App::new(store.snapshot().unwrap(), "alice");
    app.reload(&store);
    assert_eq!(app.snap.board, "default");
    assert!(!app.show_agents, "the default board hides AGENTS");
    assert_eq!(app.gh.repo.as_deref(), Some("alice/widgets"));
    assert_eq!(app.snap.wip, 3);
    let before = render(&app, 126, 41);

    // --- B opens the picker, on the board you are on, listing what `tb boards` lists
    assert!(!app.handle_key(key(KeyCode::Char('B')), &mut store));
    let Mode::Boards { sel } = app.mode else { panic!("B did not open the picker: {:?}", app.mode) };
    assert_eq!(sel, 0, "the current board starts selected");
    assert_eq!(rows_of(&app), cli_boards(dir.path()), "the picker's rows are not `tb boards`'s rows");
    assert_eq!(rows_of(&app).len(), 2);

    // --- it is an overlay: nothing on the board underneath moved
    let over = render(&app, 126, 41);
    assert_eq!(before.lines().next(), over.lines().next(), "the header moved:\n{over}");
    assert_eq!(before.lines().last(), over.lines().last(), "the footer moved:\n{over}");
    let (inner, _) = overlay(&over).unwrap_or_else(|| panic!("no picker:\n{over}"));
    for head in ["BOARD", "TODO", "DOING", "REVIEW", "DONE"] {
        assert!(inner.iter().any(|l| l.contains(head)), "picker header {head} missing:\n{over}");
    }
    let row = inner.iter().find(|l| l.contains("work")).unwrap_or_else(|| panic!("no 'work' row:\n{over}"));
    assert!(row.contains('1'), "work's todo count:\n{row}");
    assert!(
        inner.iter().any(|l| l.contains('*') && l.contains("default")),
        "the default board is not marked:\n{over}"
    );

    // --- the key is in the footer hints and in the `?` help
    assert!(before.lines().last().unwrap().contains("B boards"), "footer hint:\n{before}");
    assert!(
        terminal_board::tui::HELP_GROUPS.iter().any(|(_, keys)| keys.iter().any(|(k, _)| *k == "B")),
        "B is missing from the ? help"
    );

    // --- esc changes nothing
    app.handle_key(key(KeyCode::Down), &mut store);
    app.handle_key(key(KeyCode::Esc), &mut store);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.snap.board, "default");
    assert_eq!(store.name, "default");
    assert_eq!(app.snap.wip, 3);
    assert!(!app.show_agents);
    assert_eq!(app.gh.repo.as_deref(), Some("alice/widgets"));
    assert_eq!(render(&app, 126, 41), before, "esc left the board different");

    // --- enter switches the RUNNING board: header, panels and WIP follow the new board
    app.handle_key(key(KeyCode::Char('B')), &mut store);
    app.handle_key(key(KeyCode::Down), &mut store);
    assert_eq!(app.mode, Mode::Boards { sel: 1 });
    app.handle_key(key(KeyCode::Enter), &mut store);
    assert_eq!(app.mode, Mode::Normal, "the picker stays open after enter");
    assert_eq!(store.name, "work", "the store still points at the old board");
    assert_eq!(app.snap.board, "work");
    assert_eq!(app.snap.wip, 7, "the WIP limit did not follow the board");
    assert!(app.show_agents, "the AGENTS panel did not follow the board");
    assert_eq!(app.gh.repo, None, "the GITHUB panel kept the old board's repo");
    assert_eq!(app.snap.cards.len(), 1, "the cards are still the old board's");
    let after = render(&app, 126, 41);
    assert!(after.lines().next().unwrap().contains("work"), "the header still names the old board:\n{after}");
    assert!(after.contains("0/7"), "DOING does not show the new WIP limit:\n{after}");
    assert!(after.contains("rotate tokens"), "the new board's card is missing:\n{after}");
    assert!(after.contains("no repo"), "the GITHUB panel is not the new board's:\n{after}");

    // --- and back again, with the first board's settings intact
    app.handle_key(key(KeyCode::Char('B')), &mut store);
    assert_eq!(app.mode, Mode::Boards { sel: 1 }, "the picker opens on the board you are on");
    app.handle_key(key(KeyCode::Up), &mut store);
    app.handle_key(key(KeyCode::Enter), &mut store);
    assert_eq!(app.snap.board, "default");
    assert_eq!(store.name, "default");
    assert_eq!(app.snap.wip, 3);
    assert!(!app.show_agents);
    assert_eq!(app.gh.repo.as_deref(), Some("alice/widgets"));

    // --- j / k move too (the issue asks for arrows or j/k)
    app.handle_key(key(KeyCode::Char('B')), &mut store);
    app.handle_key(key(KeyCode::Char('j')), &mut store);
    assert_eq!(app.mode, Mode::Boards { sel: 1 });
    app.handle_key(key(KeyCode::Char('k')), &mut store);
    assert_eq!(app.mode, Mode::Boards { sel: 0 });
    app.handle_key(key(KeyCode::Esc), &mut store);

    // --- one board only: a single row that still works, and enter keeps you where you are
    let solo = tempfile::tempdir().unwrap();
    home(solo.path());
    let only = board("default");
    only.add("admin: renew domain", "", &[], "alice").unwrap();
    drop(only);
    let mut store = board("default");
    let mut app = App::new(store.snapshot().unwrap(), "alice");
    app.reload(&store);
    app.handle_key(key(KeyCode::Char('B')), &mut store);
    assert_eq!(app.mode, Mode::Boards { sel: 0 });
    assert_eq!(rows_of(&app), cli_boards(solo.path()));
    assert_eq!(app.boards.len(), 1);
    let one = render(&app, 100, 30);
    assert!(one.contains(" Boards ") && one.contains("default"), "the single row is missing:\n{one}");
    app.handle_key(key(KeyCode::Enter), &mut store);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.snap.board, "default");
    assert_eq!(app.snap.cards.len(), 1);
}

/// The picker's bottom hint, longest first. The first two name the default/archive/restore/
/// delete keys (`*`/`a`/`r`/`d`); a narrow pane still falls back to the
/// short forms it always had.
const HINTS: [&str; 4] = [
    "up/down select · enter switch · * default · a archive · r restore · d delete · esc cancel",
    "enter switch · * default · a archive · r restore · d delete · esc",
    "enter switch · esc cancel",
    "esc",
];

/// The overlay's inner rows, found by its ` Boards ` title and cut to its own box, plus the
/// bottom border row that carries the hint.
fn overlay(screen: &str) -> Option<(Vec<String>, String)> {
    let rows: Vec<Vec<char>> = screen.lines().map(|l| l.chars().collect()).collect();
    let r0 = rows.iter().position(|r| r.iter().collect::<String>().contains("Boards"))?;
    let at = (0..rows[r0].len()).find(|&i| rows[r0][i..].iter().take(6).collect::<String>() == "Boards")?;
    let c0 = (0..=at).rev().find(|&i| rows[r0][i] == '┏')?;
    let c1 = (at..rows[r0].len()).find(|&i| rows[r0][i] == '┓')?;
    let mut inner = Vec::new();
    let mut bottom = String::new();
    for r in rows.iter().skip(r0 + 1) {
        if r[c0] == '┗' {
            bottom = r[c0..=c1].iter().collect();
            break;
        }
        inner.push(r[c0 + 1..c1].iter().collect::<String>());
    }
    Some((inner, bottom))
}

#[test]
fn the_picker_renders_whole_words_in_small_panes() {
    // rows are injected, so this reads no environment and no boards directory
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    common::seed(&s, "alice").unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(&s);
    let names = ["default", "home", "work"];
    app.boards = names
        .iter()
        .zip([[4, 3, 3, 2], [1, 0, 0, 0], [9, 2, 1, 7]])
        .map(|(n, counts)| BoardRow {
            name: (*n).into(),
            is_default: *n == "default",
            counts,
            path: dir.path().join(format!("{n}.db")),
        })
        .collect();
    app.mode = Mode::Boards { sel: 2 };
    let heads = ["BOARD", "TODO", "DOING", "REVIEW", "DONE"];
    let (mut drawn, mut full) = (0, 0);
    for w in 12u16..=140 {
        for h in [5u16, 8, 12, 20, 41] {
            let screen = render(&app, w, h);
            let Some((inner, bottom)) = overlay(&screen) else {
                // too small for an overlay at all: the board is left alone, nothing is cut
                assert!(w < 20 || h < 6, "{w}x{h}: no picker drawn:\n{screen}");
                continue;
            };
            drawn += 1;
            // every word the overlay writes is whole: a header, a board name (or a visibly
            // ellipsized one), a count or a marker — never half a word
            for tok in inner.iter().flat_map(|l| l.split_whitespace()) {
                let whole = heads.contains(&tok)
                    || names.contains(&tok)
                    || tok.chars().all(|c| c.is_ascii_digit())
                    || tok.chars().all(|c| "><*".contains(c));
                let cut_but_marked =
                    tok.ends_with('…') && names.iter().any(|n| n.starts_with(tok.trim_end_matches('…')));
                assert!(whole || cut_but_marked, "{w}x{h}: cut token {tok:?}:\n{screen}");
            }
            // the hint on the bottom border is one of its whole forms, or absent
            let letters = bottom.chars().any(|c| c.is_alphabetic());
            assert!(!letters || HINTS.iter().any(|x| bottom.contains(x)), "{w}x{h}: cut hint {bottom:?}");
            // the selected row is visible however short the pane is (it scrolls to it)
            assert!(inner.iter().any(|l| l.contains('>')), "{w}x{h}: the selection scrolled out:\n{screen}");
            if w >= 48 {
                full += 1;
                for head in heads {
                    assert!(inner.iter().any(|l| l.contains(head)), "{w}x{h}: header {head} missing:\n{screen}");
                }
                for n in names {
                    assert!(inner.iter().any(|l| l.contains(n)), "{w}x{h}: board {n} missing:\n{screen}");
                }
            }
        }
    }
    assert!(drawn > 100, "the sweep barely drew the picker ({drawn})");
    assert!(full > 100, "the sweep never reached the full-width picker ({full})");
}
