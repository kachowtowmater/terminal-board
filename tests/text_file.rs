//! Text from a file or from standard input: `tb add … --desc-file PATH|-`,
//! `tb edit ID --desc-file PATH|-`, `tb note ID --file PATH|-`.
//!
//! The point of the feature is that NOTHING touches the text on the way in — a shell eats
//! backticks, `$`, quotes and backslashes in a long string; a file does not. Every test here
//! drives the real binary and reads the stored text back from SQLite (raw, byte for byte);
//! the `--json` view shows the same text cleaned like the screen.
#![cfg(unix)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Everything a shell, a format string or a terminal would mangle. No leading or trailing
/// blank space apart from the file's final newline (outer blank space has its own test).
const NASTY: &str = concat!(
    "Done = `cargo test` passes and $HOME, ${HOME} and $(rm -rf /) stay as typed\n",
    "quotes: \"double\" 'single' and a lone \" and a lone ' and `back`ticks`\n",
    "backslashes: \\ \\\\ \\n \\t C:\\temp\\new  (none of these is an escape)\n",
    "\ttab-indented line, with a tab\there and trailing spaces   \n",
    "\n",
    "\n",
    "a Windows line ends in CR LF\r\n",
    "- a line that looks like a flag: --force --json -b other\n",
    "=1+1 +2 @SUM(A1) %s %d {} {0} !! ~ * ? [a-z] | > < & ; # $? $$ $!\n",
    "unicode: héllo · naïve — “curly” 你好 🚀 e\u{301}\n",
    "what happened — and the separator tb uses in its own errors\n",
    "last line\n",
);

