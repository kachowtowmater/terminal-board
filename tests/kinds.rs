//! Board kinds: `tb new NAME --kind deadline` and `tb new NAME --from BOARD`.
//!
//! The panel's shape, pinned here: a kind is ONE named bundle of settings, and the KIND is
//! what the suite proves — the default kind renders exactly the board tb always made (the
//! existing goldens), and the deadline kind has its own golden. No test tries to cover the
//! 2^n boards the individual keys can make.
mod common;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::PathBuf;
use std::process::{Command, Output};
use terminal_board::store::Store;
use terminal_board::tui::{draw, App};

const NOW: i64 = 1_790_856_000; // 2026-10-01T12:00:00Z

/// Boards mode: a temp HOME with its own boards directory, so `tb new` really makes files.
struct Home {
    pub dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home { dir: tempfile::tempdir().unwrap() }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("HOME", self.dir.path())
            .env("TB_AS", "alice")
            .env("TB_NO_HERDR", "1")
            .env("TZ", "UTC")
            .env("TB_NOW", NOW.to_string())
            .env_remove("TB_DB")
            .env_remove("TTYBOARD_DB")
            .env_remove("XDG_STATE_HOME")
            .env_remove("TB_BOARD")
            .env_remove("HERDR_AGENT_NAME")
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let out = self.ok(args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    fn refused(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(1), "{args:?} should be refused: {}", String::from_utf8_lossy(&o.stdout));
        String::from_utf8_lossy(&o.stderr).trim().to_string()
    }

    fn db(&self, board: &str) -> PathBuf {
        self.dir.path().join(".local/state/terminal-board/boards").join(format!("{board}.db"))
    }

    /// The board's whole config table, as stored.
    fn config_rows(&self, board: &str) -> Vec<(String, String)> {
        let c = rusqlite::Connection::open(self.db(board)).unwrap();
        let mut st = c.prepare("SELECT key, value FROM config ORDER BY key").unwrap();
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        v
    }
}

/// THE RULE: a board of the default kind is the board tb always made — however it was made.
#[test]
fn the_default_kind_is_byte_for_byte_the_board_tb_always_made() {
    let h = Home::new();
    // three ways to the same board: `tb new`, `tb new --kind default`, and simply adding a card
    h.ok(&["new", "one"]);
    h.ok(&["new", "two", "--kind", "default"]);
    h.ok(&["three", "add", "x"]);
    h.ok(&["one", "add", "x"]);
    h.ok(&["two", "add", "x"]);
    assert_eq!(h.config_rows("one"), h.config_rows("three"), "`tb new` wrote something `tb add` does not");
    assert_eq!(h.config_rows("two"), h.config_rows("three"), "`--kind default` wrote something");
    assert_eq!(h.ok(&["one", "config"]), h.ok(&["three", "config"]));
    assert_eq!(h.ok(&["one", "board"]).replace("one", "X"), h.ok(&["three", "board"]).replace("three", "X"));
    assert!(!h.ok(&["one", "config"]).contains("kind"), "the default kind is not a setting anybody has to read");
    let v = h.json(&["new", "four", "--json"]);
    assert_eq!(v["ok"], true);
    assert_eq!((v["board"]["name"].as_str(), v["board"]["kind"].as_str()), (Some("four"), Some("default")));
    assert!(v["board"]["from"].is_null());
    assert_eq!(h.config_rows("four"), Vec::new(), "nothing at all is written");
}

#[test]
fn the_deadline_kind_writes_one_named_bundle_and_records_itself() {
    let h = Home::new();
    let out = h.ok(&["new", "filings", "--kind", "deadline"]);
    assert_eq!(
        out.trim(),
        "created board 'filings' of kind deadline — open it with 'tb filings', add work with 'tb filings add \"tag: title\"'"
    );
    assert_eq!(
        h.config_rows("filings"),
        [
            ("card-line", "due"),
            ("due-warn", "7"),
            ("kind", "deadline"),
            ("label.doing", "IN HAND"),
            ("label.done", "FILED"),
            ("label.review", "WITH REVIEWER"),
            ("label.todo", "TO PREPARE"),
            ("sort", "due"),
            ("waiting-lane", "shown"),
            ("wip-counts-blocked", "no"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()))
    );
    // every one of those is a setting that already existed and still answers for itself
    let cfg = h.ok(&["filings", "config"]);
    for want in ["sort          due", "card-line     due", "due-warn      7", "waiting-lane  shown", "kind          deadline"] {
        assert!(cfg.contains(want), "{want:?} missing:\n{cfg}");
    }
    assert_eq!(h.json(&["filings", "config", "--json"])["config"]["kind"], "deadline");
    // and the board behaves as the bundle says, with no extra machinery
    h.ok(&["filings", "add", "permits: renewal", "--due", "2026-10-20"]);
    h.ok(&["filings", "add", "tax: return", "--due", "2026-10-03"]);
    assert_eq!(h.json(&["filings", "board", "--json"])["columns"]["todo"][0]["id"], 2, "sort due");
    assert!(h.ok(&["filings", "board"]).contains("TO PREPARE (2) [todo] · by due"), "{}", h.ok(&["filings", "board"]));
    assert!(h.ok(&["filings", "list"]).contains("due Oct 3"), "card-line due");
}

