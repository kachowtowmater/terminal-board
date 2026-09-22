//! `tb config sort position|due`: ONE order for `tb next`, `tb next --review`, `tb list`,
//! `tb board`, every `--json` board and the full-screen board.
//!
//! `tb next` is what an unattended agent works from, so the rules are strict:
//! - `position` (the default) is exactly the order tb always had;
//! - `due` puts the nearest due date first (an overdue card before everything), cards without
//!   a date after every dated card, and equal dates — or no dates — in `position` order, so
//!   the order is always deterministic;
//! - what `tb next` hands out is always the first unblocked card of the TODO column every
//!   other command shows, also when many agents race for it;
//! - the order never depends on what today is.
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use terminal_board::store::Store;
use terminal_board::tui::{draw, App};

const NOW: &str = "2026-10-01T12:00:00Z";

fn unix(rfc3339: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(rfc3339).unwrap_or_else(|e| panic!("{rfc3339}: {e}")).timestamp()
}

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

    fn cmd(&self, tz: &str, at: &str, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args)
            .env("TB_DB", &self.db)
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .env("TZ", tz)
            .env("TB_NOW", unix(at).to_string())
            .env_remove("TB_BOARD")
            .env_remove("HERDR_AGENT_NAME");
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd("UTC", NOW, args).output().unwrap()
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

    fn add(&self, title: &str, due: Option<&str>) {
        match due {
            Some(d) => self.ok(&["add", title, "--due", d]),
            None => self.ok(&["add", title]),
        };
    }

    /// The ids of `column` as each way of looking at the board gives them. All of them must
    /// be the same list, and that list is returned.
    fn column(&self, column: &str) -> Vec<i64> {
        let board = self.json(&["board", "--json"]);
        let ids = |v: &serde_json::Value| -> Vec<i64> { v.as_array().unwrap().iter().map(|c| c["id"].as_i64().unwrap()).collect() };
        let from_board_json = ids(&board["columns"][column]);
        assert_eq!(ids(&self.json(&["--json"])["columns"][column]), from_board_json, "bare `tb --json` vs `tb board --json` ({column})");
        // plain `tb list`: one line per card, `column #id …`
        let listed: Vec<i64> = self.ok(&["list"]).lines().filter(|l| l.starts_with(&format!("{column} "))).map(hash_id).collect();
        assert_eq!(listed, from_board_json, "plain `tb list` vs `tb board --json` ({column})");
        // plain `tb board`: the cards under the column's header, up to the next empty line
        let plain = self.ok(&["board"]);
        let header = format!("{} (", column.to_ascii_uppercase());
        let on_plain_board: Vec<i64> = plain
            .lines()
            .skip_while(|l| !l.starts_with(&header))
            .skip(1)
            .take_while(|l| !l.is_empty())
            .filter(|l| l.starts_with("  #"))
            .map(hash_id)
            .collect();
        if column != "done" {
            assert_eq!(on_plain_board, from_board_json, "plain `tb board` vs `tb board --json` ({column})");
        }
        from_board_json
    }

    /// `tb list --json`, the ids of one column in array order.
    fn list_json(&self, column: &str) -> Vec<i64> {
        let list = self.json(&["list", "--json"]);
        list.as_array().unwrap().iter().filter(|c| c["column"] == column).map(|c| c["id"].as_i64().unwrap()).collect()
    }

    /// The full-screen board, drawn in-process from this file: the ids of the cards whose
    /// titles appear on screen, top to bottom.
    fn on_screen(&self, titles: &[(i64, &str)]) -> Vec<i64> {
        let store = Store::open(&self.db).unwrap();
        let app = App::new(store.snapshot().unwrap(), "tester");
        let mut t = Terminal::new(TestBackend::new(170, 60)).unwrap();
        t.draw(|f| draw(f, &app)).unwrap();
        let buf = t.backend().buffer().clone();
        let rows: Vec<String> = buf
            .content
            .chunks(buf.area.width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect();
        let mut seen: Vec<(usize, i64)> = titles
            .iter()
            .filter_map(|(id, title)| rows.iter().position(|r| r.contains(&format!("#{id} {title}"))).map(|row| (row, *id)))
            .collect();
        seen.sort();
        seen.into_iter().map(|(_, id)| id).collect()
    }
}

