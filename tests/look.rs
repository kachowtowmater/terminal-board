//! The board's look: `card-line age|due`, the loud due mark, column display labels, and the
//! header note on a date-ordered column.
//!
//! LABELS ARE CHROME: they change what a person reads and nothing else. JSON `column` and
//! every command keep the internal names `todo`, `doing`, `review`, `done`.
//! A board that sets nothing renders byte for byte as before (the first golden and the
//! main-vs-branch sweep); a board with the settings on has its own golden here.
mod common;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use terminal_board::herdr::{parse_agents, AgentsState};
use terminal_board::plain::meta_fit;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App};

/// 2026-10-01T12:00:00Z — every test in this binary runs at this instant, in-process too.
const NOW: i64 = 1_790_856_000;

fn pin() {
    use std::sync::Once;
    static PIN: Once = Once::new();
    PIN.call_once(|| {
        std::env::set_var("TB_NOW", NOW.to_string());
        std::env::set_var("TZ", "UTC");
    });
}

struct Board {
    _dir: tempfile::TempDir,
    db: PathBuf,
}

impl Board {
    fn new() -> Board {
        pin();
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
            .env("TZ", "UTC")
            .env("TB_NOW", NOW.to_string())
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
}

/// Settings written straight into the file: the in-process tests need no API that an older
/// tb lacks, so on an older tb they fail by what they SEE, not by what they cannot compile.
fn set(db: &Path, key: &str, value: &str) {
    rusqlite::Connection::open(db)
        .unwrap()
        .execute("INSERT INTO config(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [key, value])
        .unwrap();
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(w as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// Clock times (HH:MM and HH:MM:SS) become ##:## so a golden render is stable.
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

// ---------------------------------------------------------------- labels are chrome

#[test]
fn a_label_changes_what_people_read_and_nothing_a_command_or_json_says() {
    let b = Board::new();
    b.ok(&["add", "permits: send the renewal"]);
    b.ok(&["take", "1"]);
    assert_eq!(b.ok(&["config", "label", "review"]).trim(), "REVIEW", "no label: the plain name");
    assert_eq!(
        b.ok(&["config", "label", "review", "WITH REVIEWER"]).trim(),
        "review is now shown as WITH REVIEWER — display only: commands and JSON still say review"
    );
    assert_eq!(b.ok(&["config", "label", "review"]).trim(), "WITH REVIEWER");
    assert_eq!(
        b.json(&["config", "label", "REVIEW", "--json"]),
        serde_json::json!({"ok": true, "config": {"key": "label.review", "value": "WITH REVIEWER"}})
    );
    assert!(b.ok(&["config"]).contains(&format!("{:<13} {}", "label.review", "WITH REVIEWER")), "listed once set");

    // every command still takes ONLY the internal name — and says so to someone who types the label
    let e = b.refused(&["move", "1", "with reviewer"]);
    assert_eq!(e, "tb: 'with reviewer' is a display label, not a column — the column is review: 'tb move 1 review'");
    assert_eq!(b.json(&["show", "1", "--json"])["column"], "doing", "nothing moved");
    // an answer that names a column names the one to type, with the label after it
    assert_eq!(b.ok(&["move", "1", "review"]).trim(), "#1 is now in review (shown as WITH REVIEWER)");
    b.ok(&["move", "1", "doing", "needs the fee"]);
    assert!(b.ok(&["done", "1"]).starts_with("#1 is now in review (shown as WITH REVIEWER) — close it with 'tb done 1'"));

    // JSON: `column` is the internal name for ever; `column_label` is the additive display text
    for v in [b.json(&["show", "1", "--json"]), b.json(&["list", "--json"])[0].clone(), b.json(&["board", "--json"])["columns"]["review"][0].clone()] {
        assert_eq!((v["column"].as_str(), v["column_label"].as_str()), (Some("review"), Some("WITH REVIEWER")), "{v}");
    }
    let board = b.json(&["board", "--json"]);
    assert_eq!(board["v"], 1);
    assert_eq!(board["labels"], serde_json::json!({"todo": "TODO", "doing": "DOING", "review": "WITH REVIEWER", "done": "DONE"}));
    assert_eq!(board["columns"].as_object().unwrap().keys().cloned().collect::<Vec<_>>(), ["doing", "done", "review", "todo"]);

    // plain text: `tb list` keeps the internal name in front; the board header shows both
    assert!(b.ok(&["list"]).starts_with("review  #1 send the renewal"), "{}", b.ok(&["list"]));
    assert!(b.ok(&["board"]).contains("\nWITH REVIEWER (1) [review]\n"), "{}", b.ok(&["board"]));

    // clearing it puts everything back
    assert_eq!(b.ok(&["config", "label", "review", "--off"]).trim(), "review is shown as REVIEW again");
    assert_eq!(b.json(&["show", "1", "--json"])["column_label"], "REVIEW");
    assert!(b.ok(&["board"]).contains("\nREVIEW (1)\n"));
    assert!(!b.ok(&["config"]).contains("label"), "cleared = no longer listed");
}

/// THE DEFECT (PR #105 review): a label that is a column's internal name used to shadow that
/// column — `config label todo done` was accepted, and `tb move 1 done` was then refused with
/// "the column is todo", sending a card to the wrong place and misdirecting the person.
/// Both ends are closed: such a label is refused, and an internal name always wins anyway.
#[test]
fn a_label_can_never_shadow_a_column_that_a_command_takes() {
    let cols = ["todo", "doing", "review", "done"];
    // (a) every label/column pair: setting a label that IS a column name is refused
    for on in cols {
        for name in cols {
            for typed in [name.to_string(), name.to_ascii_uppercase(), format!("  {name} ")] {
                let b = Board::new();
                b.ok(&["add", "permits: send the renewal"]);
                let o = b.run(&["config", "label", on, &typed, "--json"]);
                assert_eq!(o.status.code(), Some(1), "label {on} {typed:?} was accepted");
                let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
                assert_eq!(v["ok"], false);
                assert!(v["error"].as_str().unwrap().contains("is the name of a column, so it cannot be a label"), "{v}");
                assert!(v["hint"].as_str().unwrap().contains(&format!("'tb config label {on} \"TEXT\"'")), "{v}");
                assert_eq!(b.ok(&["config", "label", on]).trim(), on.to_ascii_uppercase(), "nothing was stored");
                // and the column is still reachable
                assert_eq!(b.json(&["board", "--json"])["labels"][on], on.to_ascii_uppercase());
            }
        }
    }
    // (b) a board file that ALREADY holds such a label (an older tb, or another writer):
    // every command still reaches every column, and no answer misdirects
    for on in cols {
        for shadowed in cols {
            let b = Board::new();
            b.ok(&["add", "permits: send the renewal"]);
            set(&b.db, &format!("label.{on}"), shadowed);
            // the legal way round the whole board, every column named by its internal name
            for step in [vec!["move", "1", "doing"], vec!["move", "1", "review"], vec!["move", "1", "done", "--as", "checker"], vec!["move", "1", "todo"]] {
                let target = step[2];
                let o = b.run(&step);
                assert!(o.status.success(), "label.{on}={shadowed}: {step:?} was refused: {}", String::from_utf8_lossy(&o.stderr));
                assert_eq!(b.json(&["show", "1", "--json"])["column"], target, "label.{on}={shadowed}: {step:?} went to the wrong column");
            }
        }
    }
    // a label that is NOT a column name still behaves as it should
    let b = Board::new();
    b.ok(&["add", "permits: send the renewal"]);
    b.ok(&["config", "label", "review", "WITH REVIEWER"]);
    assert!(b.refused(&["move", "1", "WITH REVIEWER"]).contains("is a display label, not a column — the column is review"));
    assert!(b.run(&["move", "1", "review"]).status.success());
}

/// The other commands that take a column, checked for the same precedence bug. `tb move` is
/// the only one that takes a typed column name; the full-screen board moves a card by column
/// INDEX from a key, and `tb config label` resolves internal names only — so no label can
/// reach any of them. This test fails if a future command starts taking one.
#[test]
fn no_other_command_takes_a_column_name_a_label_could_shadow() {
    let b = Board::new();
    b.ok(&["add", "permits: send the renewal"]);
    set(&b.db, "label.todo", "done");
    set(&b.db, "label.doing", "review");
    // every command in the manual that names a column, run with a column word
    for args in [
        vec!["move", "1", "doing"],
        vec!["move", "1", "review"],
        vec!["move", "1", "done", "--as", "checker"],
        vec!["move", "1", "todo"],
        vec!["config", "label", "done"],
        vec!["config", "label", "review"],
        vec!["config", "label", "todo"],
        vec!["config", "label", "doing"],
    ] {
        let o = b.run(&args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    }
    // `config label COLUMN` reads the column named, never the one a label points at
    assert_eq!(b.ok(&["config", "label", "todo"]).trim(), "done");
    assert_eq!(b.ok(&["config", "label", "done"]).trim(), "DONE");
    // and the help never offers a label where a column is expected
    let help = b.ok(&["--help"]);
    assert!(help.contains("move ID todo|doing|review|done"), "{help}");
    // the full-screen board moves by column index, not by a typed name: a clashing label
    // cannot reach it (it draws and moves the card one column right, as always)
    let mut store = Store::open(&b.db).unwrap();
    let mut app = App::new(store.snapshot().unwrap(), "tester");
    app.handle_key(
        ratatui::crossterm::event::KeyEvent::new(ratatui::crossterm::event::KeyCode::Char('>'), ratatui::crossterm::event::KeyModifiers::NONE),
        &mut store,
    );
    assert_eq!(store.card(1).unwrap().column, "doing");
}

#[test]
fn tb_show_carries_the_same_due_mark_as_the_board() {
    let b = dated();
    // #1 is overdue, #3 is soon, #4 is far, #7 is done and late
    assert!(b.ok(&["show", "1"]).contains(" - due 2026-09-28 - ! overdue 3d"), "{}", b.ok(&["show", "1"]));
    assert!(b.ok(&["show", "2"]).contains("! due today"));
    assert!(b.ok(&["show", "3"]).contains("! due in 2d"));
    assert!(!b.ok(&["show", "4"]).contains('!'), "a date that is far off is not marked:\n{}", b.ok(&["show", "4"]));
    assert!(!b.ok(&["show", "7"]).contains('!'), "a finished card is never marked:\n{}", b.ok(&["show", "7"]));
    // next to a block, both are shown, in the board's order
    b.ok(&["block", "1", "#3"]);
    assert!(b.ok(&["show", "1"]).contains("! overdue 3d - x blocked by #3"), "{}", b.ok(&["show", "1"]));
    // a board with no due dates prints exactly what it always did
    let plain = Board::new();
    plain.ok(&["add", "ops: rotate API tokens"]);
    assert_eq!(plain.ok(&["show", "1"]), "#1 rotate API tokens\ntodo - ops - unowned - 0m\n\n12:00 added by tester\n");
}

#[test]
fn a_label_is_sanitised_bounded_and_refused_with_the_command_to_run() {
    let b = Board::new();
    // control characters and escape sequences never reach the file, the screen or JSON
    b.ok(&["config", "label", "todo", "IN\x1b[31mBOX\x07   NOW"]);
    assert_eq!(b.ok(&["config", "label", "todo"]).trim(), "INBOX NOW");
    assert_eq!(b.json(&["board", "--json"])["labels"]["todo"], "INBOX NOW");
    // wide characters are fine
    b.ok(&["config", "label", "done", "完了 ✅"]);
    assert_eq!(b.ok(&["config", "label", "done"]).trim(), "完了 ✅");
    for (args, error, hint) in [
        (vec!["config", "label", "review", "a label that is much too long to be a header"], "that label is 44 characters, the limit is 24", "'tb config label review \"TEXT\"'"),
        (vec!["config", "label", "review", "  \x1b[0m "], "the label is empty", "'tb config label review --off'"),
        (vec!["config", "label", "inbox", "X"], "unknown column 'inbox'", "'tb config label review \"WITH REVIEWER\"'"),
        (vec!["config", "label"], "say which column", "'tb config label review \"WITH REVIEWER\"'"),
        (vec!["config", "label", "review", "X", "--off"], "a label and --off together", "'tb config label review --off'"),
        (vec!["config", "wip", "3", "4"], "'wip' takes one value", "'tb config label review \"WITH REVIEWER\"'"),
        (vec!["config", "card-line", "date"], "unknown card-line 'date'", "'tb config card-line due'"),
        (vec!["config", "card-line", "--off"], "--off does not go with card-line", "'tb config card-line age'"),
    ] {
        let mut with_json = args.clone();
        with_json.push("--json");
        let o = b.run(&with_json);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["ok"], false, "{args:?}");
        assert_eq!(v["error"], error, "{args:?}");
        assert!(v["hint"].as_str().unwrap().contains(hint), "{args:?}: {v}");
    }
    assert_eq!(b.ok(&["config", "label", "review"]).trim(), "REVIEW", "a refused label changed nothing");
}

// ---------------------------------------------------------------- card line + the due mark

/// today = 2026-10-01 (UTC, pinned); due-warn 3.
fn dated() -> Board {
    let b = Board::new();
    b.ok(&["config", "tz", "UTC"]);
    b.ok(&["config", "wip", "9"]);
    for (title, due) in [
        ("permits: overdue", Some("2026-09-28")),
        ("permits: today", Some("2026-10-01")),
        ("permits: soon", Some("2026-10-03")),
        ("permits: far", Some("2026-10-19")),
        ("permits: next year", Some("2027-01-05")),
        ("permits: undated", None),
        ("permits: finished late", Some("2026-09-01")),
    ] {
        match due {
            Some(d) => b.ok(&["add", title, "--due", d]),
            None => b.ok(&["add", title]),
        };
    }
    b.ok(&["move", "7", "done"]);
    b
}

#[test]
fn the_due_mark_is_loud_in_tb_list_for_soon_and_overdue_and_never_on_done() {
    let b = dated();
    assert_eq!(b.ok(&["config", "card-line"]).trim(), "age", "the default");
    assert_eq!(
        b.ok(&["list"]),
        "todo    #1 overdue  [permits - 0m - due 2026-09-28  ! overdue 3d]\n\
         todo    #2 today  [permits - 0m - due 2026-10-01  ! due today]\n\
         todo    #3 soon  [permits - 0m - due 2026-10-03  ! due in 2d]\n\
         todo    #4 far  [permits - 0m - due 2026-10-19]\n\
         todo    #5 next year  [permits - 0m - due 2027-01-05]\n\
         todo    #6 undated  [permits - 0m]\n\
         done    #7 finished late  [permits - due 2026-09-01]\n"
    );
    // `card-line due`: the date and the days left stand where the age was
    assert_eq!(b.json(&["config", "card-line", "due", "--json"]), serde_json::json!({"ok": true, "config": {"key": "card-line", "value": "due"}}));
    assert_eq!(b.ok(&["config", "card-line"]).trim(), "due");
    assert_eq!(
        b.ok(&["list"]),
        "todo    #1 overdue  [permits - due Sep 28  ! overdue 3d]\n\
         todo    #2 today  [permits - due Oct 1  ! due today]\n\
         todo    #3 soon  [permits - due Oct 3  ! due in 2d]\n\
         todo    #4 far  [permits - due Oct 19 - 18d]\n\
         todo    #5 next year  [permits - due 2027-01-05 - 96d]\n\
         todo    #6 undated  [permits - 0m]\n\
         done    #7 finished late  [permits - due Sep 1]\n"
    );
    assert!(b.ok(&["board"]).contains("  #1 overdue\n      permits - due Sep 28  ! overdue 3d\n"), "{}", b.ok(&["board"]));
    // the mark follows `due-warn` and the board's today, like `due_state` does
    b.ok(&["config", "due-warn", "20"]);
    assert!(b.ok(&["list"]).contains("#4 far  [permits - due Oct 19  ! due in 18d]"));
    b.ok(&["config", "card-line", "age"]);
    assert!(b.ok(&["list"]).contains("#4 far  [permits - 0m - due 2026-10-19  ! due in 18d]"));
}

/// The mark is the point of a deadline board: on a narrowing card line it outlives the age,
/// the tag, the checklist count — everything but itself — and is never cut inside a word.
#[test]
fn the_due_mark_outlives_the_age_on_a_narrow_card_line() {
    let b = dated();
    b.ok(&["take", "1", "--as", "alice"]);
    b.ok(&["check", "1", "--add", "fee paid"]);
    let store = Store::open(&b.db).unwrap();
    let snap = store.snapshot().unwrap();
    let card = store.card(1).unwrap();
    let whole_forms = ["! overdue 3d", "! late 3d", "! late", "!"];
    let mut age_seen = false;
    for width in (1..=60).rev() {
        let (base, warn) = meta_fit(&card, &snap, width);
        assert!(whole_forms.contains(&warn.as_str()), "width {width}: the mark is {warn:?} — gone, or cut inside a word");
        let line = if base.is_empty() { warn.clone() } else { format!("{base} {warn}") };
        assert!(line.chars().count() <= width, "width {width}: {line:?}");
        age_seen |= base.contains("0m");
    }
    assert!(age_seen, "at full width the age is on the line too");
    let (base, warn) = meta_fit(&card, &snap, 14);
    assert_eq!((base.as_str(), warn.as_str()), ("alice", "! late"), "the age went, the mark stayed");
}

// ---------------------------------------------------------------- the full-screen board

const AGENTS: &str = r#"{"result":{"agents":[
  {"name":"bot-2","agent":"aider","agent_status":"working","pane_id":"w:p5"},
  {"name":"alice","agent":"claude","agent_status":"idle","pane_id":"w:p2"}
]}}"#;

/// A deadline board with everything on: `card-line due`, three labels, `sort due`, and cards
/// that are overdue, due today, soon, far, undated, blocked and done. Cards are added in due
/// order, so the picture is the same whether or not the board's order follows the dates.
fn deadline_board() -> (tempfile::TempDir, Store) {
    pin();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let mut s = Store::open(&db).unwrap();
    s.set_wip(4).unwrap();
    s.set_tz("UTC").unwrap();
    s.set_panel("github-panel", "hidden").unwrap();
    s.set_panel("agents-panel", "hidden").unwrap();
    let cards: [(&str, &str, Option<&str>, Option<&str>); 9] = [
        ("permits: fire permit renewal", "todo", None, Some("2026-09-28")),
        ("tax: quarterly return", "todo", None, Some("2026-10-02")),
        ("lease: send the notice", "todo", None, Some("2026-11-16")),
        ("filing: annual report", "todo", None, None),
        ("audit: answer the letter", "doing", Some("bot-1"), Some("2026-10-01")),
        ("insurance: compare quotes", "doing", Some("bot-2"), None),
        ("grant: final report", "review", Some("bot-3"), Some("2026-10-04")),
        ("licence: renewal form", "review", Some("bot-1"), Some("2026-10-30")),
        ("payroll: september", "done", Some("alice"), Some("2026-09-30")),
    ];
    for (title, col, owner, due) in cards {
        let id = s.add(title, "", &[], "alice").unwrap();
        if let Some(d) = due {
            let date = terminal_board::store::due::DueDate::parse(d, "x").unwrap();
            s.set_due(id, date.as_ref(), "alice").unwrap();
        }
        if col != "todo" {
            s.move_to(id, col, owner.unwrap_or("alice")).unwrap();
        }
    }
    s.block(7, Some("#5"), "alice").unwrap();
    set(&db, "card-line", "due");
    set(&db, "sort", "due");
    set(&db, "label.todo", "INTAKE");
    set(&db, "label.review", "WITH THE REVIEWER");
    set(&db, "label.done", "FILED");
    (dir, s)
}

/// The SECOND golden: a board with the settings on. (The first golden,
/// `half_h_126x41.txt`, is a board that sets nothing, and this change does not touch it.)
#[test]
fn deadline_board_126x41_golden() {
    let (_d, s) = deadline_board();
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(&s);
    let got = normalize(&render(&app, 126, 41));
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/deadline_126x41.txt");
    if std::env::var("TB_UPDATE_GOLDEN").is_ok() {
        std::fs::write(path, format!("{got}\n")).unwrap();
    }
    let want = std::fs::read_to_string(path).expect("golden file (TB_UPDATE_GOLDEN=1 to create)");
    assert_eq!(got, want.trim_end_matches('\n'), "the deadline board's look changed");
    // what the golden has to show, said in words
    for want in [
        "o INTAKE (4) by due ━", // a date-ordered column says so
        "o DOING (2/4) ─",
        "o WITH THE REVIEWER (2) ─", // no room left for `by due`: it goes whole, the label and the count stay
        "o FILED today (1) ─",
        "due Sep 28 ! overdue 3d",
        "due Oct 2 ! due in 1d",
        "bot-1 ! due today",
        "lease - due Nov 16 - 46d",
        "! due 3d x blocked by #5",
        "filing - 0m", // no date: its age, as always
    ] {
        assert!(got.contains(want), "{want:?} missing:\n{got}");
    }
    assert!(got.contains("payroll - due Sep 30  ") && !got.contains("payroll - due Sep 30 !"), "a finished card carries no mark");
}

/// Labels and marks at every width and in every view: nothing panics, a label is never cut
/// inside a word, the count is never pushed off a header, and the mark is whole.
#[test]
fn labels_and_marks_degrade_in_whole_words_at_every_size() {
    let (_d, s) = deadline_board();
    let names: [(&str, Vec<&str>); 4] = [
        ("TODO", vec!["INTAKE"]),
        ("DOING", vec![]),
        ("REVIEW", vec!["WITH THE REVIEWER", "WITH THE", "WITH"]),
        ("DONE", vec!["FILED today", "FILED"]),
    ];
    // after `!`: a word of a mark, or the `x` that starts the next warning (`! x blocked by #5`)
    let mark_words = ["overdue", "late", "due", "in", "today", "1d", "3d", "x"];
    let (mut full_label, mut short_label, mut plain_instead, mut marks) = (0, 0, 0, 0);
    for layout in ["auto", "focus", "third-h", "third-v", "half-h", "half-v"] {
        s.set_layout(layout).unwrap();
        let mut app = App::new(s.snapshot().unwrap(), "alice");
        app.reload(&s);
        for h in [8u16, 14, 24, 41] {
            for w in 20u16..=160 {
                let screen = render(&app, w, h);
                for line in screen.lines() {
                    // a header: ` o NAME (count)` on a column, ` o NAME ` on the focus card
                    if let Some(at) = line.find(" o ") {
                        let rest: String = line[at + 3..].chars().take_while(|c| !"─━│┃┐┓┌┏".contains(*c)).collect();
                        let name = rest.split(" (").next().unwrap_or("").trim();
                        let of_label = names.iter().flat_map(|(_, l)| l.first()).any(|full| !name.is_empty() && full.starts_with(name));
                        if rest.starts_with("REVIEW (") {
                            plain_instead += 1;
                        }
                        if of_label {
                            full_label += usize::from(name == "WITH THE REVIEWER");
                            short_label += usize::from(name == "WITH THE" || name == "WITH");
                            let whole = names.iter().any(|(_, labels)| labels.contains(&name));
                            assert!(whole, "{layout} {w}x{h}: the label is cut inside a word: {name:?}\n{line}");
                            if let Some(open) = rest.find(" (") {
                                let count: String = rest[open + 2..].chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect();
                                assert!(!count.is_empty() && rest[open..].contains(')'), "{layout} {w}x{h}: the count was pushed off:\n{line}");
                            }
                        }
                    }
                    // a due mark: `!` and then only whole words of a mark
                    if let Some(at) = line.find("! ") {
                        let word: String = line[at + 2..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                        marks += 1;
                        let cut = line[at + 2 + word.len()..].starts_with('…');
                        assert!(!cut && (word.is_empty() || mark_words.contains(&word.as_str())), "{layout} {w}x{h}: a mark cut inside a word:\n{line}");
                    }
                }
            }
        }
    }
    // the sweep really met every stage of the degrade, and marks at all
    assert!(full_label > 0 && short_label > 0 && plain_instead > 0, "label stages seen: whole {full_label}, fewer words {short_label}, plain name {plain_instead}");
    assert!(marks > 1000, "due marks seen: {marks}");
}

/// A stored `tz` this build does not know is said out loud, never silently replaced.
#[test]
fn an_unknown_stored_tz_is_reported_not_silently_ignored() {
    let b = dated();
    set(&b.db, "tz", "Mars/Olympus_Mons");
    let o = b.run(&["list"]);
    assert!(o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("tz 'Mars/Olympus_Mons' is not a time zone this version knows") && err.contains("'tb config tz local'"), "{err}");
    let o = b.run(&["list", "--json"]);
    assert!(serde_json::from_slice::<serde_json::Value>(&o.stdout).is_ok(), "stdout stays clean JSON");
    b.ok(&["config", "tz", "UTC"]);
    assert!(b.run(&["list"]).stderr.is_empty(), "fixed: quiet again");
}

/// `--due` accepts shell padding and stores the date without it — and says so in the docs.
#[test]
fn a_padded_due_date_is_stored_without_the_padding() {
    let b = Board::new();
    assert_eq!(b.json(&["add", "x", "--due", "  2026-10-09 ", "--json"])["card"]["due"], "2026-10-09");
    for doc in ["README.md", "docs/JSON.md", "docs/HUMANS.md"] {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(doc)).unwrap();
        assert!(text.contains("spaces around it"), "{doc} does not say that spaces around a date are dropped");
    }
}

/// The agent manual says it once, and mentions the settings that decide today and soon.
#[test]
fn the_agent_manual_says_internal_names_are_the_api() {
    let b = Board::new();
    let guide = b.ok(&["guide"]);
    assert!(guide.contains("internal names are the API") && guide.contains("labels are chrome"), "one line in the manual");
    assert!(guide.contains("tb config tz") && guide.contains("due-warn"), "tb guide mentions tz and due-warn");
    assert!(guide.lines().count() <= 250);
}

// ---------------------------------------------------------------- the render sweep (a tool)

/// Not an assertion but the tool behind one: with `TB_SWEEP_OUT=FILE` it writes every render
/// of a board that sets NOTHING — every width from 30 to 200, several heights, all six
/// layouts, both themes of the wide ones — to FILE. Run on two trees, the files must be
/// identical: that is the main-vs-branch proof that a no-settings board did not move.
#[test]
fn render_sweep_dump() {
    let Ok(out) = std::env::var("TB_SWEEP_OUT") else { return };
    pin();
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    common::seed(&s, "alice").unwrap();
    let mut dump = String::new();
    let mut n = 0;
    for layout in ["auto", "focus", "third-h", "third-v", "half-h", "half-v"] {
        s.set_layout(layout).unwrap();
        for agents in [false, true] {
            let mut app = App::new(s.snapshot().unwrap(), "alice");
            app.reload(&s);
            if agents {
                app.agents = AgentsState::Agents(parse_agents(AGENTS, None).unwrap());
            }
            for h in [10u16, 14, 24, 41, 60] {
                for w in 30u16..=200 {
                    dump.push_str(&format!("=== {layout} agents={agents} {w}x{h}\n{}\n", normalize(&render(&app, w, h))));
                    n += 1;
                }
            }
        }
    }
    std::fs::write(&out, dump).unwrap();
    eprintln!("render sweep: {n} renders -> {out}");
}
