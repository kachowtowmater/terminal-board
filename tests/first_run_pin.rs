//! The first-run wizard: on a fresh machine bare `tb` in a terminal runs setup, which
//! CREATES the board. Like `tb new` / `tb setup` / create-on-first-use, a session pinned
//! with `TB_AS` must be refused (`as_mismatch`) BEFORE any board file is made (#201).
//! Drives the real binary through a pty (`script`), because first run only triggers on a
//! terminal on stdin and stdout — the same harness `first_run_actor.rs` uses for #158, so
//! a first-run site changed to skip the guard fails here.
use std::process::{Command, Output};

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Bare `tb --as b-y` in a pty on a fresh HOME, under a session pinned to `b-x`. The keys
/// are never needed: the guard must refuse before the wizard can even ask. The watchdog is
/// perl's `alarm` (macOS has no `timeout`); Linux `script` takes the command with `-e -c`,
/// the BSD/macOS one as a bare operand (the same branch `access.rs::board_keys` uses).
fn first_run_drive(home: &std::path::Path, actor: &str) -> Output {
    let tb = env!("CARGO_BIN_EXE_tb");
    let pty = if cfg!(target_os = "linux") {
        format!("script -q -e -c {0} /dev/null", shell_quote(tb))
    } else {
        format!("script -q /dev/null {0} --as b-y; true", shell_quote(tb))
    };
    let feed = format!(
        "(sleep 1; printf sqq; sleep 2; printf q; sleep 1) | perl -e 'alarm shift @ARGV; exec @ARGV' 30 {pty} 2>&1",
    );
    let mut c = Command::new("sh");
    c.arg("-c").arg(&feed);
    c.current_dir(home).env("HOME", home).env("TB_AS", "b-x").env("TB_SESSION", "sess-fr").env("TB_NO_HERDR", "1").env("TZ", "UTC");
    for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD", "TB_CONFIG", "TB_READONLY", "TTYBOARD_READONLY", "HERDR_AGENT_NAME", "TB_HARNESS", "TB_MODEL", "TB_ROLE", "TB_HOST", "TB_TTY", "TTYBOARD_TTY", "AI_AGENT", "OMPCODE", "CLAUDECODE", "CODEX_SESSION_ID", "HERDR_PANE_ID"] {
        c.env_remove(k);
    }
    let o = c.output().unwrap();
    assert_eq!(actor, "b-x", "the pin is the env, the forged --as is the arg");
    o
}

#[test]
fn the_first_run_wizard_refuses_a_forged_as_before_making_the_board() {
    let home = tempfile::tempdir().unwrap();
    let o = first_run_drive(home.path(), "b-x");
    let seen = String::from_utf8_lossy(&o.stdout);
    assert!(seen.contains("as_mismatch"), "the refusal never came: {seen:?}");
    assert!(
        !home.path().join(".local/state/terminal-board/boards/default.db").exists(),
        "the refused first run must not create default.db"
    );
    assert!(
        !home.path().join(".local/state/terminal-board").exists(),
        "no boards directory is left behind by the refused run"
    );
}

#[test]
fn the_first_run_wizard_under_its_own_name_still_runs() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path();
    let tb = env!("CARGO_BIN_EXE_tb");
    let pty = if cfg!(target_os = "linux") {
        format!("script -q -e -c {0} /dev/null", shell_quote(tb))
    } else {
        format!("script -q /dev/null {0}", shell_quote(tb))
    };
    let feed = format!(
        "(sleep 1; printf s; sleep 0.5; printf '\\r'; sleep 2; printf q; sleep 1) | perl -e 'alarm shift @ARGV; exec @ARGV' 30 {pty} 2>&1",
    );
    let mut c = Command::new("sh");
    c.arg("-c").arg(&feed);
    c.current_dir(dir).env("HOME", dir).env("TB_AS", "own-user").env("TB_SESSION", "sess-own").env("TB_NO_HERDR", "1").env("TZ", "UTC");
    for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD", "TB_CONFIG", "TB_READONLY", "TTYBOARD_READONLY", "HERDR_AGENT_NAME", "TB_HARNESS", "TB_MODEL", "TB_ROLE", "TB_HOST", "TB_TTY", "TTYBOARD_TTY", "AI_AGENT", "OMPCODE", "CLAUDECODE", "CODEX_SESSION_ID", "HERDR_PANE_ID"] {
        c.env_remove(k);
    }
    let o = c.output().unwrap();
    let seen = String::from_utf8_lossy(&o.stdout);
    // Proof the refusal path did not eat the wizard itself: the same drive with the actor's
    // OWN name must still ask and still make the board (first_run_actor.rs proves the full
    // recording; this only proves the guard does not over-refuse its own name).
    assert!(seen.contains("Set up Terminal Board now?"), "the wizard never asked: {seen:?}");
    assert!(dir.join(".local/state/terminal-board/boards/default.db").exists(), "own-name first run never made the board");
}