/// A kind is a LABEL, not a lock: the settings always decide, and the name never lies.
#[test]
fn changing_a_setting_afterwards_keeps_working_and_is_shown() {
    let h = Home::new();
    h.ok(&["new", "filings", "--kind", "deadline"]);
    h.ok(&["filings", "config", "sort", "position"]);
    let cfg = h.ok(&["filings", "config"]);
    assert!(cfg.contains("sort          position"), "the setting decides:\n{cfg}");
    assert!(cfg.contains("kind          deadline (changed)"), "the name says it was changed:\n{cfg}");
    let v = h.json(&["filings", "config", "--json"]);
    assert_eq!((v["config"]["kind"].as_str(), v["config"]["kind_changed"].as_bool()), (Some("deadline"), Some(true)));
    // putting it back clears the mark
    h.ok(&["filings", "config", "sort", "due"]);
    assert!(h.ok(&["filings", "config"]).contains("kind          deadline\n"));
    // declaring a board a default one drops the name but never undoes a setting
    h.ok(&["filings", "config", "kind", "default"]);
    let cfg = h.ok(&["filings", "config"]);
    assert!(!cfg.contains("kind"), "{cfg}");
    assert!(cfg.contains("sort          due"), "settings are the user's, not the kind's:\n{cfg}");
    // and a kind can be applied to a board that already has cards, without touching them
    h.ok(&["filings", "add", "permits: renewal"]);
    h.ok(&["filings", "config", "kind", "deadline"]);
    assert!(h.ok(&["filings", "config"]).contains("kind          deadline"));
    assert!(h.ok(&["filings", "list"]).contains("#1 renewal"), "cards are untouched");
}

#[test]
fn from_copies_the_settings_and_never_the_cards() {
    let h = Home::new();
    h.ok(&["new", "filings", "--kind", "deadline"]);
    h.ok(&["filings", "config", "wip", "5"]);
    h.ok(&["filings", "config", "tz", "America/Los_Angeles"]);
    h.ok(&["filings", "add", "permits: renewal"]);
    let out = h.ok(&["new", "matters", "--from", "filings"]);
    assert_eq!(
        out.trim(),
        "created board 'matters' with the settings of 'filings' — open it with 'tb matters', add work with 'tb matters add \"tag: title\"'"
    );
    assert_eq!(h.config_rows("matters"), h.config_rows("filings"), "every setting travels");
    assert_eq!(h.ok(&["matters", "list"]), "no cards — add one with 'tb matters add \"tag: title\"'\n", "no card does");
    assert!(h.ok(&["matters", "config"]).contains("kind          deadline"), "a copy of a deadline board is one");
    let v = h.json(&["new", "third", "--from", "matters", "--json"]);
    assert_eq!((v["board"]["from"].as_str(), v["board"]["kind"].as_str()), (Some("matters"), Some("deadline")));
    assert_eq!(v["board"]["config"]["wip"], "5");
}

