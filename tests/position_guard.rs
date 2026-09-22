//! A `cards.position` that is not a number — written by something other than tb — must
//! refuse every read path with a house message that names the card, the value and the board
//! file, and the ONE command that fixes it. Never a raw database error (`next`, `list`,
//! `board`, `show`, the full-screen board, the picker, plain or `--json`), and never a
//! silent wrong order. A normal board renders exactly as before: NULL reads as 0 and a
//! foreign REAL in range orders, while a REAL outside i64 reads as its clamped value.
//!
//! Everything here drives the real `tb` binary and corrupts a board file the way a foreign
//! writer would: `UPDATE cards SET position=…` straight into SQLite.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use terminal_board::store::Store;

struct Board {
    _dir: tempfile::TempDir,
    db: PathBuf,
}

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("board.db");
        Board { _dir: dir, db }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("TB_DB", &self.db)
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .env_remove("HERDR_AGENT_NAME")
            .env_remove("TB_NOW")
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_str(&self.ok(args)).unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    /// stderr+stdout of a failing run (either channel may carry the message).
    fn text(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(!o.status.success(), "{args:?} should fail: {}", String::from_utf8_lossy(&o.stdout));
        format!("{}{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout))
    }

    /// The `--json` error envelope of a failing run: the object prints on STDOUT, exit 1.
    fn json_fail(&self, args: &[&str]) -> serde_json::Value {
        let o = self.run(args);
        assert!(!o.status.success(), "{args:?} should fail");
        let out = String::from_utf8(o.stdout).unwrap();
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    /// The board as the FULL-SCREEN board draws it, from a snapshot in this process.
    fn on_screen(&self, needle: &str) -> bool {
        // the TUI owns the terminal; tb prints the refusal on stderr and exits before drawing.
        // Draw what the TUI WOULD draw from a snapshot taken in-process: the snapshot call is
        // where the guard refuses.
        let store = Store::open(&self.db).unwrap();
        let snap = match store.snapshot() {
            Ok(s) => s,
            Err(e) => return e.to_string().contains(needle),
        };
        let app = terminal_board::tui::App::new(snap, "tester");
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(170, 60)).unwrap();
        t.draw(|f| terminal_board::tui::draw(f, &app)).unwrap();
        let buf = t.backend().buffer().clone();
        buf.content.iter().map(|c| c.symbol()).collect::<String>().contains(needle)
    }

    fn corrupt(&self, card: i64, expr: &str) {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        conn.execute(&format!("UPDATE cards SET position={expr} WHERE id={card}"), []).unwrap();
    }

    fn row(&self, card: i64) -> (String, i64) {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        conn.query_row("SELECT typeof(position), position FROM cards WHERE id=?", [card], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })
        .unwrap()
    }
}

/// The house refusal: names card, value, file — and the one sqlite3 line that fixes it.
/// The `— check TB_DB` tail may be split off into the JSON `hint`, so only the part before
/// it must appear whole.
fn assert_refusal(text: &str, db: &Path, card: i64, value: &str) {
    let expected = format!(
        "card #{card} has a position that is not a number ({value}) — {} was written by something other than tb; give card #{card} a whole-number position again with: sqlite3 \"{}\" \"UPDATE cards SET position=0 WHERE id={card}\"",
        db.display(),
        db.display()
    );
    assert!(
        text.contains(&expected),
        "no refusal naming the card, the value, the file and the fix:\n{text}\nwant: {expected}"
    );
    assert!(!text.contains("Invalid column type"), "raw database error surfaced:\n{text}");
}

fn corrupt_board() -> Board {
    let b = Board::new();
    b.ok(&["add", "alpha card"]);
    b.ok(&["add", "beta card"]);
    b.ok(&["add", "gamma card"]);
    b
}

#[test]
fn a_text_position_refuses_every_read_path_with_the_house_message() {
    let b = corrupt_board();
    b.corrupt(1, "'abc'");

    // the picker rows (tb boards) read every board; the full-screen board draws a snapshot
    for args in [
        vec!["list"],
        vec!["next"],
        vec!["show", "1"],
        vec!["board"],
    ] {
        let text = b.text(&args);
        assert_refusal(&text, &b.db, 1, "abc");
    }
    for args in [
        vec!["list", "--json"],
        vec!["next", "--json"],
        vec!["show", "1", "--json"],
        vec!["board", "--json"],
        vec!["--json"],
        vec!["boards", "--json"],
    ] {
        let v = b.json_fail(&args);
        assert_eq!(v["ok"], false, "{args:?}: {v}");
        let err = v["error"].as_str().unwrap_or_default();
        let hint = v["hint"].as_str().unwrap_or_default();
        let whole = format!("{err}\n{hint}");
        assert!(err.contains("card #1 has a position that is not a number (abc)"), "{args:?}: {v}");
        assert!(!whole.contains("Invalid column type"), "raw database error in {args:?}: {v}");
        assert!(hint.contains("UPDATE cards SET position=0 WHERE id=1"), "{args:?}: {v}");
    }
    assert!(b.on_screen("position that is not a number"), "the full-screen board refuses too");
}

