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
/// would have: rebuild the table with a nullable column and put NULL on one card.
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
            blocked TEXT, position INTEGER DEFAULT 0, reviewer TEXT
        );
        INSERT INTO cards SELECT * FROM cards_null;
        DROP TABLE cards_null;
        UPDATE cards SET position=NULL WHERE id=1;
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
    let listed = b.ok(&["list"]);
    let lines: Vec<&str> = listed.lines().collect();
    let at1 = lines.iter().position(|l| l.contains("#1")).unwrap();
    let at2 = lines.iter().position(|l| l.contains("#2")).unwrap();
    assert!(at1 < at2, "#1 reads as 0, first:\n{listed}");
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