#[test]
fn tb_new_refuses_with_the_command_to_run_and_writes_nothing() {
    let h = Home::new();
    h.ok(&["new", "filings", "--kind", "deadline"]);
    for (args, want, code) in [
        (vec!["new", "filings"], "board 'filings' already exists", "invalid_value"),
        (vec!["new", "x", "--kind", "law"], "unknown kind 'law' — tb knows default and deadline", "invalid_value"),
        (vec!["new", "y", "--from", "nope"], "no board 'nope' to copy — boards: filings", "no_board"),
        (vec!["new", "Filings"], "is not a command or a valid board name", "invalid_board_name"),
        (vec!["new", "z", "--from", "z"], "'z' cannot copy itself", "invalid_value"),
    ] {
        let e = h.refused(&args);
        assert!(e.contains(want), "{args:?}: {e}");
        // #81: the same refusal under --json carries a stable code
        let mut jargs = args.clone();
        jargs.push("--json");
        let jo = h.run(&jargs);
        assert!(!jo.status.success(), "{jargs:?}");
        let jv: serde_json::Value = serde_json::from_slice(&jo.stdout).unwrap();
        assert_eq!(jv["code"], code, "{jargs:?}: {jv}");
    }
    let boards = h.ok(&["boards"]);
    assert!(boards.contains("filings") && !boards.contains(" x ") && !boards.contains(" y "), "nothing was created:\n{boards}");
    // `new` IS a legal board name (an older tb made such boards), so this one is made and
    // stays reachable — it is only the first WORD that belongs to the command
    h.ok(&["new", "new"]);
    assert!(h.ok(&["boards"]).contains("new"), "a board called new is listed");
    assert!(h.ok(&["-b", "new", "list"]).contains("no cards"), "and opens");
    // --kind and --from together is an argument error, not a silent winner
    let o = h.run(&["new", "w", "--kind", "deadline", "--from", "filings"]);
    assert_eq!(o.status.code(), Some(2));
}

/// D2 (review of #121): `tb new` under `TB_DB` used to write a kind's settings straight
/// into the pinned board — a board with real cards — and report success. `TB_DB` pins ONE
/// file: there is no board to make and none to copy, so it is refused like every other
/// command that names a board, and the pinned board is left exactly as it was.
#[test]
fn tb_new_is_refused_under_tb_db_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("pinned.db");
    let pinned = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("TB_DB", &db)
            .env("TB_AS", "alice")
            .env("TB_NO_HERDR", "1")
            .env("TZ", "UTC")
            .env("TB_NOW", NOW.to_string())
            .env_remove("TB_BOARD")
            .output()
            .unwrap()
    };
    assert!(pinned(&["add", "permits: renewal"]).status.success());
    let before_config = String::from_utf8(pinned(&["config"]).stdout).unwrap();
    let before_rows: Vec<(String, String)> = {
        let c = rusqlite::Connection::open(&db).unwrap();
        let mut st = c.prepare("SELECT key, value FROM config ORDER BY key").unwrap();
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        v
    };
    for args in [
        vec!["new", "anything", "--kind", "deadline"],
        vec!["new", "anything"],
        vec!["new", "anything", "--from", "doesnotexist"],
        vec!["new", "default", "--kind", "deadline"],
    ] {
        let o = pinned(&args);
        assert_eq!(o.status.code(), Some(1), "{args:?} was allowed: {}", String::from_utf8_lossy(&o.stdout));
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains("TB_DB is set — board names are ignored"), "{args:?}: {err}");
        assert!(!String::from_utf8_lossy(&o.stdout).contains("created board"), "{args:?} claimed to create one");
    }
    // the pinned board is untouched: same settings, same rows, same card
    assert_eq!(String::from_utf8(pinned(&["config"]).stdout).unwrap(), before_config);
    let after_rows: Vec<(String, String)> = {
        let c = rusqlite::Connection::open(&db).unwrap();
        let mut st = c.prepare("SELECT key, value FROM config ORDER BY key").unwrap();
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        v
    };
    assert_eq!(after_rows, before_rows, "a kind's settings were written into the pinned board");
    assert!(String::from_utf8_lossy(&pinned(&["list"]).stdout).contains("#1 renewal"), "the card is still there");
}

/// D3 (review of #121): a board called `new`, made by a version before `tb new` existed,
/// must not become unreachable. `new` is a command word but still a valid BOARD NAME.
#[test]
fn a_board_called_new_is_never_swallowed_by_the_command() {
    let h = Home::new();
    // made the way an older tb made it (that version had no `new` command, so its users
    // could and did call a board this; `-b` names it here because `new` is now a command word)
    h.ok(&["-b", "new", "add", "permits: renewal"]);
    assert!(h.db("new").exists(), "the file is there");
    // it is listed, and every way of naming it still opens it
    let boards = h.ok(&["boards"]);
    assert!(boards.contains("new"), "tb boards hides a board whose file is right there:\n{boards}");
    assert!(h.ok(&["-b", "new", "list"]).contains("#1 renewal"), "-b opens it");
    assert!(h.ok(&["-b", "new", "list"]).contains("#1 renewal"), "-b new");
    let by_env = Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(["list"])
        .env("HOME", h.dir.path())
        .env("TB_BOARD", "new")
        .env("TB_AS", "alice")
        .env("TB_NO_HERDR", "1")
        .env_remove("TB_DB")
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&by_env.stdout).contains("#1 renewal"), "TB_BOARD=new");
    // and `tb new` with no name says which command to run for that board
    let e = h.refused(&["new"]);
    assert!(e.contains("say what to call the board") && e.contains("'tb -b new'"), "{e}");
    // the command itself still works on other names
    h.ok(&["new", "filings", "--kind", "deadline"]);
    assert!(h.ok(&["boards"]).contains("filings") && h.ok(&["boards"]).contains("new"));
}

