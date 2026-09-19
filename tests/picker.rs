//! Repo picker (R) and `github repos` / `config github` CLI, via a fake gh (TB_GH).
//! One test function: TB_GH is process-wide, so scenarios run in order.
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use terminal_board::herdr::AgentsState;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App, Mode, RepoState};

fn ago(secs: i64) -> String {
    chrono::DateTime::from_timestamp(terminal_board::store::now() - secs, 0).unwrap().to_rfc3339()
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

fn fake_ok(dir: &Path) -> PathBuf {
    let own = format!(
        r#"[{{"nameWithOwner":"me/old","description":"old stuff","pushedAt":"{}","isPrivate":false}},
 {{"nameWithOwner":"me/widgets","description":"image resizing service","pushedAt":"{}","isPrivate":true}}]"#,
        ago(30 * 86400),
        ago(2 * 86400)
    );
    let org = format!(
        r#"[{{"nameWithOwner":"acme/factory","description":"","pushedAt":"{}","isPrivate":false}}]"#,
        ago(3600)
    );
    std::fs::write(dir.join("own.json"), own).unwrap();
    std::fs::write(dir.join("org.json"), org).unwrap();
    let d = dir.display();
    script(
        dir,
        "gh-ok",
        &format!(
            r#"#!/bin/sh
case "$1 $2" in
  "repo list") if [ "$3" = "acme" ]; then cat {d}/org.json; else cat {d}/own.json; fi;;
  "api user/orgs") echo acme;;
  "repo view") if [ "$3" = "good/repo" ]; then echo '{{"nameWithOwner":"good/repo"}}'; else echo "GraphQL: Could not resolve to a Repository with the name '$3'." >&2; exit 1; fi;;
  *) echo '[]';;
esac
"#
        ),
    )
}

fn fake_logged_out(dir: &Path) -> PathBuf {
    script(dir, "gh-out", "#!/bin/sh\necho 'To get started with GitHub CLI, please run:  gh auth login' >&2\nexit 4\n")
}

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn typed(app: &mut App, s: &mut Store, text: &str) {
    for c in text.chars() {
        app.handle_key(key(KeyCode::Char(c)), s);
    }
}

fn wait_loaded(app: &mut App) {
    let t = Instant::now();
    while matches!(app.repos, RepoState::Loading) && t.elapsed() < Duration::from_secs(10) {
        app.poll_repos();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

#[test]
fn picker_and_cli() {
    let dir = tempfile::tempdir().unwrap();
    let ok = fake_ok(dir.path());
    let out = fake_logged_out(dir.path());
    std::env::set_var("TB_GH", &ok);
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.add("x", "", &[], "me").unwrap();
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    app.reload(&s);

    // footer hint when unconfigured
    let screen = render(&app, 160, 50);
    assert!(screen.contains("R github: pick repo") && !screen.contains("G github"), "{screen}");

    // open: loading, then sorted rows (most recently pushed first), none row first
    app.handle_key(key(KeyCode::Char('R')), &mut s);
    assert!(matches!(app.mode, Mode::Picker { .. }));
    let screen = render(&app, 160, 50);
    assert!(screen.contains("Pick a GitHub repo"));
    wait_loaded(&mut app);
    let screen = render(&app, 160, 50);
    let pos = |n: &str| screen.find(n).unwrap_or_else(|| panic!("{n}:\n{screen}"));
    // own account first, then orgs; newest pushed first within a group
    assert!(pos("turn GitHub off") < pos("  widgets"));
    assert!(pos("  widgets") < pos("  old") && pos("  old") < pos("acme") && pos("acme") < pos("  factory"), "{screen}");
    assert!(screen.contains("image resizing"), "{screen}");

    // filter (fuzzy) + select saves config and clears the cache
    s.save_github(&Ok(terminal_board::github::GhSnapshot { repo: "old/cache".into(), ..Default::default() })).unwrap();
    typed(&mut app, &mut s, "widg");
    let screen = render(&app, 160, 50);
    assert!(screen.contains("widgets") && !screen.contains("factory"), "{screen}");
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(s.github_repo().unwrap().as_deref(), Some("me/widgets"));
    assert_eq!(s.github_cache().unwrap(), (None, None, 0), "cache cleared on repo change");
    assert!(render(&app, 160, 50).contains("github: me/widgets"), "status names the pick");
    app.handle_key(key(KeyCode::Esc), &mut s); // clear the status line
    let screen = render(&app, 160, 50);
    assert!(screen.contains("? help") && !screen.contains("pick repo"), "{screen}");

    // current repo is marked
    app.handle_key(key(KeyCode::Char('R')), &mut s);
    wait_loaded(&mut app);
    let screen = render(&app, 160, 50);
    assert!(screen.contains(">*   widgets"), "{screen}");
    // the current repo is pre-selected (row 0 = off, then me/widgets, me/old, acme/factory)
    assert_eq!(app.mode, Mode::Picker { filter: String::new(), sel: 1 });
    // the none row turns it off
    for _ in 0..5 {
        app.handle_key(key(KeyCode::Up), &mut s);
    }
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(s.github_repo().unwrap(), None);

    // typed owner/repo not in the list: validated with gh repo view
    app.handle_key(key(KeyCode::Char('R')), &mut s);
    wait_loaded(&mut app);
    typed(&mut app, &mut s, "bad/nope");
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert!(matches!(app.mode, Mode::Picker { .. }), "stays open on error");
    let screen = render(&app, 160, 50);
    assert!(screen.contains("Could not resolve"), "{screen}");
    assert_eq!(s.github_repo().unwrap(), None);
    for _ in 0.."bad/nope".len() {
        app.handle_key(key(KeyCode::Backspace), &mut s);
    }
    typed(&mut app, &mut s, "good/repo");
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(s.github_repo().unwrap().as_deref(), Some("good/repo"));
    // esc cancels without saving
    app.handle_key(key(KeyCode::Char('R')), &mut s);
    app.handle_key(key(KeyCode::Down), &mut s);
    app.handle_key(key(KeyCode::Esc), &mut s);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(s.github_repo().unwrap().as_deref(), Some("good/repo"));

    // gh logged out: inline error
    std::env::set_var("TB_GH", &out);
    app.handle_key(key(KeyCode::Char('R')), &mut s);
    wait_loaded(&mut app);
    let screen = render(&app, 160, 50);
    assert!(screen.contains("gh not logged in — run 'gh auth login'"), "{screen}");
    app.handle_key(key(KeyCode::Esc), &mut s);

    // CLI parity
    let db = dir.path().join("cli.db");
    let run = |args: &[&str], gh: &Path| {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("TB_DB", &db)
            .env("TB_GH", gh)
            .env("TB_NO_HERDR", "1")
            .output()
            .unwrap()
    };
    let o = run(&["config", "github"], &ok);
    assert!(String::from_utf8_lossy(&o.stdout).contains("github is off"));
    run(&["config", "github", "me/widgets"], &ok);
    let o = run(&["config", "github"], &ok);
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "me/widgets");
    let o = run(&["github", "repos"], &ok);
    let t = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(t.contains("* me/widgets  (private)  pushed 2d  image resizing service") && t.contains("  acme/factory"), "{t}");
    let o = run(&["github", "repos", "--json"], &ok);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let names: Vec<_> = v.as_array().unwrap().iter().map(|r| r["name_with_owner"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, ["acme/factory", "me/widgets", "me/old"]);
    let o = run(&["github", "repos"], &out);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("gh auth login"));
}
