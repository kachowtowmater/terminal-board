//! Displayed text never carries control characters or escape sequences to the terminal:
//! card fields, notes, owners, GitHub titles and agent labels are data. `--json` output shows
//! the same cleaned text (see `json_output_is_cleaned_too`); the store keeps text raw.
#![cfg(unix)]
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};
use terminal_board::github::{GhSnapshot, GhView, Issue, Merged, Pr};
use terminal_board::herdr::{Agent, AgentsState};
use terminal_board::store::{Store, LAYOUTS};
use terminal_board::tui::{draw, App, Mode, View};

/// Harmless sequences of each kind: a colour, a window-title set, a clear screen, C1 forms.
const COLOUR: &str = "\x1b[31m";
const TITLE_SET: &str = "\x1b]0;label\x07";
const CLEAR: &str = "\x1b[2J\x1b[H";
const C1: &str = "\u{9b}2J\u{9d}0;t\u{9c}\u{85}";

fn noisy(word: &str) -> String {
    format!("{word}{COLOUR}{TITLE_SET}-{CLEAR}{C1}\x07\x08!")
}

/// No ESC, no BEL/backspace and no C1 control anywhere in `s`.
fn assert_clean(what: &str, s: &str) {
    let bad: Vec<String> = s
        .chars()
        .filter(|c| {
            let u = *c as u32;
            (u < 0x20 && *c != '\n') || (0x7f..=0x9f).contains(&u)
        })
        .map(|c| format!("U+{:04X}", c as u32))
        .collect();
    assert!(bad.is_empty(), "{what}: control characters {bad:?} in:\n{s}");
}

fn gh_snapshot() -> GhSnapshot {
    let now = chrono::Utc::now().to_rfc3339();
    GhSnapshot {
        repo: "acme/widgets".into(),
        fetched_at: terminal_board::store::now(),
        issues_open: 1,
        prs: vec![Pr {
            number: 20,
            title: noisy("prtitle"),
            head_ref: noisy("branch"),
            is_draft: false,
            review: "-".into(),
            ci: "ok".into(),
            created_at: now.clone(),
            updated_at: now.clone(),
            author: noisy("author"),
            closes: vec![21],
        }],
        issues: vec![Issue {
            number: 21,
            title: noisy("issuetitle"),
            labels: vec![noisy("label")],
            assignees: vec![noisy("assignee")],
            created_at: now.clone(),
        }],
        merged_today: vec![Merged { number: 19, title: noisy("mergedtitle"), merged_at: now }],
        main_ci: None,
    }
}

/// A board where every stored text field carries sequences, plus a cached GitHub snapshot.
fn noisy_board(db: &Path) -> Store {
    let mut s = Store::open(db).unwrap();
    let title = format!("tag: gh#21 {}", noisy("cardtitle"));
    let id = s.add(&title, &format!("line one\n{}", noisy("desc")), &[noisy("item")], &noisy("creator")).unwrap();
    s.take(id, &noisy("owner")).unwrap();
    s.note(id, &noisy("note"), &noisy("owner")).unwrap();
    let b = s.add(&format!("two {}", noisy("blocked")), "", &[], "me").unwrap();
    s.block(b, Some(&noisy("reason")), "me").unwrap();
    s.set_github(Some("acme/widgets")).unwrap();
    s.save_github(&Ok(gh_snapshot())).unwrap();
    s
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

#[test]
fn tui_cells_never_carry_control_characters() {
    let dir = tempfile::tempdir().unwrap();
    let s = noisy_board(&dir.path().join("b.db"));
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.agents = AgentsState::Agents(vec![Agent {
        name: noisy("agent"),
        agent_name: Some(noisy("agent")),
        harness: noisy("harness"),
        status: "working".into(),
        pane_id: "w:p1".into(),
        job: Some(noisy("job")),
        aliases: vec![],
    }]);
    app.reload(&s);
    app.gh = GhView { repo: Some("acme/widgets".into()), snap: Some(gh_snapshot()), error: Some(noisy("gherror")), fails: 3 };
    app.status = Some((noisy("status"), true));
    let mut seen = String::new();
    for layout in LAYOUTS {
        app.snap.layout = layout.to_string();
        for (w, h) in [(200, 60), (140, 40), (100, 30), (60, 20)] {
            for view in [View::Board, View::Github, View::Agents] {
                app.view = view;
                let screen = render(&app, w, h);
                assert_clean(&format!("{layout} {w}x{h} {view:?}"), &screen);
                seen.push_str(&screen);
            }
        }
    }
    // the card popup (description, checklist, notes) and the GitHub item popup
    app.view = View::Board;
    app.popup = Some(s.show(1).unwrap());
    app.mode = Mode::Popup(1);
    let screen = render(&app, 140, 40);
    assert_clean("popup", &screen);
    seen.push_str(&screen);
    app.popup = None;
    app.mode = Mode::GhItem { pr: false, number: 21 };
    let screen = render(&app, 140, 40);
    assert_clean("github item", &screen);
    seen.push_str(&screen);
    // the text itself still shows
    for word in ["cardtitle", "owner", "issuetitle", "prtitle", "agent", "desc", "item", "note"] {
        assert!(seen.contains(word), "{word} missing from every screen");
    }
}

fn tb(db: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(args)
        .env("TB_DB", db)
        .env("TB_GH", "/nonexistent/gh")
        .env("TB_AS", "tester")
        .env("TB_NO_HERDR", "1")
        .envs(env.iter().copied())
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn cli_text_never_carries_control_characters() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    noisy_board(&db);
    for args in [
        vec!["list"],
        vec!["show", "1"],
        vec!["show", "2"],
        vec!["board"],
        vec!["github"],
        vec!["note", "1", "plain note"],
        vec!["take", "2"],
    ] {
        let o = tb(&db, &args, &[]);
        assert_clean(&format!("{args:?} stdout"), &text(&o.stdout));
        assert_clean(&format!("{args:?} stderr"), &text(&o.stderr));
    }
    let list = text(&tb(&db, &["list"], &[]).stdout);
    assert_eq!(list.lines().count(), 2, "one line per card, whatever a title holds:\n{list}");
    assert!(list.contains("cardtitle") && list.contains("owner"), "{list}");
    let show = text(&tb(&db, &["show", "1"], &[]).stdout);
    assert!(show.contains("line one\ndesc"), "descriptions keep their line breaks:\n{show}");
    let gh = text(&tb(&db, &["github"], &[]).stdout);
    assert!(gh.contains("issuetitle") && gh.contains("prtitle") && gh.contains("mergedtitle"), "{gh}");
    // an actor name with sequences, and errors that echo typed text
    let o = tb(&db, &["note", "1", &noisy("typed")], &[("TB_AS", &noisy("actor"))]);
    assert_clean("typed note", &text(&o.stdout));
    let o = tb(&db, &["move", "1", &noisy("column")], &[]);
    assert!(!o.status.success());
    assert_clean("error echo", &text(&o.stderr));
    assert!(text(&o.stderr).contains("column"), "{}", text(&o.stderr));
}

#[test]
fn agents_rows_are_clean() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    noisy_board(&db);
    let json = serde_json::json!({"result": {"agents": [{
        "name": noisy("agentname"), "agent": noisy("harness"), "agent_status": "working",
        "pane_id": "w:p1", "label": format!("{} · {}", noisy("label"), noisy("job"))
    }]}});
    std::fs::write(dir.path().join("agents.json"), json.to_string()).unwrap();
    let herdr = dir.path().join("herdr");
    let script = format!(
        "#!/bin/sh\n[ \"$1 $2\" = \"agent list\" ] && exec cat '{}/agents.json'\nexit 1\n",
        dir.path().display()
    );
    std::fs::write(&herdr, script).unwrap();
    std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(["agents"])
        .env("TB_DB", &db)
        .env("TB_GH", "/nonexistent/gh")
        .env("TB_AS", "tester")
        .env_remove("TB_NO_HERDR")
        .env("HERDR_ENV", "1")
        .env("HERDR_BIN_PATH", &herdr)
        .output()
        .unwrap();
    let out = text(&o.stdout);
    assert!(out.contains("agentname"), "{out}");
    // the board's own actors come first (the card's owner, then the two who wrote card events),
    // then the herdr agent, which holds nothing here — every name is noisy, every line clean
    assert_eq!(out.lines().count(), 4, "{out}");
    assert!(out.lines().next().unwrap().contains("owner") && out.lines().last().unwrap().contains("agentname"), "{out}");
    assert_clean("agents", &out);
}