/// D4/D5 (review of #121): some settings belong to ONE board. `--from` must not wire a new
/// board to another board's repository, hand it another board's list of who may close a
/// card, or claim a file mode its own file does not have.
#[test]
fn from_leaves_behind_the_settings_that_belong_to_one_board() {
    let h = Home::new();
    h.ok(&["new", "filings", "--kind", "deadline"]);
    h.ok(&["filings", "config", "wip", "5"]);
    h.ok(&["filings", "config", "tz", "America/Los_Angeles"]);
    h.ok(&["filings", "config", "done-by", "anna,ben"]);
    // set the two that need no external check straight in the file
    {
        let c = rusqlite::Connection::open(h.db("filings")).unwrap();
        for (k, v) in [("github", "acme/widgets"), ("file-mode", "shared")] {
            c.execute("INSERT INTO config(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [k, v]).unwrap();
        }
    }
    let out = h.ok(&["new", "matters", "--from", "filings"]);
    assert!(out.contains("(not done-by, file-mode, github: each belongs to one board)"), "it says what it left: {out}");
    let copied: Vec<String> = h.config_rows("matters").into_iter().map(|(k, _)| k).collect();
    for never in ["github", "done-by", "file-mode"] {
        assert!(!copied.contains(&never.to_string()), "{never} was copied: {copied:?}");
    }
    // what a person copies a board FOR did travel
    for want in ["kind", "sort", "card-line", "due-warn", "waiting-lane", "wip-counts-blocked", "wip", "tz", "label.review"] {
        assert!(copied.contains(&want.to_string()), "{want} did not travel: {copied:?}");
    }
    let cfg = h.ok(&["matters", "config"]);
    assert!(cfg.contains("github        off"), "the new board is not wired to another board's repo:\n{cfg}");
    assert!(!cfg.contains("done-by"), "and does not decide who may close its cards:\n{cfg}");
    assert!(!cfg.contains("shared"), "and claims no file mode it does not have:\n{cfg}");
    assert_eq!(h.ok(&["matters", "config", "done-by"]).trim(), "anyone");
}

/// D6/D7 (review of #121): the default kind writes nothing, not even an event; and a machine
/// reading `config --json` gets the kind and whether it changed as two fields.
#[test]
fn the_default_kind_logs_nothing_and_json_splits_the_kind() {
    let h = Home::new();
    h.ok(&["new", "one"]);
    h.ok(&["two", "add", "x"]);
    h.ok(&["one", "add", "x"]);
    let events = |b: &str| -> i64 {
        rusqlite::Connection::open(h.db(b)).unwrap().query_row("SELECT COUNT(*) FROM board_events", [], |r| r.get(0)).unwrap()
    };
    assert_eq!(events("one"), events("two"), "`tb new` wrote a board event `tb add` does not");
    // a real kind IS worth a line in the board's log
    h.ok(&["new", "filings", "--kind", "deadline"]);
    assert_eq!(events("filings"), 1);
    // JSON: two fields, no string to parse
    let v = h.json(&["filings", "config", "--json"]);
    assert_eq!((v["config"]["kind"].as_str(), v["config"]["kind_changed"].as_bool()), (Some("deadline"), Some(false)));
    h.ok(&["filings", "config", "sort", "position"]);
    let v = h.json(&["filings", "config", "--json"]);
    assert_eq!((v["config"]["kind"].as_str(), v["config"]["kind_changed"].as_bool()), (Some("deadline"), Some(true)));
    assert!(h.ok(&["filings", "config"]).contains("kind          deadline (changed)"), "the plain listing is unchanged");
    // a default board has neither field
    let v = h.json(&["one", "config", "--json"]);
    assert!(v["config"]["kind"].is_null() && v["config"]["kind_changed"].is_null());
}

// ------------------------------------------------------------------ the deadline golden

/// The SECOND golden suite the panel asked for: a board made by `--kind deadline`, seeded
/// with the states a deadline board has. The default kind is covered by the goldens that
/// were already there, which this change leaves untouched.
#[test]
fn deadline_kind_126x41_golden() {
    common::pin_clock();
    std::env::set_var("TB_NOW", NOW.to_string());
    // the board is made and filled THROUGH THE BINARY, so this test needs nothing that only
    // this version's library has: on an older tb it fails at the first command, by assertion
    let h = Home::new();
    h.ok(&["new", "filings", "--kind", "deadline"]);
    h.ok(&["filings", "config", "tz", "UTC"]);
    h.ok(&["filings", "config", "wip", "4"]);
    h.ok(&["filings", "config", "github-panel", "hidden"]);
    h.ok(&["filings", "config", "agents-panel", "hidden"]);
    let cards: [(&str, &str, Option<&str>, Option<&str>); 8] = [
        ("permits: fire permit renewal", "todo", None, Some("2026-09-28")),
        ("tax: quarterly return", "todo", None, Some("2026-10-02")),
        ("lease: send the notice", "todo", None, Some("2026-11-16")),
        ("filing: annual report", "todo", None, None),
        ("audit: answer the letter", "doing", Some("bot-1"), Some("2026-10-01")),
        ("insurance: compare quotes", "doing", Some("bot-2"), None),
        ("grant: final report", "review", Some("bot-3"), Some("2026-10-04")),
        ("payroll: september", "done", Some("alice"), Some("2026-09-30")),
    ];
    for (i, (title, col, owner, due)) in cards.iter().enumerate() {
        let id = (i + 1).to_string();
        match due {
            Some(d) => h.ok(&["filings", "add", title, "--due", d]),
            None => h.ok(&["filings", "add", title]),
        };
        if *col != "todo" {
            let mut args = vec!["filings", "move", id.as_str(), *col, "--as", owner.unwrap_or("alice")];
            if *col == "done" {
                args.push("--force"); // fixture only: nothing reaches done except from review (verifier rule), so a card seeded straight into done is a forced move
            }
            h.ok(&args);
        }
    }
    h.ok(&["filings", "block", "7", "waiting for the signed copy", "--on", "#5", "--until", "2026-10-06", "--as", "bot-3"]);
    let s = Store::open(&h.db("filings")).unwrap().named("filings");
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(&s);
    let mut t = Terminal::new(TestBackend::new(126, 41)).unwrap();
    t.draw(|f| draw(f, &app)).unwrap();
    let b = t.backend().buffer();
    let screen: String =
        b.content.chunks(126).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n");
    let got = normalize(&screen);
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/deadline_kind_126x41.txt");
    if std::env::var("TB_UPDATE_GOLDEN").is_ok() {
        std::fs::write(path, format!("{got}\n")).unwrap();
    }
    let want = std::fs::read_to_string(path).expect("golden file (TB_UPDATE_GOLDEN=1 to create)");
    assert_eq!(got, want.trim_end_matches('\n'), "the deadline kind's look changed");
    // what the kind is FOR, said in words as well as pixels
    for want in ["o TO PREPARE (4) by due", "o IN HAND (2/4)", "o WITH REVIEWER (1) by due", "o FILED today (1)", "! overdue 3d", "! due today", "due Nov 16"] {
        assert!(got.contains(want), "{want:?} missing:\n{got}");
    }
}

/// Clock times become ##:## so a golden render is stable.
fn normalize(screen: &str) -> String {
    let mut c: Vec<char> = screen.chars().collect();
    let d = |x: char| x.is_ascii_digit();
    let mut i = 0;
    while i + 5 <= c.len() {
        if d(c[i]) && d(c[i + 1]) && c[i + 2] == ':' && d(c[i + 3]) && d(c[i + 4]) {
            for k in [0, 1, 3, 4] {
                c[i + k] = '#';
            }
            if i + 8 <= c.len() && c[i + 5] == ':' && d(c[i + 6]) && d(c[i + 7]) {
                c[i + 6] = '#';
                c[i + 7] = '#';
            }
        }
        i += 1;
    }
    c.into_iter().collect::<String>().lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
}

/// The docs teach the kind, not thirteen keys.
#[test]
fn the_docs_teach_running_a_deadline_board() {
    for (doc, needle) in [("README.md", "--kind deadline"), ("docs/HUMANS.md", "--kind deadline"), ("docs/AGENTS.md", "tb new")] {
        let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(doc)).unwrap();
        assert!(text.contains(needle), "{doc} does not teach `{needle}`");
    }
}
