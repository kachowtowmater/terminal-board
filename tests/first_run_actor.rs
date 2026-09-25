#![cfg(unix)]
//! #158: bare `tb` on a fresh HOME in a real terminal runs the first-run wizard; answering
//! `s` to "Set up Terminal Board now?" still creates the board, so that path must record
//! WHO ran it — the actor (`TB_AS`) and the session (`TB_SESSION`) main.rs hands to
//! `setup::run`. Drives the real binary through a pty (`script`), because first run only
//! triggers on a terminal on stdin and stdout, and asserts the board's `created_by` from
//! `tb boards --json`. Alone in its own file because the drive needs a controlled
//! environment; `setup_first_run_creator.rs` proves the same recording in-process, this one
//! proves main.rs's wiring, so a first-run site changed to `actor: None` fails here.
use std::process::{Command, Output};

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Bare `tb` in a pty on a fresh HOME, fed `s` (skip the wizard) and then `q` (quit the
/// board that opens): the keys are fed to `script`'s stdin, because crossterm reads the
/// terminal (`/dev/tty`), not a pipe — what `script` reads on its stdin is what the board
/// reads as typed keys (the same harness `access.rs` uses for the full-screen board).
fn first_run_drive(home: &std::path::Path, actor: &str) -> Output {
    let tb = env!("CARGO_BIN_EXE_tb");
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    // The watchdog is perl's `alarm`, not `timeout`: macOS has no
    // `timeout`, and a missing watchdog would leave the board (or the wizard) waiting for
    // keys for ever. The pauses let the wizard print and the board start before the keys
    // arrive — everything reads them as they come, never all at once.
    // Linux `script` takes the command with `-e -c`; the BSD/macOS one takes it as a bare
    // operand (the same branch `access.rs::board_keys` uses — same tool, two dialects).
    let pty = if cfg!(target_os = "linux") {
        format!("script -q -e -c {0} /dev/null", shell_quote(tb))
    } else {
        format!("script -q /dev/null {0}", shell_quote(tb))
    };
    let feed = format!(
        "(sleep 1; printf s; sleep 0.5; printf '\\r'; sleep 2; printf q; sleep 1) | perl -e 'alarm shift @ARGV; exec @ARGV' 30 {} 2>&1",
        shell_quote(&pty)
    );
    let mut c = Command::new("sh");
    c.arg("-c").arg(&feed);
    c.current_dir(home).env("HOME", home).env("TB_AS", actor).env("TB_SESSION", "sess-firstrun").env("TB_NO_HERDR", "1").env("TZ", "UTC");
    for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD", "TB_CONFIG", "TB_READONLY", "TTYBOARD_READONLY", "HERDR_AGENT_NAME", "TB_HARNESS", "TB_MODEL", "TB_ROLE", "TB_HOST", "TB_TTY", "TTYBOARD_TTY", "AI_AGENT", "OMPCODE", "CLAUDECODE", "CODEX_SESSION_ID", "HERDR_PANE_ID"] {
        c.env_remove(k);
    }
    c.output().unwrap()
}

#[test]
fn the_first_run_skip_records_the_actor_and_session_of_the_bare_tb_that_ran() {
    let home = tempfile::tempdir().unwrap();
    let o = first_run_drive(home.path(), "firstuser");
    // Proof the wizard really ran and the board really took its quit key — without this,
    // every assertion below is satisfied just as well by a drive that never got past the
    // first-run gate (`script`'s pty is sized 0x0, so nothing legible is drawn, but the
    // alternate screen is unmissable).
    let seen = String::from_utf8_lossy(&o.stdout);
    assert!(seen.contains("Set up Terminal Board now?"), "the wizard never asked: {seen:?}");
    assert!(seen.contains("\x1b[?1049h"), "the board never opened: {seen:?}");
    assert!(seen.contains("\x1b[?1049l"), "the board never took its quit key: {seen:?}");
    assert!(home.path().join(".local/state/terminal-board/boards/default.db").exists(), "the skipped first run never made the board");

    let rows: serde_json::Value = match Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(["boards", "--json"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("USER", "login-user")
        .env("PATH", "/usr/bin:/bin")
        .env_remove("TB_AS")
        .output()
    {
        Ok(o) if o.status.success() => serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("boards --json was not JSON: {e}\n{}", String::from_utf8_lossy(&o.stderr))),
        Ok(o) => panic!("boards --json failed: {}", String::from_utf8_lossy(&o.stderr)),
        Err(e) => panic!("boards --json failed to run: {e}"),
    };
    let c = rows
        .as_array()
        .and_then(|r| r.iter().find(|r| r["name"] == "default"))
        .unwrap_or_else(|| panic!("no default board: {rows}"))["created_by"]
        .clone();
    assert_eq!(
        (c["actor"].as_str(), c["session"].as_str()),
        (Some("firstuser"), Some("sess-firstrun")),
        "bare tb's first run created the board, so it records who ran it: {c}"
    );
    // #158 drives the whole wiring, not just the name: the session comes through
    // `use_environment` too, so the record carries the session of the process that ran.
    assert_eq!(c["source"], "board", "the board's own record, not a log line: {c}");
    // the tests don't depend on the clock, so no `at` check beyond "a time was recorded"
    assert!(c["at"].is_string(), "a creator is recorded with a time: {c}");
}