#[test]
fn next_refuses_instead_of_handing_out_a_later_card() {
    // the order hole the one-ordering change left open: main's `tb next` SKIPS a card whose
    // row it cannot read when the skip is legitimate anyway (here: blocked) and hands out a
    // LATER card. An agent runs `tb next` blind; if the queue is unreadable the command must
    // refuse, never hand out what happens to parse.
    let b = corrupt_board();
    b.ok(&["block", "1", "#2"]);
    b.corrupt(1, "'abc'");
    let text = b.text(&["next"]);
    assert_refusal(&text, &b.db, 1, "abc");
    // nothing was claimed either: every read path refuses the same way
    assert_refusal(&b.text(&["list"]), &b.db, 1, "abc");
}

/// A board whose `position` lost its NOT NULL — what a foreign writer that stores NULL
/// would have: rebuild the table with a nullable column and put NULL on two cards.
///
/// The NULLs are on #1 (position 0) and #3 (position 2) on purpose. `by_position` breaks a
/// tie by id, so a NULL on the LOWEST id alone would keep its place whatever number it read:
/// #3 is the card that only reads as 0 if NULL really is 0 (as 1 it would tie with #2 and
/// fall behind it).
fn nullable_board() -> Board {
    let b = corrupt_board();
    let conn = rusqlite::Connection::open(&b.db).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE cards_null AS SELECT * FROM cards;
        DROP TABLE cards;
        CREATE TABLE cards (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL, tag TEXT, description TEXT NOT NULL DEFAULT '',
            "column" TEXT NOT NULL DEFAULT 'todo' CHECK ("column" IN ('todo','doing','review','done')),
            owner TEXT, due TEXT, gh_ref INTEGER, created_at INTEGER NOT NULL, column_since INTEGER NOT NULL,
            blocked TEXT, position INTEGER DEFAULT 0, reviewer TEXT, blocked_on TEXT, blocked_until TEXT
        );
        INSERT INTO cards SELECT * FROM cards_null;
        DROP TABLE cards_null;
        UPDATE cards SET position=NULL WHERE id IN (1, 3);
        "#,
    )
    .unwrap();
    b
}

#[test]
fn a_null_position_reads_as_zero_and_the_next_write_fixes_the_row() {
    // a foreign tool may relax NOT NULL; a card with no position reads as 0 (top of its
    // column, ties broken by id — the same value `add` would give the first card), every
    // read path works, and tb's next renumbering write gives the row a real number
    let b = nullable_board();

    // the value itself, not just an order that a tie-break could produce anyway: a NULL
    // position is read as ZERO, and nothing else
    let v = b.json(&["board", "--json"]);
    let todo = v["columns"]["todo"].as_array().unwrap();
    let read = |id: i64| todo.iter().find(|c| c["id"] == id).unwrap()["position"].as_i64().unwrap();
    assert_eq!(read(1), 0, "a NULL position reads as 0, not as anything else:\n{v}");
    assert_eq!(read(3), 0, "a NULL position reads as 0, not as anything else:\n{v}");
    assert_eq!(read(2), 1, "the card that was never corrupted keeps its own position:\n{v}");

    let listed = b.ok(&["list"]);
    let lines: Vec<&str> = listed.lines().collect();
    let at = |card: &str| lines.iter().position(|l| l.contains(card)).unwrap();
    let (at1, at2, at3) = (at("#1"), at("#2"), at("#3"));
    assert!(at1 < at2, "#1 reads as 0, first:\n{listed}");
    // #3 is the one that proves the number: it is last by id, so it only comes before #2
    // (position 1) if its NULL really read as 0
    assert!(at3 < at2, "#3 reads as 0 and sorts ahead of #2 at 1:\n{listed}");

    b.ok(&["prio", "1", "bottom"]); // a write that renumbers the column gives #1 a real position
    let (kind, value) = b.row(1);
    assert_eq!((kind.as_str(), value), ("integer", 2), "the row is whole again (bottom of three)");
}

#[test]
fn a_huge_real_position_is_a_whole_card_again_after_any_write() {
    // 2^63 is out of i64's range: the card orders last, not an error, and tb's next write
    // renumbers the column so the value is an in-range integer again
    let b = corrupt_board();
    b.corrupt(1, "9223372036854775808.0");
    b.ok(&["list"]);
    b.ok(&["prio", "1", "top"]);
    let (kind, value) = b.row(1);
    assert_eq!((kind.as_str(), value), ("integer", 0), "the row is whole again");
}

