//! Issue #37: a repo gh cannot find is reported in full with a repo hint — on the config,
//! setup, fetch and sync paths — and the auth hint appears only for auth failures.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MISSING: &str = "nobody-xyz/does-not-exist-123";

/// A fake gh: `auth status` succeeds; every other call fails with `stderr` (exit 1).
fn fake_gh(dir: &Path, stderr: &str) -> PathBuf {
    let p = dir.join("gh-fake");
    std::fs::write(
        &p,
        format!("#!/bin/sh\ncase \"$1 $2\" in\n  \"auth status\") exit 0;;\nesac\necho \"{stderr}\" >&2\nexit 1\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

fn tb(home: &Path, gh: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(args)
        .env("HOME", home)
        .env("TB_DB", home.join("b.db"))
        .env_remove("TB_BOARD")
        .env("TB_GH", gh)
        .env("TB_AS", "tester")
        .env("TB_NO_HERDR", "1")
        .output()
        .unwrap()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

/// A board already connected to `repo` (saved before the check existed).
fn saved(home: &Path, repo: &str) {
    terminal_board::store::Store::open(&home.join("b.db")).unwrap().set_github(Some(repo)).unwrap();
}

#[test]
fn fetch_and_sync_name_a_missing_repo_in_full_without_the_auth_hint() {
    let home = tempfile::tempdir().unwrap();
    let gh = fake_gh(home.path(), &format!("GraphQL: Could not resolve to a Repository with the name '{MISSING}'. (repository)"));
    saved(home.path(), MISSING);
    let want = format!("no repo '{MISSING}' on GitHub (or no access)");
    for args in [&["github", "--refresh"][..], &["sync"]] {
        let o = tb(home.path(), &gh, args);
        assert!(!o.status.success(), "{args:?}");
        let e = err(&o);
        assert!(e.contains(&want), "{args:?}: the full name: {e}");
        assert!(e.contains("see 'tb github repos'"), "{args:?}: the repo hint: {e}");
        assert!(!e.contains("gh auth status"), "{args:?}: no auth hint for a missing repo: {e}");
    }
    // --json: the same words, and the stored error the board shows is not cut either
    let o = tb(home.path(), &gh, &["sync", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"], format!("github: {want}"));
    assert_eq!(v["hint"], "see 'tb github repos', then 'tb config github OWNER/REPO'");
    let b: serde_json::Value = serde_json::from_slice(&tb(home.path(), &gh, &["board", "--json"]).stdout).unwrap();
    assert_eq!(b["github"]["error"], want.as_str());
}

#[test]
fn the_auth_hint_is_only_for_auth_failures() {
    let home = tempfile::tempdir().unwrap();
    saved(home.path(), "acme/widgets");
    let gh = fake_gh(home.path(), "HTTP 401: Bad credentials (https://api.github.com/graphql)");
    let e = err(&tb(home.path(), &gh, &["sync"]));
    assert!(e.contains("check 'gh auth status', then 'tb sync'"), "{e}");
    let gh = fake_gh(home.path(), "error connecting to api.github.com");
    let e = err(&tb(home.path(), &gh, &["sync"]));
    assert!(e.contains("try 'tb sync' again") && !e.contains("gh auth status") && !e.contains("no repo"), "{e}");
}

#[test]
fn config_calls_only_a_real_not_found_missing() {
    let home = tempfile::tempdir().unwrap();
    // any other gh failure (exit 1 too) is reported as itself, not as "no repo"
    let gh = fake_gh(home.path(), "something unexpected happened");
    let e = err(&tb(home.path(), &gh, &["config", "github", "acme/widgets"]));
    assert!(e.contains("something unexpected happened") && !e.contains("no repo"), "{e}");
    let gh = fake_gh(home.path(), "GraphQL: Could not resolve to a Repository with the name 'acme/nope'.");
    let e = err(&tb(home.path(), &gh, &["config", "github", "acme/nope"]));
    assert!(e.contains("no repo 'acme/nope' on GitHub (or no access) — see 'tb github repos'"), "{e}");
}

#[test]
fn setup_refuses_a_missing_repo_named_with_github() {
    let home = tempfile::tempdir().unwrap();
    let gh = fake_gh(home.path(), &format!("GraphQL: Could not resolve to a Repository with the name '{MISSING}'."));
    let o = tb(home.path(), &gh, &["setup", "--yes", "--no-agents", "--github", MISSING]);
    assert!(!o.status.success(), "setup refuses: {}", String::from_utf8_lossy(&o.stdout));
    assert!(err(&o).contains(&format!("no repo '{MISSING}' on GitHub (or no access) — see 'tb github repos'")), "{}", err(&o));
    let o = tb(home.path(), &gh, &["setup", "--yes", "--no-agents", "--github", MISSING, "--json"]);
    assert!(!o.status.success());
    let out = String::from_utf8_lossy(&o.stdout);
    let v: serde_json::Value = serde_json::from_str(out.lines().skip_while(|l| !l.starts_with('{')).collect::<Vec<_>>().join("\n").as_str())
        .unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"], format!("no repo '{MISSING}' on GitHub (or no access)"));
    assert_eq!(v["hint"], "see 'tb github repos'");
}