#[test]
fn json_output_is_cleaned_too() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    noisy_board(&db);
    // every --json command that can carry user text: no control characters, no sequences —
    // the same text the screen shows, still valid JSON that parses
    for args in [
        vec!["show", "1", "--json"],
        vec!["show", "2", "--json"],
        vec!["list", "--json"],
        vec!["board", "--json"],
        vec!["--json"],
    ] {
        let out = tb(&db, &args, &[]).stdout;
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        assert_clean(&format!("{args:?} json stdout"), &text(&out));
        let _ = parsed;
    }
    // the dangerous bytes are gone, the safe characters survive, byte for byte
    let raw = text(&tb(&db, &["show", "1", "--json"], &[]).stdout);
    assert!(!raw.contains('\x1b') && !raw.contains('\x7f'), "DEL and ESC gone:\n{raw}");
    for c in ["\u{9b}", "\u{9d}", "\u{90}", "\u{85}", "\u{9c}", "\x07", "\x08", "\r"] {
        assert!(!raw.contains(c), "C1/control {c:?} gone:\n{raw}");
    }
    assert!(raw.contains("cardtitle"), "ordinary text survives:\n{raw}");
    let shown: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let title = shown["title"].as_str().unwrap();
    // title parsing folds Unicode whitespace (U+0085 became a space when the card was added)
    // and joins words; the words survive, every sequence and control is gone ("label" was
    // INSIDE the title-setting sequence, so it is removed whole with it)
    assert!(title.contains("cardtitle"), "the words stay: {title:?}");
    assert!(!title.contains('\x1b') && !title.contains('\x07') && !title.contains('\x08'), "no ESC/BEL/BS: {title:?}");
    assert!(!title.contains("label"), "a sequence's payload is removed whole with the sequence: {title:?}");
    // the stored description is `line one\n{noisy("desc")}`: line breaks are kept (they were
    // meaningful text from a file), every sequence and control is removed — exactly what the
    // screen shows
    let desc = shown["description"].as_str().unwrap();
    let want = format!("line one\n{}", noisy_desc_cleaned());
    assert_eq!(desc, want, "newlines kept as in the stored text; sequences gone:\n{desc:?}");
    // the owner was `noisy("owner")`: cleaned like the screen (no raw bytes), words kept
    let owner = shown["owner"].as_str().unwrap();
    assert!(owner.contains("owner"), "{owner:?}");
    assert!(!owner.chars().any(|c| { let u = c as u32; (u < 0x20 && c != '\n') || (0x7f..=0x9f).contains(&u) }), "{owner:?}");
}

/// The noisy description as the display sanitiser shows it (`sanitize_lines`): the words and
/// the line break survive, every sequence and control character is gone.
fn noisy_desc_cleaned() -> String {
    terminal_board::text::sanitize_lines(&noisy("desc"))
}