/// `… #12 title …` -> 12
fn hash_id(line: &str) -> i64 {
    let at = line.find('#').unwrap_or_else(|| panic!("no #id in {line:?}"));
    line[at + 1..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap()
}

const TITLES: [(i64, &str); 7] =
    [(1, "alpha"), (2, "bravo"), (3, "charlie"), (4, "delta"), (5, "echo"), (6, "foxtrot"), (7, "golf")];

/// Seven TODO cards whose dates disagree with their positions, with a three-way tie and two
/// cards without a date. `prio 6 top` makes position differ from id order too.
///
/// position order: 6 1 2 3 4 5 7        due order: 3 (overdue) · 6 2 4 (same day, by
/// position) · 7 · then the undated 1 5 (by position)
fn queue() -> Board {
    let b = Board::new();
    b.ok(&["config", "wip", "9"]);
    b.add("alpha", None);
    b.add("bravo", Some("2026-10-09"));
    b.add("charlie", Some("2026-03-01"));
    b.add("delta", Some("2026-10-09"));
    b.add("echo", None);
    b.add("foxtrot", Some("2026-10-09"));
    b.add("golf", Some("2027-01-01"));
    b.ok(&["prio", "6", "top"]);
    b
}

const BY_POSITION: [i64; 7] = [6, 1, 2, 3, 4, 5, 7];
const BY_DUE: [i64; 7] = [3, 6, 2, 4, 7, 1, 5];

/// The rule for this whole series: a board that sets nothing behaves exactly as before — here
/// that means dates order NOTHING until the board asks for it.
#[test]
fn the_default_is_position_and_dates_order_nothing() {
    let b = queue();
    assert_eq!(b.ok(&["config", "sort"]).trim(), "position", "unset = position");
    assert_eq!(b.json(&["config", "sort", "--json"]), serde_json::json!({"ok": true, "config": {"key": "sort", "value": "position"}}));
    assert_eq!(b.json(&["board", "--json"])["sort"], "position");
    assert_eq!(b.column("todo"), BY_POSITION, "every view is in position order");
    assert_eq!(b.on_screen(&TITLES), BY_POSITION, "the full-screen board too");
    assert_eq!(b.list_json("todo"), [1, 2, 3, 4, 5, 6, 7], "`tb list --json` stays in id order, as it always was");
    assert!(!b.ok(&["config"]).contains("sort"), "a setting the board never set is not listed");
    // `tb next` takes the TOP card — the one with no date at all here — not the overdue one
    for want in BY_POSITION {
        assert_eq!(b.json(&["next", "--json"])["card"]["id"], want, "tb next under the default sort");
    }
    // and `tb prio` reports exactly what it always did
    let b = queue();
    assert_eq!(b.ok(&["prio", "7", "top"]), "#7 is now at position 1 in todo\n");
    let v = b.json(&["prio", "7", "bottom", "--json"]);
    assert_eq!(v.as_object().unwrap().keys().cloned().collect::<Vec<_>>(), ["card", "ok"], "no extra keys: {v}");
}

#[test]
fn sort_due_is_nearest_first_then_undated_and_ties_keep_position_in_every_view() {
    let b = queue();
    let v = b.json(&["config", "sort", "due", "--json"]);
    assert_eq!(v, serde_json::json!({"ok": true, "config": {"key": "sort", "value": "due"}}));
    assert_eq!(b.json(&["board", "--json"])["sort"], "due");
    assert_eq!(b.column("todo"), BY_DUE, "list, board and both JSON boards");
    assert_eq!(b.on_screen(&TITLES), BY_DUE, "the full-screen board");
    assert_eq!(b.list_json("todo"), BY_DUE, "`tb list --json` follows the board's order once the board sorts by due");
    // re-dating a card moves it; clearing its date sends it behind every dated card
    b.ok(&["edit", "5", "--due", "2026-01-15"]);
    assert_eq!(b.column("todo"), [5, 3, 6, 2, 4, 7, 1]);
    b.ok(&["edit", "3", "--due", "none"]);
    assert_eq!(b.column("todo"), [5, 6, 2, 4, 7, 1, 3], "undated: after the dated ones, by position (#1 is above #3)");
    assert_eq!(b.on_screen(&TITLES), [5, 6, 2, 4, 7, 1, 3]);
    // back to position: the dates order nothing again
    b.ok(&["config", "sort", "position"]);
    assert_eq!(b.column("todo"), BY_POSITION);
    assert_eq!(b.list_json("todo"), [1, 2, 3, 4, 5, 6, 7]);
}

/// The agent loop: whatever `tb next` hands out is the first unblocked TODO card every view
/// shows — checked before every single take, until the column is empty.
#[test]
fn tb_next_takes_the_nearest_due_unblocked_card_and_never_disagrees_with_the_board() {
    let b = queue();
    b.ok(&["config", "sort", "due"]);
    b.ok(&["block", "3", "waiting for the signed copy"]); // the overdue card is blocked: skipped, not lost
    b.ok(&["block", "2", "#3"]);
    let mut handed_out = Vec::new();
    loop {
        let board = b.json(&["board", "--json"]);
        let shown_first = board["columns"]["todo"].as_array().unwrap().iter().find(|c| c["blocked"].is_null()).map(|c| c["id"].as_i64().unwrap());
        let o = b.run(&["next", "--json", "--as", "agent"]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        match shown_first {
            Some(id) => {
                assert_eq!(v["card"]["id"], id, "tb next took a card the board does not show first: {v}");
                handed_out.push(id);
            }
            None => {
                assert_eq!(v["ok"], false, "nothing unblocked is left: {v}");
                break;
            }
        }
    }
    assert_eq!(handed_out, [6, 4, 7, 1, 5], "nearest date first, ties by position, undated last, blocked skipped");
    assert_eq!(b.column("todo"), [3, 2], "the blocked cards are still there, still in due order");
    b.ok(&["block", "3", "--clear"]);
    assert_eq!(b.json(&["next", "--json", "--as", "agent"])["card"]["id"], 3, "unblocked: the overdue card is next");
}

#[test]
fn next_review_follows_the_same_order() {
    let b = Board::new();
    b.ok(&["config", "sort", "due"]);
    for (title, due, worker) in [
        ("alpha", Some("2026-12-01"), "w1"),
        ("bravo", Some("2026-10-05"), "w2"),
        ("charlie", None, "w3"),
        ("delta", Some("2026-10-05"), "rev"), // the reviewer's own work: never handed to them
        ("echo", Some("2026-09-01"), "w4"),
    ] {
        b.add(title, due);
        let id = b.json(&["list", "--json"]).as_array().unwrap().len().to_string();
        b.ok(&["take", &id, "--as", worker]);
        b.ok(&["done", &id, "--as", worker]);
    }
    assert_eq!(b.column("review"), [5, 2, 4, 1, 3], "REVIEW: nearest date first, same day by position, undated last");
    let mut claimed = Vec::new();
    loop {
        let shown_first = b.json(&["board", "--json"])["columns"]["review"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["reviewer"].is_null() && c["owner"] != "rev")
            .map(|c| c["id"].as_i64().unwrap());
        let o = b.run(&["next", "--review", "--json", "--as", "rev"]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        match shown_first {
            Some(id) => {
                assert_eq!(v["card"]["id"], id, "next --review disagrees with the board: {v}");
                claimed.push(id);
            }
            None => {
                assert_eq!(v["ok"], false, "{v}");
                assert!(v["hint"].as_str().unwrap().contains("your own work"), "{v}");
                break;
            }
        }
    }
    assert_eq!(claimed, [5, 2, 1, 3]);
}

/// Dates compare as dates. What today is — the clock, the board's `tz`, the zone of whoever
/// runs the command — decides `days_left`, never the order.
#[test]
fn the_order_never_depends_on_today() {
    let b = queue();
    b.ok(&["config", "sort", "due"]);
    let todo_at = |tz: &str, at: &str| -> Vec<i64> {
        let o = b.cmd(tz, at, &["board", "--json"]).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        v["columns"]["todo"].as_array().unwrap().iter().map(|c| c["id"].as_i64().unwrap()).collect()
    };
    for board_tz in ["local", "Pacific/Kiritimati", "America/Los_Angeles"] {
        b.ok(&["config", "tz", board_tz]);
        for process_tz in ["UTC", "Pacific/Auckland", "Pacific/Pago_Pago"] {
            // long before every date, on the tie date at 23:59:59, after all but one, after all
            for at in ["2020-01-01T00:00:00Z", "2026-10-09T23:59:59Z", "2026-12-31T23:59:59Z", "2031-06-01T00:00:00Z"] {
                assert_eq!(todo_at(process_tz, at), BY_DUE, "board tz {board_tz}, TZ={process_tz}, now {at}");
            }
        }
    }
    // when every card is overdue the nearest — most overdue — date still goes first
    let o = b.cmd("Pacific/Auckland", "2031-06-01T00:00:00Z", &["next", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["card"]["id"], 3, "{v}");
    assert_eq!(v["card"]["due_state"], "overdue");
}

#[test]
fn doing_stays_in_position_order_and_done_newest_first() {
    let b = queue();
    b.ok(&["config", "sort", "due"]);
    for id in ["7", "1", "3"] {
        b.ok(&["take", id]); // taken in this order = DOING positions 0 1 2
    }
    assert_eq!(b.column("doing"), [7, 1, 3], "DOING is work in hand, not a queue: position order");
    assert_eq!(b.ok(&["prio", "3", "top"]), "#3 is now at position 1 in doing\n", "prio in DOING means what it always meant");
    assert_eq!(b.column("doing"), [3, 7, 1]);
    // DONE: newest first, whatever the dates (the clock moves a minute per card)
    for (id, at) in [("2", "2026-10-01T12:01:00Z"), ("5", "2026-10-01T12:02:00Z"), ("4", "2026-10-01T12:03:00Z")] {
        let o = b.cmd("UTC", at, &["move", id, "done"]).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    assert_eq!(b.column("done"), [4, 5, 2], "a finished card's date orders nothing");
}

/// Position is still the tie-break on a due-sorted board, so `tb prio` still works — and it
/// says what it did and did not do, instead of seeming to do nothing.
#[test]
fn prio_says_what_it_means_on_a_due_sorted_board() {
    let b = queue();
    b.ok(&["config", "sort", "due"]);
    // #7 is the only card due that day: its position changes, its place cannot
    let out = b.ok(&["prio", "7", "top"]);
    assert_eq!(
        out,
        "#7 is now at position 1 in todo — this board sorts by due date, so position only orders cards with the same date (or none): #7 is 5 of 7 in todo (unchanged); its date decides the rest — 'tb edit 7 --due DATE'\n"
    );
    assert_eq!(b.json(&["show", "7", "--json"])["position"], 0, "the position did change");
    assert_eq!(b.column("todo"), BY_DUE, "and the order did not");
    // #4 shares its date with #6 and #2: inside that group position decides, so it moves
    let out = b.ok(&["prio", "4", "top"]);
    assert!(out.contains("#4 is 2 of 7 in todo (was 4)"), "{out}");
    assert_eq!(b.column("todo"), [3, 4, 6, 2, 7, 1, 5]);
    // undated cards are a tie group too
    let v = b.json(&["prio", "5", "top", "--json"]);
    assert_eq!(v["ok"], true);
    assert_eq!(v["card"]["position"], 0);
    let note = v["note"].as_str().unwrap_or_else(|| panic!("--json carries the same explanation as `note`: {v}"));
    assert!(note.contains("this board sorts by due date") && note.contains("#5 is 6 of 7 in todo (was 7)") && note.contains("'tb edit 5 --due DATE'"), "{note}");
    assert_eq!(b.column("todo"), [3, 4, 6, 2, 7, 5, 1]);
    // the full-screen board says it as well (shift+down / J on the selected card)
    let mut store = Store::open(&b.db).unwrap();
    let mut app = App::new(store.snapshot().unwrap(), "tester");
    app.handle_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE), &mut store);
    let (status, is_error) = app.status.clone().expect("a status line");
    assert!(!is_error && status.contains("sorted by due date") && status.contains("same date"), "{status}");
}

#[test]
fn the_full_screen_board_reports_a_reorder_as_before_without_sort_due() {
    let b = queue();
    let mut store = Store::open(&b.db).unwrap();
    let mut app = App::new(store.snapshot().unwrap(), "tester");
    app.handle_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE), &mut store);
    assert_eq!(app.status.clone().map(|s| s.0).as_deref(), Some("#6 moved down"));
}

#[test]
fn sort_is_set_read_listed_and_refused_from_the_cli() {
    let b = Board::new();
    assert_eq!(b.ok(&["config", "sort", "due"]).lines().count(), 1);
    assert_eq!(b.ok(&["config", "sort"]).trim(), "due");
    assert!(b.ok(&["config"]).contains(&format!("{:<13} {}", "sort", "due")), "listed once set");
    assert_eq!(b.json(&["config", "--json"])["config"]["sort"], "due");
    for bad in ["date", "deadline", "", "due,position"] {
        let o = b.run(&["config", "sort", bad, "--json"]);
        assert_eq!(o.status.code(), Some(1), "sort {bad:?}");
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().starts_with("unknown sort"), "{v}");
        let hint = v["hint"].as_str().unwrap();
        assert!(hint.contains("'tb config sort position'") && hint.contains("'tb config sort due'"), "{v}");
    }
    let o = b.run(&["config", "sort", "--off"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("'tb config sort position'"));
    assert_eq!(b.ok(&["config", "sort"]).trim(), "due", "a refused value changed nothing");
    assert_eq!(b.ok(&["config", "sort", "Position"]).lines().count(), 1);
    assert_eq!(b.ok(&["config", "sort"]).trim(), "position");
}

/// Text in the due column that is not a date (another writer's) orders as "no date".
#[test]
fn free_text_in_the_due_column_sorts_as_undated() {
    let b = queue();
    b.ok(&["config", "sort", "due"]);
    rusqlite::Connection::open(&b.db).unwrap().execute("UPDATE cards SET due='ASAP' WHERE id=3", []).unwrap();
    assert_eq!(b.column("todo"), [6, 2, 4, 7, 1, 3, 5], "#3 now sits with the undated cards, by position");
}

/// The atomic claim survives: agents racing `tb next` — real processes, started together —
/// each get a DIFFERENT card, and the cards go out strictly in due order.
#[test]
fn racing_agents_each_get_a_different_card_and_the_nearest_dates_go_first() {
    const RACERS: usize = 8;
    let b = Board::new();
    b.ok(&["config", "wip", "99"]);
    b.ok(&["config", "sort", "due"]);
    // (due, blocked) in position order: dates run against position, with ties and gaps
    let cards: [(Option<&str>, bool); 20] = [
        (None, false),
        (Some("2026-11-20"), false),
        (Some("2026-10-02"), true), // the nearest date, blocked: nobody may get it
        (Some("2026-12-24"), false),
        (None, false),
        (Some("2026-10-05"), false),
        (Some("2026-11-20"), false), // tie with #2: #2 goes first
        (Some("2026-10-03"), true),  // blocked too
        (Some("2026-10-05"), false), // tie with #6
        (Some("2027-03-01"), false),
        (Some("2026-09-15"), false), // overdue: the very first
        (None, false),
        (Some("2026-10-30"), false),
        (Some("2026-10-05"), false), // three-way tie with #6 and #9
        (Some("2026-11-01"), false),
        (None, false),
        (Some("2026-10-12"), false),
        (Some("2026-09-28"), false), // overdue too
        (Some("2026-12-24"), false), // tie with #4
        (Some("2026-10-20"), false),
    ];
    for (i, (due, blocked)) in cards.iter().enumerate() {
        b.add(&format!("card {}", i + 1), *due);
        if *blocked {
            b.ok(&["block", &(i + 1).to_string(), "waiting"]);
        }
    }
    // the oracle, independent of tb: dated before undated, by date, then by position (= id here)
    let mut expected: Vec<i64> = (1..=20).filter(|id| !cards[*id as usize - 1].1).collect();
    expected.sort_by_key(|id| (cards[*id as usize - 1].0.is_none(), cards[*id as usize - 1].0, *id));
    assert_eq!(expected.len(), 18);
    assert_eq!(&expected[..4], [11, 18, 6, 9]);

    let mut taken_by: std::collections::BTreeMap<i64, String> = Default::default();
    for wave in 0..3 {
        let children: Vec<(String, std::process::Child)> = (0..RACERS)
            .map(|i| {
                let name = format!("agent-{wave}-{i}");
                let child = b.cmd("UTC", NOW, &["next", "--json", "--as", &name]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
                (name, child)
            })
            .collect();
        let mut got = Vec::new();
        for (name, child) in children {
            let o = child.wait_with_output().unwrap();
            let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{name}: {e}: {o:?}"));
            if v["ok"] == true {
                let id = v["card"]["id"].as_i64().unwrap();
                assert_eq!(v["card"]["owner"], name.as_str(), "{name} was handed a card owned by someone else");
                assert!(taken_by.insert(id, name.clone()).is_none(), "#{id} was handed out twice (wave {wave})");
                got.push(id);
            } else {
                assert!(v["error"].as_str().unwrap().contains("no todo cards"), "{name}: {v}");
            }
        }
        // whoever won which card, each wave took exactly the nearest dates that were left
        got.sort_by_key(|id| expected.iter().position(|e| e == id));
        let from = wave * RACERS;
        let want = &expected[from.min(expected.len())..(from + RACERS).min(expected.len())];
        assert_eq!(got, want, "wave {wave}");
    }
    assert_eq!(taken_by.len(), 18, "every unblocked card went out exactly once");
    // strictly in due order: the claims are serialised, and each one took the card on top
    let conn = rusqlite::Connection::open(&b.db).unwrap();
    let order: Vec<i64> = conn
        .prepare("SELECT card_id FROM events WHERE kind='taken' ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(order, expected, "the order in which the racers' claims landed");
    assert_eq!(b.column("todo"), [3, 8], "only the blocked cards are left");
}
