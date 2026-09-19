//! A reader that closes stdout early (`tb list | head -1`, `tb config | grep -q …`) must not
//! make tb panic ("failed printing to stdout: Broken pipe", exit 101): tb stops quietly.
#![cfg(unix)]
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};

fn tb(db: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
    c.env("TB_DB", db).env("TB_GH", "/nonexistent/gh").env("TB_AS", "tester").env("TB_NO_HERDR", "1");
    c
}

/// Commands with enough output to write more than once.
const CMDS: [&[&str]; 6] = [&["list"], &["config"], &["guide"], &["board", "--json"], &["board"], &["show", "1"]];

fn seeded() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    for i in 0..40 {
        let o = tb(&db).args(["add", &format!("tag: card number {i} with a longer title"), "--check", "one", "--check", "two"]).output().unwrap();
        assert!(o.status.success());
    }
    (dir, db)
}

/// Run `args`, close our end of its stdout after reading `keep` bytes, and check the exit.
fn run_closed(db: &Path, args: &[&str], keep: usize) -> Result<(), String> {
    let mut child = tb(db).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut out = child.stdout.take().unwrap();
    if keep > 0 {
        let mut buf = vec![0u8; keep];
        let _ = out.read(&mut buf);
    }
    drop(out); // the reader goes away, like `head -1` or `grep -q` after a match
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    let st = child.wait().unwrap();
    let bad = st.code() == Some(101) || err.contains("panicked") || err.contains("Broken pipe") || st.signal().is_some();
    let ok_code = matches!(st.code(), Some(0));
    if bad || !ok_code {
        return Err(format!("{args:?} keep={keep}: status {st:?}, stderr: {err}"));
    }
    Ok(())
}

#[test]
fn closed_stdout_never_panics() {
    let (_d, db) = seeded();
    let mut failures = Vec::new();
    for round in 0..25 {
        for args in CMDS {
            // closed before tb writes anything, and closed after the first bytes
            for keep in [0, 1 + round % 7] {
                if let Err(e) = run_closed(&db, args, keep) {
                    failures.push(e);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of {} runs failed; first: {}", failures.len(), 25 * CMDS.len() * 2, failures[0]);
}

#[test]
fn a_full_reader_still_gets_everything() {
    let (_d, db) = seeded();
    let o = tb(&db).args(["list"]).output().unwrap();
    assert!(o.status.success());
    assert_eq!(String::from_utf8_lossy(&o.stdout).lines().count(), 40);
}