struct Board {
    dir: tempfile::TempDir,
    db: PathBuf,
}

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("board.db");
        Board { dir, db }
    }
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args)
            .current_dir(self.dir.path())
            .env("TB_DB", &self.db)
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .env_remove("TB_BOARD")
            .env_remove("HERDR_AGENT_NAME");
        c
    }
    /// Run with standard input closed (/dev/null): what an agent's harness usually gives.
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).stdin(Stdio::null()).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }
    /// Run with `input` piped to standard input.
    fn piped(&self, args: &[&str], input: &[u8]) -> Output {
        let mut child = self.cmd(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        // a refusal may close the pipe before everything is written: that is not the test's error
        let _ = stdin.write_all(input);
        drop(stdin);
        child.wait_with_output().unwrap()
    }
    fn file(&self, name: &str, bytes: &[u8]) -> String {
        let p = self.dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p.to_str().unwrap().to_string()
    }
    /// Text as the STORE holds it — read from SQLite, not through the `--json` view (JSON
    /// output shows text cleaned like the screen; the store keeps it raw, byte for byte).
    fn stored(&self, sql: &str, id: i64) -> String {
        let conn = rusqlite::Connection::open_with_flags(&self.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        conn.query_row(sql, [id], |r| r.get(0)).unwrap()
    }
    fn card(&self, id: i64) -> serde_json::Value {
        serde_json::from_str(&self.ok(&["show", &id.to_string(), "--json"])).unwrap()
    }
    fn description(&self, id: i64) -> String {
        self.stored("SELECT description FROM cards WHERE id = ?", id)
    }
    fn notes(&self, id: i64) -> Vec<String> {
        let conn = rusqlite::Connection::open_with_flags(&self.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let mut stmt = conn.prepare("SELECT text FROM events WHERE card_id = ? AND kind = 'note' ORDER BY id").unwrap();
        let rows = stmt.query_map([id], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        rows
    }
    fn event_count(&self, id: i64) -> usize {
        self.card(id)["events"].as_array().unwrap().len()
    }
}

fn refusal(o: &Output) -> String {
    assert_eq!(o.status.code(), Some(1), "a refusal exits 1: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(o.stdout.is_empty(), "a refusal prints nothing on stdout: {}", String::from_utf8_lossy(&o.stdout));
    String::from_utf8_lossy(&o.stderr).to_string()
}

fn json_refusal(o: &Output) -> (String, String) {
    assert_eq!(o.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], false, "{v}");
    let mut keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["code", "error", "hint", "ok"], "{v}");
    // #81: every refusal reading text from a file/stdin carries the stable `io_error` code
    assert_eq!(v["code"], "io_error", "{v}");
    (v["error"].as_str().unwrap().to_string(), v["hint"].as_str().unwrap().to_string())
}

/// The nasty fixture reaches the store byte for byte through all three commands, from a file.
#[test]
fn a_file_arrives_byte_for_byte() {
    let b = Board::new();
    let f = b.file("brief.md", NASTY.as_bytes());
    let want = NASTY.trim_end_matches('\n');
    assert!(want.contains('\t') && want.contains("\r\n") && want.contains("\n\n\n") && want.contains("   \n"), "the fixture keeps its teeth");

    let out = b.ok(&["add", "docs: from a file", "--desc-file", &f]);
    assert!(out.starts_with("added #1"), "{out}");
    assert_eq!(b.description(1), want, "add --desc-file");

    b.ok(&["add", "docs: edited from a file", "-d", "old text"]);
    assert_eq!(b.ok(&["edit", "2", "--desc-file", &f]).trim(), "#2 saved");
    assert_eq!(b.description(2), want, "edit --desc-file");

    assert_eq!(b.ok(&["note", "2", "--file", &f]).trim(), "noted #2");
    assert_eq!(b.notes(2), [want.to_string()], "note --file");

    // and a relative path means relative to where tb runs
    b.ok(&["note", "2", "--file", "brief.md"]);
    assert_eq!(b.notes(2).len(), 2);
}

/// The same bytes through standard input (`-`), and through a real shell pipe and redirect —
/// the two forms the manual teaches.
#[test]
fn standard_input_arrives_byte_for_byte() {
    let b = Board::new();
    let want = NASTY.trim_end_matches('\n');
    let o = b.piped(&["add", "docs: piped", "--desc-file", "-"], NASTY.as_bytes());
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(b.description(1), want);

    b.ok(&["add", "docs: second"]);
    let o = b.piped(&["edit", "2", "--desc-file", "-", "--json"], NASTY.as_bytes());
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    // the --json answer shows the stored text cleaned: line breaks and TABS are text and
    // survive, so the text still round-trips; only the CR of a CRLF becomes a space
    let cleaned = v["card"]["description"].as_str().unwrap();
    assert_eq!(cleaned, want.replace('\r', " "), "the --json answer keeps the tabs and folds only CR");
    assert!(cleaned.contains("a tab\there"), "a tab written from a file survives the JSON view: {cleaned:?}");
    assert!(!cleaned.contains('\x1b') && !cleaned.contains('\r'), "JSON output is cleaned: {cleaned:?}");

    let o = b.piped(&["note", "2", "--file", "-", "--json"], NASTY.as_bytes());
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    let note_text = v["card"]["events"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap();
    assert_eq!(note_text, want.replace('\r', " "), "the --json answer shows the stored note (cleaned)");

    // through /bin/sh: a redirect and a pipe
    let f = b.file("brief.md", NASTY.as_bytes());
    for script in ["\"$TB\" note 2 --file - < \"$F\"", "cat \"$F\" | \"$TB\" note 2 --file -"] {
        let o = Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("TB", env!("CARGO_BIN_EXE_tb"))
            .env("F", &f)
            .env("TB_DB", &b.db)
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(o.status.success(), "{script}: {}", String::from_utf8_lossy(&o.stderr));
    }
    assert_eq!(b.notes(2), [want.to_string(), want.to_string(), want.to_string()]);
}

/// One file leaves the same text whichever command carried it: outer blank space is trimmed
/// (as `edit --desc` and note text already are), everything inside is kept.
#[test]
fn outer_blank_space_is_trimmed_the_same_way_everywhere() {
    let b = Board::new();
    let f = b.file("padded.md", b"\n\n  \tfirst line\n\n    indented inside\n\nlast line  \n\n\n");
    let want = "first line\n\n    indented inside\n\nlast line";
    b.ok(&["add", "a: one", "--desc-file", &f]);
    b.ok(&["add", "a: two"]);
    b.ok(&["edit", "2", "--desc-file", &f]);
    b.ok(&["note", "2", "--file", &f]);
    assert_eq!(b.description(1), want);
    assert_eq!(b.description(2), want);
    assert_eq!(b.notes(2), [want.to_string()]);
    // a UTF-8 byte-order mark is an encoding signature, not text
    let bom = b.file("bom.md", "\u{feff}héllo\n".as_bytes());
    b.ok(&["edit", "1", "--desc-file", &bom]);
    assert_eq!(b.description(1), "héllo");
    // and `add -d` agrees with them: every way to write a description trims it the same way
    b.ok(&["add", "a: three", "-d", "  typed with blank space around it \n"]);
    assert_eq!(b.description(3), "typed with blank space around it");
}

/// The store keeps text raw (read back from SQLite); every screen path and the JSON path go
/// through the control-character sanitiser, so escape sequences in a file never reach a
/// terminal, a log or another tool.
#[test]
fn the_display_sanitiser_still_applies() {
    let b = Board::new();
    let raw = "before\x1b[31mred\x1b[0m \x1b]0;window title\x07 \u{9b}2J bell\x07 back\x08space\nsecond line\tafter a tab";
    let f = b.file("noisy.md", raw.as_bytes());
    b.ok(&["add", "ops: noisy", "--desc-file", &f]);
    b.ok(&["note", "1", "--file", &f]);
    assert_eq!(b.description(1), raw, "stored raw, exactly as --desc stores it");
    assert_eq!(b.notes(1), [raw.to_string()]);
    for args in [&["show", "1", "--json"][..], &["list", "--json"][..], &["board", "--json"][..], &["show", "1"][..], &["list"][..], &["board"][..], &[][..]] {
        let out = b.ok(args);
        let bad: Vec<String> = out
            .chars()
            .filter(|c| {
                let u = *c as u32;
                (u < 0x20 && *c != '\n') || (0x7f..=0x9f).contains(&u)
            })
            .map(|c| format!("U+{:04X}", c as u32))
            .collect();
        assert!(bad.is_empty(), "{args:?} printed control characters {bad:?}:\n{out}");
    }
    let shown = b.ok(&["show", "1"]);
    assert!(shown.contains("beforered") && shown.contains("second line after a tab"), "{shown}");
    assert!(!shown.contains("window title"), "a title-setting sequence is removed whole:\n{shown}");
}

/// The size limit is stated, exact, and enforced on files and on standard input alike.
#[test]
fn the_size_limit_is_stated_and_exact() {
    const LIMIT: usize = 256 * 1024;
    let b = Board::new();
    b.ok(&["add", "big: text"]);
    let at = b.file("at.txt", "x".repeat(LIMIT).as_bytes());
    let over = b.file("over.txt", "x".repeat(LIMIT + 1).as_bytes());
    b.ok(&["edit", "1", "--desc-file", &at]);
    assert_eq!(b.description(1).len(), LIMIT, "exactly the limit is accepted");

    let before = b.event_count(1);
    let e = refusal(&b.run(&["edit", "1", "--desc-file", &over]));
    assert!(e.contains("over.txt' is 262145 bytes; text from a file is limited to 262144 bytes (256 KiB) — "), "{e}");
    let e = refusal(&b.run(&["note", "1", "--file", &over]));
    assert!(e.contains("limited to 262144 bytes (256 KiB)"), "{e}");
    let o = b.piped(&["note", "1", "--file", "-"], "y".repeat(LIMIT + 1).as_bytes());
    let e = refusal(&o);
    assert!(e.contains("standard input is over the limit of 262144 bytes (256 KiB)"), "{e}");
    let (err, hint) = json_refusal(&b.piped(&["edit", "1", "--desc-file", "-", "--json"], "y".repeat(LIMIT + 1).as_bytes()));
    assert!(err.starts_with("standard input is over the limit of 262144 bytes") && hint.starts_with("shorten it"), "{err} / {hint}");
    assert_eq!(b.event_count(1), before, "a refused text writes nothing");
    assert_eq!(b.description(1).len(), LIMIT);
}

/// A file that cannot be used is refused in the house style — what happened, then the exact
/// command — and nothing is written.
#[test]
fn unusable_files_are_refused_with_the_next_command() {
    let b = Board::new();
    b.ok(&["add", "docs: target", "-d", "the brief"]);
    let before = b.event_count(1);

    let e = refusal(&b.run(&["note", "1", "--file", "no-such-file.md"]));
    assert_eq!(
        e.trim(),
        "tb: no file 'no-such-file.md' — check the path (it is relative to where tb runs): 'tb note 1 --file PATH', or pipe the text: 'tb note 1 --file -'"
    );
    let (err, hint) = json_refusal(&b.run(&["edit", "1", "--desc-file", "no-such-file.md", "--json"]));
    assert_eq!(err, "no file 'no-such-file.md'");
    assert!(hint.contains("'tb edit 1 --desc-file PATH'") && hint.contains("'tb edit 1 --desc-file -'"), "{hint}");

    let dir = b.dir.path().to_str().unwrap().to_string();
    let e = refusal(&b.run(&["edit", "1", "--desc-file", &dir]));
    assert!(e.contains("is a directory, not a text file — name a file: 'tb edit 1 --desc-file PATH'"), "{e}");

    let latin1 = b.file("latin1.txt", b"caf\xe9 au lait");
    let e = refusal(&b.run(&["note", "1", "--file", &latin1]));
    assert!(e.contains("is not UTF-8 text (bad byte at offset 3) — ") && e.contains("'tb note 1 --file PATH'"), "{e}");

    let utf16 = b.file("utf16.txt", b"\xff\xfeh\0i\0");
    let e = refusal(&b.run(&["note", "1", "--file", &utf16]));
    assert!(e.contains("holds a NUL byte") && e.contains("not a text file"), "{e}");

    let secret = b.file("secret.txt", b"text");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    // (root reads anything: only assert where the permission actually bites)
    if std::fs::read(&secret).is_err() {
        let e = refusal(&b.run(&["note", "1", "--file", &secret]));
        assert!(e.contains("secret.txt': permission denied — make it readable"), "{e}");
    }

    assert_eq!(b.event_count(1), before, "no refusal wrote anything");
    assert_eq!(b.description(1), "the brief");
}

/// An empty file — or the empty standard input of an agent that forgot the pipe — must never
/// blank a description or log an empty note.
#[test]
fn empty_text_is_refused_not_stored() {
    let b = Board::new();
    b.ok(&["add", "docs: target", "-d", "the brief"]);
    let empty = b.file("empty.md", b"");
    let blank = b.file("blank.md", b" \n\t\n\n");
    for f in [&empty, &blank] {
        let e = refusal(&b.run(&["edit", "1", "--desc-file", f]));
        assert!(e.contains(".md' is empty — nothing to store"), "{e}");
    }
    // `-` with /dev/null on standard input: what a forgotten `<` looks like to an agent
    let e = refusal(&b.run(&["edit", "1", "--desc-file", "-"]));
    assert!(e.starts_with("tb: standard input is empty — nothing to store") && e.contains("'tb edit 1 --desc-file -'"), "{e}");
    let e = refusal(&b.run(&["note", "1", "--file", "-"]));
    assert!(e.contains("standard input is empty"), "{e}");
    let e = refusal(&b.run(&["add", "docs: another", "--desc-file", "-"]));
    assert!(e.contains("standard input is empty") && e.contains("'tb add \"tag: title\" --desc-file -'"), "{e}");
    assert_eq!(b.description(1), "the brief", "the description survived every refusal");
    assert!(b.notes(1).is_empty());
    assert!(b.ok(&["list", "--json"]).matches("\"id\"").count() == 1, "the refused add made no card");
    // clearing a description on purpose still works, the way it always did
    b.ok(&["edit", "1", "--desc", ""]);
    assert_eq!(b.description(1), "");
}

/// Text given twice is an argument error (exit 2), also under --json — and a plain
/// `--desc-file` without `--desc` is not mistaken for one (`-d` has a default value).
#[test]
fn text_given_twice_is_refused() {
    let b = Board::new();
    let f = b.file("brief.md", b"from the file");
    b.ok(&["add", "docs: target", "-d", "the brief"]);
    for args in [
        &["add", "docs: both", "-d", "inline", "--desc-file", &f][..],
        &["edit", "1", "--desc", "inline", "--desc-file", &f][..],
        &["note", "1", "inline", "--file", &f][..],
    ] {
        let o = b.run(args);
        assert_eq!(o.status.code(), Some(2), "{args:?} is a usage error");
        let e = String::from_utf8_lossy(&o.stderr);
        assert!(e.contains("cannot be used with") && e.contains("tb --help"), "{args:?}: {e}");
        let mut with_json = args.to_vec();
        with_json.push("--json");
        let o = b.run(&with_json);
        assert_eq!(o.status.code(), Some(2));
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["ok"], false, "{v}");
        assert!(v["error"].as_str().unwrap().starts_with("argument error:") && v["error"].as_str().unwrap().contains("cannot be used with"), "{v}");
        assert!(v["hint"].as_str().unwrap().contains("tb --help"), "{v}");
    }
    assert_eq!(b.description(1), "the brief");
    assert!(b.notes(1).is_empty());
    assert_eq!(b.ok(&["list", "--json"]).matches("\"id\"").count(), 1);
    // a title together with --desc-file is fine
    b.ok(&["edit", "1", "--title", "docs: renamed", "--desc-file", &f]);
    let c = b.card(1);
    assert_eq!((c["title"].as_str().unwrap(), c["description"].as_str().unwrap()), ("renamed", "from the file"));
}

/// A missing note text is still the same argument error, word for word.
#[test]
fn a_note_without_text_or_file_is_still_the_same_usage_error() {
    let b = Board::new();
    b.ok(&["add", "docs: target"]);
    let o = b.run(&["note", "1"]);
    assert_eq!(o.status.code(), Some(2));
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("the following required arguments were not provided:") && e.contains("<TEXT>"), "{e}");
    assert!(e.contains("Usage: tb note <ID> <TEXT>"), "{e}");
    // and the flag is discoverable from the one-screen help
    let help = b.ok(&["--help"]);
    assert!(help.contains("--desc-file PATH") && help.contains("note ID --file PATH") && help.contains("- = stdin"), "{help}");
}

/// A refused `add` must not leave a new, empty board file behind: the text is read before
/// the board is opened or created.
#[test]
fn a_refused_add_creates_no_board() {
    let b = Board::new();
    assert!(!b.db.exists());
    refusal(&b.run(&["add", "docs: first card", "--desc-file", "no-such-file.md"]));
    assert!(!b.db.exists(), "the board file was created by a command that was refused");
}

/// Hints name an explicitly chosen board, so a copied hint acts on the same board.
#[test]
fn hints_carry_an_explicit_board() {
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .current_dir(home.path())
            .env("HOME", home.path())
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .env_remove("TB_DB")
            .env_remove("TTYBOARD_DB")
            .env_remove("TB_BOARD")
            .env_remove("XDG_STATE_HOME")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    assert!(run(&["work", "add", "docs: on the work board"]).status.success());
    let e = refusal(&run(&["work", "note", "1", "--file", "missing.md"]));
    assert!(e.contains("'tb work note 1 --file PATH'") && e.contains("'tb work note 1 --file -'"), "{e}");
    let e = refusal(&run(&["-b", "work", "edit", "1", "--desc-file", "-"]));
    assert!(e.contains("'tb work edit 1 --desc-file -'"), "{e}");
}

/// Wait for `child` at most `limit`; kill it and return None when it is still running.
fn wait_at_most(mut child: std::process::Child, limit: Duration) -> Option<Output> {
    let start = Instant::now();
    loop {
        match child.try_wait().unwrap() {
            Some(_) => return Some(child.wait_with_output().unwrap()),
            None if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

/// tb never reads standard input unless `-` asked for it: a command given its text inline
/// returns at once even when standard input is a pipe nobody ever closes (an agent harness).
#[test]
fn an_open_pipe_on_stdin_never_stalls_a_command_that_did_not_ask_for_it() {
    let b = Board::new();
    b.ok(&["add", "docs: target"]);
    let f = b.file("brief.md", b"from the file");
    for args in [&["note", "1", "inline text"][..], &["note", "1", "--file", &f][..], &["edit", "1", "--desc-file", &f][..]] {
        let mut child = b.cmd(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let held_open = child.stdin.take().unwrap();
        let out = wait_at_most(child, Duration::from_secs(20));
        drop(held_open);
        let out = out.unwrap_or_else(|| panic!("{args:?} waited on a standard input it was never asked to read"));
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

/// #79: `--file -` on a pipe nobody ever closes hangs forever by default — never let that
/// stall an unattended agent loop with no way out. `TB_STDIN_TIMEOUT` is the opt-in: set, it
/// bounds the wait for `-`'s FIRST byte and refuses instead of hanging.
#[test]
fn tb_stdin_timeout_refuses_a_pipe_nobody_closes_instead_of_hanging() {
    let b = Board::new();
    b.ok(&["add", "docs: target"]);
    let mut cmd = b.cmd(&["note", "1", "--file", "-"]);
    cmd.env("TB_STDIN_TIMEOUT", "1").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let held_open = child.stdin.take().unwrap(); // never written to, never closed — the reproducer
    let out = wait_at_most(child, Duration::from_secs(10));
    drop(held_open);
    let out = out.unwrap_or_else(|| panic!("TB_STDIN_TIMEOUT=1 did not bound a pipe nobody closes"));
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("TB_STDIN_TIMEOUT") && err.contains("waited 1s"), "{err}");
    assert!(b.notes(1).is_empty(), "nothing was stored from a refused read");
}

/// #79: unset (the default), `TB_STDIN_TIMEOUT` never applies — a producer that is merely
/// slow to write its first byte is not mistaken for a hung one, however long it takes. This
/// is the failure mode the decision explicitly avoids: a wrong default would truncate this.
#[test]
fn without_tb_stdin_timeout_a_slow_starting_producer_is_never_truncated() {
    let b = Board::new();
    b.ok(&["add", "docs: target"]);
    let mut child = b.cmd(&["note", "1", "--file", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    // slower than the timeout the test above uses, on purpose: proves the default is unbounded
    std::thread::sleep(Duration::from_millis(1500));
    stdin.write_all(b"arrived late, on purpose").unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "a slow starter was refused although TB_STDIN_TIMEOUT is unset: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(b.notes(1), ["arrived late, on purpose"]);
}

/// #79 (found reviewing PR #95): a FIFO with no writer used to hang inside `open()`, before
/// any guard could run. Fixed the wrong way at first (SENT BACK): refusing every FIFO on file
/// type alone also refused one that DOES have a writer — a previously-working use case
/// (`mkfifo f; (echo hi > f &); tb note 1 --file f`). A second wrong fix (also caught before
/// landing): opening non-blocking and switching to blocking mode before reading LOOKS right
/// but is not — a FIFO read `open()` with `O_NONBLOCK` always succeeds at once whether or not
/// a writer exists, but a subsequent `read()`, even back in blocking mode, does NOT wait for
/// a writer that has not attached yet: with zero writers it returns EOF immediately, because
/// the kernel only blocks a read while a writer already holds the pipe open. The real fix
/// keeps the ordinary blocking `open()` (which DOES correctly wait for a writer, however long
/// that takes — unset `TB_STDIN_TIMEOUT`, nothing changes) and, only when `TB_STDIN_TIMEOUT`
/// is set, bounds that open with a deadline via a background thread + channel, since `open()`
/// itself takes no timeout parameter.
///
/// A writer, ready immediately: succeeds, default settings, no timeout needed.
#[test]
fn a_fifo_with_a_writer_still_succeeds() {
    let b = Board::new();
    b.ok(&["add", "docs: target"]);
    let fifo = b.dir.path().join("a-writer-is-here");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let mut writer = Command::new("sh").arg("-c").arg(format!("printf '%s' 'from the fifo' > '{}'", fifo.display())).spawn().unwrap();
    let o = b.run(&["note", "1", "--file", fifo.to_str().unwrap()]);
    assert!(writer.wait().unwrap().success());
    assert!(o.status.success(), "a FIFO with a writer was refused: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(b.notes(1), ["from the fifo"]);
}

/// A writer that attaches a moment later (not present when tb opens the FIFO): tb waits for
/// it rather than refusing immediately, the same way `-` waits for a slow-to-start producer.
#[test]
fn a_fifo_whose_writer_attaches_a_moment_later_still_succeeds() {
    let b = Board::new();
    b.ok(&["add", "docs: target"]);
    let fifo = b.dir.path().join("writer-attaches-late");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let mut writer = Command::new("sh")
        .arg("-c")
        .arg(format!("sleep 0.8 && printf '%s' 'arrived late, on purpose' > '{}'", fifo.display()))
        .spawn()
        .unwrap();
    let start = Instant::now();
    let o = b.run(&["note", "1", "--file", fifo.to_str().unwrap()]);
    assert!(writer.wait().unwrap().success());
    assert!(o.status.success(), "a writer that attaches a moment later was refused: {}", String::from_utf8_lossy(&o.stderr));
    assert!(start.elapsed() >= Duration::from_millis(750), "returned before the writer could plausibly have attached: {:?}", start.elapsed());
    assert_eq!(b.notes(1), ["arrived late, on purpose"]);
}

/// No writer, ever, and `TB_STDIN_TIMEOUT` set: refused promptly, not hung.
#[test]
fn a_fifo_with_no_writer_is_refused_promptly_under_tb_stdin_timeout() {
    let b = Board::new();
    b.ok(&["add", "docs: target"]);
    let fifo = b.dir.path().join("nobody-ever-writes-here");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let start = Instant::now();
    let mut cmd = b.cmd(&["note", "1", "--file", fifo.to_str().unwrap()]);
    cmd.env("TB_STDIN_TIMEOUT", "1");
    let o = cmd.output().unwrap();
    assert!(start.elapsed() < Duration::from_secs(5), "TB_STDIN_TIMEOUT did not bound a FIFO with no writer: took {:?}", start.elapsed());
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("TB_STDIN_TIMEOUT") && err.contains("waited 1s") && err.contains("writer") && err.contains("none did"), "{err}");
    assert!(b.notes(1).is_empty());
}

/// No writer, ever, and `TB_STDIN_TIMEOUT` UNSET: tb waits (today's, and always tb's,
/// default) rather than refusing on file type alone — the exact regression the FIFO fix was
/// sent back for. Bounded by `wait_at_most` so a regression here fails fast, not by hanging.
#[test]
fn without_tb_stdin_timeout_a_fifo_with_no_writer_waits_rather_than_refusing() {
    let b = Board::new();
    b.ok(&["add", "docs: target"]);
    let fifo = b.dir.path().join("nobody-ever-writes-here-either");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let child = b.cmd(&["note", "1", "--file", fifo.to_str().unwrap()]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let out = wait_at_most(child, Duration::from_secs(3));
    assert!(
        out.is_none(),
        "a FIFO with no writer and no TB_STDIN_TIMEOUT returned instead of waiting: {:?}",
        out.map(|o| String::from_utf8_lossy(&o.stderr).to_string())
    );
}

/// `script` runs a command with a real terminal on its standard input (its own pty).
fn under_a_terminal(tb_args: &[&str], db: &Path) -> Command {
    let tb = env!("CARGO_BIN_EXE_tb");
    let mut c = Command::new("script");
    if cfg!(target_os = "linux") {
        // util-linux: the command is one shell string; -e passes its exit code on
        let quoted: Vec<String> =
            std::iter::once(tb).chain(tb_args.iter().copied()).map(|a| format!("'{}'", a.replace('\'', "'\\''"))).collect();
        c.args(["-q", "-e", "-c", &quoted.join(" "), "/dev/null"]);
    } else {
        // BSD / macOS: script [-q] FILE COMMAND ARGS…
        c.args(["-q", "/dev/null", tb]).args(tb_args);
    }
    c.env("TB_DB", db).env("TB_AS", "tester").env("TB_NO_HERDR", "1").env_remove("TB_BOARD");
    c
}

/// With a TERMINAL on standard input, `-` is refused at once — tb never sits waiting for
/// typing (agents have no terminal; a command that waits never returns).
#[test]
fn a_terminal_on_standard_input_is_refused_not_waited_on() {
    let b = Board::new();
    b.ok(&["add", "docs: target", "-d", "the brief"]);
    for args in [&["note", "1", "--file", "-"][..], &["edit", "1", "--desc-file", "-"][..], &["add", "docs: x", "--desc-file", "-"][..]] {
        let mut c = under_a_terminal(args, &b.db);
        // script's own standard input: a pipe held open, so nothing ever sends an end-of-input
        // to the terminal — a tb that reads it would wait forever (and be killed below)
        let child = match c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(child) => child,
            // a machine without `script` cannot make a terminal; CI has one, so there a
            // missing `script` fails instead of silently skipping the check
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && std::env::var_os("CI").is_none() => {
                eprintln!("skipped: no `script` command here to make a terminal");
                return;
            }
            Err(e) => panic!("script: {e}"),
        };
        let mut child = child;
        let held_open = child.stdin.take().unwrap();
        let out = wait_at_most(child, Duration::from_secs(20));
        drop(held_open);
        let out = out.unwrap_or_else(|| panic!("{args:?} sat waiting on the terminal instead of refusing"));
        let seen = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(seen.contains("'-' reads piped text, but standard input is a terminal — pipe or redirect it:"), "{args:?}: {seen}");
        assert!(!out.status.success(), "{args:?} must fail: {seen}");
    }
    assert_eq!(b.description(1), "the brief");
    assert!(b.notes(1).is_empty());
    assert_eq!(b.ok(&["list", "--json"]).matches("\"id\"").count(), 1);
}

/// The agent manual and the JSON contract teach the feature.
#[test]
fn the_manuals_teach_it() {
    let agents = include_str!("../docs/AGENTS.md");
    for phrase in ["tb note ID --file", "--desc-file", "256 KiB", "never a terminal"] {
        assert!(agents.contains(phrase), "docs/AGENTS.md lacks `{phrase}`");
    }
    assert!(agents.lines().count() <= 250, "the agent manual is over its line cap");
    let json = include_str!("../docs/JSON.md");
    assert!(json.contains("--desc-file") && json.contains("note ID --file"), "docs/JSON.md");
    let humans = include_str!("../docs/HUMANS.md");
    for doc in [include_str!("../README.md"), humans] {
        assert!(doc.contains("--desc-file") && doc.contains("--file"), "README / docs/HUMANS.md");
    }
    // card #88: both manuals state that the text is trimmed and a leading byte-order mark is
    // dropped, not just "byte for byte" (which is what #95's reviewer flagged as missing). Short
    // distinctive phrases, not the exact sentence, so a reword survives but a deletion fails.
    for (name, doc) in [("docs/AGENTS.md", agents), ("docs/HUMANS.md", humans)] {
        assert!(doc.contains("trimmed"), "{name} does not say the text is trimmed");
        assert!(doc.contains("byte-order mark"), "{name} does not mention the byte-order mark");
    }
}
