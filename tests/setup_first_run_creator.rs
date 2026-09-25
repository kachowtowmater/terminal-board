#![cfg(unix)]
//! #147: bare `tb` on a fresh machine runs the wizard with `first_run`; answering `s` to
//! "Set up Terminal Board now?" still creates the board (it marks it set up), so that path
//! must record the creator too. Drives `setup::run` in-process (the real first-run needs a
//! terminal on stdin/stdout and then opens the TUI), with answers from `TB_TTY`. Alone in
//! its own file because it sets the process HOME.
use std::process::Command;
use terminal_board::setup::{run, Options};

#[test]
fn the_first_run_skip_records_the_creator_of_the_board_it_creates() {
    let home = tempfile::tempdir().unwrap();
    let answers = home.path().join("answers");
    std::fs::write(&answers, "s\n").unwrap();
    for (k, _) in std::env::vars() {
        if k.starts_with("TB_") || k.starts_with("TTYBOARD_") {
            std::env::remove_var(k);
        }
    }
    std::env::set_var("HOME", home.path());
    std::env::set_var("TB_TTY", &answers);
    std::env::set_var("TB_SESSION", "sess-first");

    run("default", Options { first_run: true, actor: Some("firstuser".into()), ..Default::default() }).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(["boards", "--json"])
        .env_clear()
        .env("HOME", home.path())
        .env("USER", "login-user")
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let row = rows.as_array().unwrap().iter().find(|r| r["name"] == "default").unwrap_or_else(|| panic!("no default board: {rows}"));
    let c = &row["created_by"];
    assert_eq!(
        (c["actor"].as_str(), c["session"].as_str(), c["source"].as_str()),
        (Some("firstuser"), Some("sess-first"), Some("board")),
        "the skipped first run created the board, so it records who ran it: {c}"
    );
}
