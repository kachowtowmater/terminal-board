//! With `HOME` unset and no `TB_DB`, tb refuses any command that needs the boards/state
//! directory instead of creating `./.local/state/…` under wherever it ran (#206). `TB_DB`
//! still works with no `HOME`, and `-V`/`--help` need no state at all.
//!
//! Drives the real binary under a throwaway working directory (env_isolation-style: the
//! tests share no process with the library, so a removed `HOME` here touches nothing else).
#![cfg(unix)]
use std::path::Path;
use std::process::{Command, Output};

/// Run the real binary with HOME gone (or pointed at a home that stays empty), inside `cwd`.
fn run(cwd: &Path, home: Option<&Path>, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
    c.args(args)
        .current_dir(cwd)
        .env("TB_AS", "tester")
        .env("TB_NO_HERDR", "1")
        .env_remove("TB_DB")
        .env_remove("TTYBOARD_DB")
        .env_remove("TB_BOARD")
        .env_remove("TTYBOARD_BOARD")
        .env_remove("TB_CONFIG")
        .env_remove("TTYBOARD_CONFIG")
        .env_remove("HERDR_AGENT_NAME")
        .stdin(std::process::Stdio::null());
    match home {
        Some(h) => {
            c.env("HOME", h);
        }
        None => {
            c.env_remove("HOME");
        }
    }
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn without_home_tb_refuses_and_creates_nothing() {
    let cwd = tempfile::tempdir().unwrap();
    for args in [
        vec!["p", "add", "x: card"],
        vec!["add", "x: card"],
        vec!["list"],
        vec!["boards"],
        vec!["tb"], // the board picker over a real terminal never gets here; non-tty refuses
    ] {
        let o = run(cwd.path(), None, &args, &[]);
        assert!(
            !o.status.success(),
            "{args:?} with no HOME must fail: {}{}",
            String::from_utf8_lossy(&o.stdout),
            err(&o)
        );
        let e = err(&o);
        assert!(e.contains("HOME is not set"), "{args:?}: {e}");
        assert!(e.contains("TB_DB"), "{args:?}: {e}");
    }
    // nothing was created under the current directory — the old fallback's damage
    assert!(
        !cwd.path().join(".local").exists(),
        "tb created ./.local with HOME unset: {:?}",
        std::fs::read_dir(cwd.path()).unwrap().collect::<Vec<_>>()
    );
}

#[test]
fn tb_db_works_without_home() {
    let cwd = tempfile::tempdir().unwrap();
    let db = cwd.path().join("pinned.db");
    let o = run(cwd.path(), None, &["add", "x: pinned card"], &[("TB_DB", db.to_str().unwrap())]);
    assert!(
        o.status.success(),
        "TB_DB must work with no HOME: {}{}",
        String::from_utf8_lossy(&o.stdout),
        err(&o)
    );
    assert!(db.is_file(), "the pinned file was not created");
    let out = run(cwd.path(), None, &["list", "--json"], &[("TB_DB", db.to_str().unwrap())]);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("pinned card"), "list --json under TB_DB: {s}");
    // and still nothing under the cwd but the file we asked for
    assert!(!cwd.path().join(".local").exists());
}

#[test]
fn version_and_help_need_no_home() {
    let cwd = tempfile::tempdir().unwrap();
    for args in [["-V"], ["--version"], ["--help"]] {
        let o = run(cwd.path(), None, &args, &[]);
        assert!(o.status.success(), "{args:?} without HOME failed: {}", err(&o));
        assert!(!String::from_utf8_lossy(&o.stdout).is_empty(), "{args:?} printed nothing");
    }
    assert!(!cwd.path().join(".local").exists());
}

#[test]
fn a_home_that_exists_but_is_empty_still_works() {
    // HOME set to an empty directory is normal first-run behavior: tb creates its state
    // dir under it (never the cwd) and prints the picker/setup-free non-tty refusal path
    let cwd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let o = run(cwd.path(), Some(home.path()), &["add", "x: card"], &[]);
    assert!(
        o.status.success(),
        "an empty HOME must keep working: {}{}",
        String::from_utf8_lossy(&o.stdout),
        err(&o)
    );
    assert!(
        home.path().join(".local/state/terminal-board/boards/default.db").is_file(),
        "the board went somewhere other than HOME"
    );
    assert!(!cwd.path().join(".local").exists(), "the cwd was touched");
}