#[test]
fn a_real_position_inside_the_range_still_orders() {
    // a foreign writer that stored 1.5 does not break the board: the value is read (1), the
    // card orders between position 1 and 2, and nothing is refused
    let b = corrupt_board();
    b.corrupt(1, "1.5");
    b.ok(&["list"]);
    let v = b.json(&["board", "--json"]);
    let pos = v["columns"]["todo"].as_array().unwrap().iter().find(|c| c["id"] == 1).map(|c| c["position"].as_i64());
    assert_eq!(pos, Some(Some(1)), "1.5 reads as 1:\n{v}");
}

#[test]
fn a_text_position_fails_on_a_board_that_also_has_json_readers() {
    // the error object is the documented --json shape: error + hint + ok=false
    let b = corrupt_board();
    b.corrupt(1, "'abc'");
    let v = b.json_fail(&["board", "--json"]);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"].as_str().unwrap(), "database error: card #1 has a position that is not a number (abc)");
    assert!(v["hint"].as_str().unwrap().starts_with(&format!("{} was written by something other than tb", b.db.display())), "{v}");
    let hint = v["hint"].as_str().unwrap();
    assert!(hint.contains("give card #1 a whole-number position again"), "{hint}");
    assert!(hint.contains("UPDATE cards SET position=0 WHERE id=1"), "{hint}");
}

/// A board chosen the ordinary way — by NAME, or the default board — with no `TB_DB` in the
/// environment at all. tb is then the only one who knows where the file is, so the refusal
/// has to say: a repair command reading `sqlite3 "the board file" …` is a command nobody
/// can run.
struct NamedBoards {
    _dir: tempfile::TempDir,
    home: PathBuf,
}

impl NamedBoards {
    fn new() -> NamedBoards {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        NamedBoards { _dir: dir, home }
    }

    /// Where tb keeps the board called `name` — never named on the command line.
    fn db(&self, name: &str) -> PathBuf {
        self.home.join(".local/state/terminal-board/boards").join(format!("{name}.db"))
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("HOME", &self.home)
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .env_remove("TB_DB")
            .env_remove("TB_BOARD")
            .env_remove("TTYBOARD_DB")
            .env_remove("TTYBOARD_BOARD")
            .env_remove("HERDR_AGENT_NAME")
            .env_remove("TB_NOW")
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn text(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(!o.status.success(), "{args:?} should fail: {}", String::from_utf8_lossy(&o.stdout));
        format!("{}{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout))
    }

    fn json_fail(&self, args: &[&str]) -> serde_json::Value {
        let o = self.run(args);
        assert!(!o.status.success(), "{args:?} should fail");
        let out = String::from_utf8(o.stdout).unwrap();
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    fn corrupt(&self, name: &str, card: i64, expr: &str) {
        let conn = rusqlite::Connection::open(self.db(name)).unwrap();
        conn.execute(&format!("UPDATE cards SET position={expr} WHERE id={card}"), []).unwrap();
    }
}

#[test]
fn the_refusal_names_the_real_file_of_a_board_chosen_by_name() {
    let b = NamedBoards::new();
    b.ok(&["scratch", "add", "x: alpha card"]);
    b.corrupt("scratch", 1, "'abc'");

    for args in [vec!["scratch", "list"], vec!["scratch", "next"], vec!["-b", "scratch", "list"]] {
        let text = b.text(&args);
        assert_refusal(&text, &b.db("scratch"), 1, "abc");
        assert!(
            !text.contains("the board file"),
            "{args:?} printed a repair command nobody can run:\n{text}"
        );
    }
    let v = b.json_fail(&["scratch", "list", "--json"]);
    let hint = v["hint"].as_str().unwrap_or_default();
    assert!(
        hint.starts_with(&format!("{} was written by something other than tb", b.db("scratch").display())),
        "the JSON hint names the file too: {v}"
    );
    assert!(hint.contains(&format!("sqlite3 \"{}\"", b.db("scratch").display())), "{hint}");
}

#[test]
fn the_refusal_names_the_real_file_of_the_default_board() {
    // plain `tb` — no board name, no TB_BOARD, no TB_DB: the commonest case of all
    let b = NamedBoards::new();
    b.ok(&["add", "x: alpha card"]);
    b.corrupt("default", 1, "'abc'");

    for args in [vec!["list"], vec!["show", "1"], vec!["board"]] {
        let text = b.text(&args);
        assert_refusal(&text, &b.db("default"), 1, "abc");
        assert!(
            !text.contains("the board file"),
            "{args:?} printed a repair command nobody can run:\n{text}"
        );
    }
}
