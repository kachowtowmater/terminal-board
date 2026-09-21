//! Due dates: `--due` on add and edit, the `tz` and `due-warn` settings, and the derived
//! `days_left` / `due_state` JSON fields.
//!
//! The hard requirement: a due date is a LOCAL CALENDAR DATE. The text the user typed is what
//! is stored and what every command reports, in every time zone, at every time of day and
//! across DST changes — it must never shift a day by passing through UTC. "Today" (which
//! decides `days_left` and `due_state`) turns over at LOCAL midnight in the board's `tz`,
//! not at UTC midnight and not in the zone of whoever runs the command.
//!
//! Everything here drives the real binary with `TZ` and the pinned clock `TB_NOW`.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Process zones the suite runs under: UTC, both sides of it, a half-hour offset, and the
/// two ends of the map (+14 and -11), so "today" differs between them for most of any day.
const ZONES: [&str; 6] =
    ["UTC", "America/Los_Angeles", "Pacific/Auckland", "Asia/Kolkata", "Pacific/Kiritimati", "Pacific/Pago_Pago"];

/// Hostile instants: the second before and the first second of each DST change in Los
/// Angeles and Auckland, 23:59:59 local on 2026-10-09 in every zone above, UTC midnight,
/// and the last second of a year.
const INSTANTS: [&str; 15] = [
    "2026-03-08T09:59:59Z", // Los Angeles 01:59:59 PST — a second later it is 03:00 PDT
    "2026-03-08T10:00:00Z",
    "2026-11-01T08:59:59Z", // Los Angeles 01:59:59 PDT — a second later it is 01:00 PST
    "2026-11-01T09:00:00Z",
    "2026-04-04T13:59:59Z", // Auckland 02:59:59 NZDT — a second later it is 02:00 NZST
    "2026-09-26T14:00:00Z", // Auckland 03:00 NZDT, the first second of summer time
    "2026-10-09T23:59:59Z", // 23:59:59 on the 9th in UTC
    "2026-10-10T06:59:59Z", // … in Los Angeles
    "2026-10-09T10:59:59Z", // … in Auckland
    "2026-10-09T18:29:59Z", // … in Kolkata
    "2026-10-09T09:59:59Z", // … in Kiritimati
    "2026-10-10T10:59:59Z", // … in Pago Pago
    "2026-10-09T00:00:00Z", // UTC midnight
    "2026-12-31T23:59:59Z",
    "2027-01-01T00:00:00Z",
];

const DATES: [&str; 4] = ["2026-10-09", "2026-03-08", "2026-11-01", "2028-02-29"];

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

    /// Run `tb` in process zone `tz` with the clock pinned at `at` — always under `TB_DB`.
    fn run(&self, tz: &str, at: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("TB_DB", &self.db)
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .env("TZ", tz)
            .env("TB_NOW", unix(at).to_string())
            .env_remove("TB_BOARD")
            .env_remove("HERDR_AGENT_NAME")
            .output()
            .unwrap()
    }

    fn ok(&self, tz: &str, at: &str, args: &[&str]) -> String {
        let o = self.run(tz, at, args);
        assert!(o.status.success(), "[TZ={tz} now={at}] {args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, tz: &str, at: &str, args: &[&str]) -> serde_json::Value {
        let out = self.ok(tz, at, args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("[TZ={tz} now={at}] {args:?}: {e}: {out}"))
    }

    /// (`due`, `days_left`, `due_state`) of card `id`, which `show`, `list` and `board` must
    /// all agree on.
    fn due_of(&self, tz: &str, at: &str, id: i64) -> (serde_json::Value, serde_json::Value, serde_json::Value) {
        let pick = |c: &serde_json::Value| (c["due"].clone(), c["days_left"].clone(), c["due_state"].clone());
        let ctx = format!("[TZ={tz} now={at}] #{id}");
        let show = self.json(tz, at, &["show", &id.to_string(), "--json"]);
        for field in ["due", "days_left", "due_state"] {
            assert!(show.as_object().unwrap().contains_key(field), "{ctx}: show --json has no `{field}`: {show}");
        }
        let list = self.json(tz, at, &["list", "--json"]);
        let listed = list.as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("{ctx} not in list"));
        assert_eq!(pick(listed), pick(&show), "{ctx}: list --json disagrees with show --json");
        let board = self.json(tz, at, &["board", "--json"]);
        let on_board = ["todo", "doing", "review", "done"]
            .iter()
            .flat_map(|col| board["columns"][*col].as_array().unwrap().iter())
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("{ctx} not on the board"));
        assert_eq!(pick(on_board), pick(&show), "{ctx}: board --json disagrees with show --json");
        pick(&show)
    }

    /// The `due` column exactly as it sits in the file.
    fn raw_due(&self, id: i64) -> Option<String> {
        raw(&self.db).query_row("SELECT due FROM cards WHERE id=?", [id], |r| r.get(0)).unwrap()
    }
}

fn raw(db: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(db).unwrap()
}

/// The `due` of every card, by id, as `list --json` reports it.
fn listed_dues(b: &Board, tz: &str, at: &str) -> Vec<serde_json::Value> {
    let list = b.json(tz, at, &["list", "--json"]);
    let mut cards: Vec<&serde_json::Value> = list.as_array().unwrap().iter().collect();
    cards.sort_by_key(|c| c["id"].as_i64());
    cards.iter().map(|c| c["due"].clone()).collect()
}

/// Add one card per date under (`zone`, `at`), then read, re-date and re-read them: the text
/// that comes back, and the text in the file, is the text that went in.
fn round_trip_in(zone: &str) {
    for at in INSTANTS {
        let b = Board::new();
        let ctx = format!("[TZ={zone} now={at}]");
        for (i, date) in DATES.iter().enumerate() {
            let id = i as i64 + 1;
            let v = b.json(zone, at, &["add", &format!("filing {id}"), "--due", date, "--json"]);
            assert_eq!(v["card"]["due"], *date, "{ctx} the add result already shifted the date");
            assert_eq!(b.raw_due(id).as_deref(), Some(*date), "{ctx} the stored text is not what was typed");
        }
        assert_eq!(listed_dues(&b, zone, at), DATES, "{ctx} list --json");
        // show, list and board agree, in JSON and in plain text
        assert_eq!(b.due_of(zone, at, 2).0, DATES[1], "{ctx} #2 shifted");
        assert!(b.ok(zone, at, &["show", "2"]).contains(&format!("due {}", DATES[1])), "{ctx} plain show");
        // re-date through edit, and back
        let v = b.json(zone, at, &["edit", "1", "--due", "2026-12-31", "--json"]);
        assert_eq!(v["card"]["due"], "2026-12-31", "{ctx} edit shifted the date");
        assert_eq!(b.raw_due(1).as_deref(), Some("2026-12-31"), "{ctx} edit stored something else");
        b.ok(zone, at, &["edit", "1", "--due", DATES[0]]);
        // every other zone reads the very same text from this file, whatever the time there
        for other in ZONES {
            for later in [INSTANTS[7], INSTANTS[8]] {
                assert_eq!(listed_dues(&b, other, later), DATES, "written under TZ={zone} at {at}, read under TZ={other} at {later}");
            }
        }
    }
}

#[test]
fn due_round_trips_unchanged_under_utc() {
    round_trip_in(ZONES[0]);
}

#[test]
fn due_round_trips_unchanged_under_los_angeles() {
    round_trip_in(ZONES[1]);
}

#[test]
fn due_round_trips_unchanged_under_auckland() {
    round_trip_in(ZONES[2]);
}

#[test]
fn due_round_trips_unchanged_under_kolkata() {
    round_trip_in(ZONES[3]);
}

#[test]
fn due_round_trips_unchanged_under_kiritimati_and_pago_pago() {
    round_trip_in(ZONES[4]);
    round_trip_in(ZONES[5]);
}

/// (board `tz`, instant, expected `days_left`, expected `due_state`) for a card due
/// 2026-10-09 with the default `due-warn` of 3.
type Flip = (&'static str, &'static str, i64, &'static str);

const FLIPS: [Flip; 17] = [
    // Los Angeles is UTC-7 in October: local midnight is 07:00 UTC
    ("America/Los_Angeles", "2026-10-06T06:59:59Z", 4, "ok"), // 23:59:59 on the 5th
    ("America/Los_Angeles", "2026-10-06T07:00:00Z", 3, "soon"), // 00:00:00 on the 6th: ok -> soon
    ("America/Los_Angeles", "2026-10-09T00:00:00Z", 1, "soon"), // UTC midnight: it is 17:00 on the 8th — NO flip
    ("America/Los_Angeles", "2026-10-09T06:59:59Z", 1, "soon"), // 23:59:59 on the 8th
    ("America/Los_Angeles", "2026-10-09T07:00:00Z", 0, "soon"), // 00:00:00 on the due date
    ("America/Los_Angeles", "2026-10-10T00:00:00Z", 0, "soon"), // UTC says the 10th; the board is still on the 9th — NOT overdue
    ("America/Los_Angeles", "2026-10-10T06:59:59Z", 0, "soon"), // 23:59:59 on the due date
    ("America/Los_Angeles", "2026-10-10T07:00:00Z", -1, "overdue"), // 00:00:00 the day after: soon -> overdue
    // Auckland is UTC+13 in October: local midnight is 11:00 UTC the day BEFORE
    ("Pacific/Auckland", "2026-10-08T10:59:59Z", 1, "soon"),
    ("Pacific/Auckland", "2026-10-08T11:00:00Z", 0, "soon"),
    ("Pacific/Auckland", "2026-10-09T10:59:59Z", 0, "soon"), // 23:59:59 on the due date
    ("Pacific/Auckland", "2026-10-09T11:00:00Z", -1, "overdue"), // overdue 13 hours before UTC reaches the 10th
    ("Pacific/Auckland", "2026-10-09T23:59:59Z", -1, "overdue"),
    // Kolkata is UTC+5:30: local midnight is 18:30 UTC the day before
    ("Asia/Kolkata", "2026-10-09T18:29:59Z", 0, "soon"),
    ("Asia/Kolkata", "2026-10-09T18:30:00Z", -1, "overdue"),
    ("UTC", "2026-10-09T23:59:59Z", 0, "soon"),
    ("UTC", "2026-10-10T00:00:00Z", -1, "overdue"),
];

/// `days_left` and `due_state` turn over at local midnight IN THE BOARD'S ZONE — whatever
/// zone the person or agent running the command is in.
#[test]
fn days_left_and_due_state_flip_at_local_midnight_in_the_board_tz() {
    let b = Board::new();
    b.ok("UTC", FLIPS[0].1, &["add", "filing: send the renewal", "--due", "2026-10-09"]);
    let mut zone = "";
    for (board_tz, at, days, state) in FLIPS {
        if zone != board_tz {
            zone = board_tz;
            b.ok("UTC", at, &["config", "tz", board_tz]);
        }
        for process_tz in ZONES {
            let (due, days_left, due_state) = b.due_of(process_tz, at, 1);
            let ctx = format!("board tz {board_tz}, run under TZ={process_tz} at {at}");
            assert_eq!(due, "2026-10-09", "{ctx}");
            assert_eq!(days_left, days, "{ctx}: days_left");
            assert_eq!(due_state, state, "{ctx}: due_state");
        }
    }
}

/// A board that sets no `tz` uses the machine's own zone: the same instant is a different
/// "today" for someone in Pago Pago and someone in Auckland.
#[test]
fn without_a_board_tz_today_is_the_local_date_of_whoever_runs_it() {
    let b = Board::new();
    b.ok("UTC", "2026-10-09T03:00:00Z", &["add", "file it", "--due", "2026-10-09"]);
    for (process_tz, at, days, state) in [
        ("Pacific/Pago_Pago", "2026-10-09T03:00:00Z", 1, "soon"), // 16:00 on the 8th
        ("America/Los_Angeles", "2026-10-09T03:00:00Z", 1, "soon"), // 20:00 on the 8th
        ("UTC", "2026-10-09T03:00:00Z", 0, "soon"),
        ("Asia/Kolkata", "2026-10-09T03:00:00Z", 0, "soon"), // 08:30 on the 9th
        ("Pacific/Auckland", "2026-10-09T03:00:00Z", 0, "soon"), // 16:00 on the 9th
        ("Pacific/Auckland", "2026-10-09T12:00:00Z", -1, "overdue"), // 01:00 on the 10th
        ("Pacific/Kiritimati", "2026-10-09T12:00:00Z", -1, "overdue"), // 02:00 on the 10th
        ("UTC", "2026-10-09T12:00:00Z", 0, "soon"),
        ("America/Los_Angeles", "2026-10-09T12:00:00Z", 0, "soon"), // 05:00 on the 9th
    ] {
        let (due, days_left, due_state) = b.due_of(process_tz, at, 1);
        assert_eq!(due, "2026-10-09");
        assert_eq!((days_left.as_i64(), due_state.as_str()), (Some(days), Some(state)), "TZ={process_tz} at {at}");
    }
    assert_eq!(b.ok("Pacific/Auckland", "2026-10-09T12:00:00Z", &["config", "tz"]).trim(), "local");
}

/// A day with a DST change is 23 or 25 hours long. `days_left` counts days on a calendar.
#[test]
fn days_left_counts_calendar_days_across_a_dst_change() {
    let b = Board::new();
    b.ok("UTC", "2026-03-01T00:00:00Z", &["config", "tz", "America/Los_Angeles"]);
    for due in ["2026-03-09", "2026-10-31", "2026-11-02"] {
        b.ok("UTC", "2026-03-01T00:00:00Z", &["add", &format!("due {due}"), "--due", due]);
    }
    // 23:30 PST on 2026-03-07; the 8th has 23 hours, so the start of the 9th is 23.5 hours
    // away — not even one 24-hour block, but two days on the calendar
    for process_tz in ZONES {
        let (_, days, state) = b.due_of(process_tz, "2026-03-08T07:30:00Z", 1);
        assert_eq!((days.as_i64(), state.as_str()), (Some(2), Some("soon")), "spring forward, TZ={process_tz}");
        // 23:30 PST on 2026-11-01, a 25-hour day
        let (_, days, state) = b.due_of(process_tz, "2026-11-02T07:30:00Z", 2);
        assert_eq!((days.as_i64(), state.as_str()), (Some(-1), Some("overdue")), "fall back, TZ={process_tz}");
        let (_, days, _) = b.due_of(process_tz, "2026-11-02T07:30:00Z", 3);
        assert_eq!(days.as_i64(), Some(1), "fall back, TZ={process_tz}");
        // 00:30 on 2026-11-01, before the repeated hour
        let (_, days, _) = b.due_of(process_tz, "2026-11-01T07:30:00Z", 3);
        assert_eq!(days.as_i64(), Some(1), "the long day, TZ={process_tz}");
    }
}

#[test]
fn due_warn_sets_how_early_soon_starts() {
    let b = Board::new();
    let at = "2026-10-01T12:00:00Z";
    b.ok("UTC", at, &["config", "tz", "UTC"]);
    for (id, due) in [(1, "2026-09-30"), (2, "2026-10-01"), (3, "2026-10-04"), (4, "2026-10-05"), (5, "2026-10-08"), (6, "2026-10-09")] {
        b.ok("UTC", at, &["add", &format!("card {id}"), "--due", due]);
    }
    let states = |b: &Board| -> Vec<String> {
        (1..=6).map(|id| b.due_of("UTC", at, id).2.as_str().unwrap().to_string()).collect()
    };
    assert_eq!(b.ok("UTC", at, &["config", "due-warn"]).trim(), "3", "the default");
    assert_eq!(states(&b), ["overdue", "soon", "soon", "ok", "ok", "ok"], "default due-warn 3");
    let v = b.json("UTC", at, &["config", "due-warn", "7", "--json"]);
    assert_eq!(v, serde_json::json!({"ok": true, "config": {"key": "due-warn", "value": 7}}));
    assert_eq!(states(&b), ["overdue", "soon", "soon", "soon", "soon", "ok"], "due-warn 7");
    b.ok("UTC", at, &["config", "due-warn", "0"]);
    assert_eq!(states(&b), ["overdue", "soon", "ok", "ok", "ok", "ok"], "due-warn 0: only today is soon");
    assert_eq!(b.json("UTC", at, &["config", "due-warn", "--json"])["config"]["value"], 0);
    for bad in ["366", "9999999999999999999", "soon", "3.5", ""] {
        let o = b.run("UTC", at, &["config", "due-warn", bad]);
        let err = String::from_utf8_lossy(&o.stderr).to_string();
        assert_eq!(o.status.code(), Some(1), "due-warn {bad}: {err}");
        assert!(err.contains("'tb config due-warn 3'"), "due-warn {bad} names the command to run: {err}");
    }
    assert_eq!(b.json("UTC", at, &["config", "due-warn", "--json"])["config"]["value"], 0, "a refused value changed nothing");
}

#[test]
fn a_bad_date_is_refused_with_the_command_to_run_and_nothing_is_written() {
    let b = Board::new();
    let at = "2026-10-01T12:00:00Z";
    for bad in ["2026-02-30", "2027-02-29", "2026-13-01", "2026-1-5", "10/09/2026", "2026/10/09", "tomorrow", "", "2026-10-09T00:00:00Z", "0000-01-01"] {
        let o = b.run("UTC", at, &["add", "never created", "--due", bad]);
        let err = String::from_utf8_lossy(&o.stderr).to_string();
        assert_eq!(o.status.code(), Some(1), "add --due {bad:?}: {err}");
        assert!(err.contains("use YYYY-MM-DD") && err.contains("'tb add \"tag: title\" --due 2026-10-09'"), "add --due {bad:?}: {err}");
        assert_eq!(err.trim().lines().count(), 1, "one line: {err}");
        assert_eq!(b.json("UTC", at, &["list", "--json"]), serde_json::json!([]), "add --due {bad:?} left a card behind");
    }
    let v = b.json("UTC", at, &["add", "real", "--due", "2026-10-09", "--json"]);
    assert_eq!(v["card"]["id"], 1);
    for bad in ["2026-02-30", "friday", ""] {
        // together with a good --title: the whole edit is refused, the title is NOT written
        let o = b.run("UTC", at, &["edit", "1", "--title", "renamed", "--due", bad, "--json"]);
        assert_eq!(o.status.code(), Some(1));
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("not a") || v["error"].as_str().unwrap().contains("is empty"), "{v}");
        assert!(v["hint"].as_str().unwrap().contains("'tb edit 1 --due 2026-10-09'"), "{v}");
        let card = b.json("UTC", at, &["show", "1", "--json"]);
        assert_eq!((card["title"].as_str(), card["due"].as_str()), (Some("real"), Some("2026-10-09")), "edit --due {bad:?}");
    }
    assert_eq!(b.raw_due(1).as_deref(), Some("2026-10-09"));
    // a missing card is reported as that, not as a date problem
    let o = b.run("UTC", at, &["edit", "99", "--due", "2026-10-09"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("no card #99"));
}

#[test]
fn none_clears_a_due_date_and_every_change_is_in_the_history() {
    let b = Board::new();
    let at = "2026-10-01T12:00:00Z";
    b.ok("UTC", at, &["add", "filing: send the renewal", "--due", "2026-10-09"]);
    b.ok("UTC", at, &["edit", "1", "--due", "2026-10-16"]);
    b.ok("UTC", at, &["edit", "1", "--due", "2026-10-16"]); // the same date again: nothing to record
    let v = b.json("UTC", at, &["edit", "1", "--due", "none", "--json"]);
    assert_eq!(v["ok"], true);
    assert!(v["card"]["due"].is_null() && v["card"]["days_left"].is_null() && v["card"]["due_state"].is_null(), "{v}");
    assert_eq!(b.raw_due(1), None, "cleared = NULL in the file");
    assert!(!b.ok("UTC", at, &["show", "1"]).contains("due 2026"), "plain show no longer names a date");
    b.ok("UTC", at, &["edit", "1", "--due", "none"]); // clearing nothing is not an error
    // title and date in one command
    b.ok("UTC", at, &["edit", "1", "--title", "filing: send the reply", "--due", "2026-11-02"]);
    let d = b.json("UTC", at, &["show", "1", "--json"]);
    assert_eq!((d["title"].as_str(), d["due"].as_str()), (Some("send the reply"), Some("2026-11-02")));
    let due_events: Vec<&str> =
        d["events"].as_array().unwrap().iter().filter(|e| e["kind"] == "due").map(|e| e["text"].as_str().unwrap()).collect();
    assert_eq!(due_events, ["none -> 2026-10-09", "2026-10-09 -> 2026-10-16", "2026-10-16 -> none", "none -> 2026-11-02"]);
    assert!(d["events"].as_array().unwrap().iter().all(|e| e["actor"] == "tester"));
}

#[test]
fn tz_is_set_read_cleared_and_refused_from_the_cli() {
    let b = Board::new();
    let at = "2026-10-01T12:00:00Z";
    assert_eq!(b.ok("UTC", at, &["config", "tz"]).trim(), "local", "unset = the machine's zone");
    assert_eq!(
        b.json("UTC", at, &["config", "tz", "--json"]),
        serde_json::json!({"ok": true, "config": {"key": "tz", "value": "local"}})
    );
    let v = b.json("UTC", at, &["config", "tz", "America/Los_Angeles", "--json"]);
    assert_eq!(v, serde_json::json!({"ok": true, "config": {"key": "tz", "value": "America/Los_Angeles"}}));
    assert_eq!(b.ok("Pacific/Auckland", at, &["config", "tz"]).trim(), "America/Los_Angeles");
    let listing = b.ok("UTC", at, &["config"]);
    assert!(listing.contains(&format!("{:<13} {}", "tz", "America/Los_Angeles")), "{listing}");
    assert_eq!(b.json("UTC", at, &["config", "--json"])["config"]["tz"], "America/Los_Angeles");
    for bad in ["Mars/Olympus_Mons", "PST8", "america/los_angeles", "UTC-8", "08:00", ""] {
        let o = b.run("UTC", at, &["config", "tz", bad, "--json"]);
        assert_eq!(o.status.code(), Some(1), "tz {bad:?}");
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["ok"], false, "tz {bad:?}");
        assert!(v["error"].as_str().unwrap().starts_with("unknown time zone"), "{v}");
        assert!(v["hint"].as_str().unwrap().contains("'tb config tz America/Los_Angeles'"), "{v}");
    }
    assert_eq!(b.ok("UTC", at, &["config", "tz"]).trim(), "America/Los_Angeles", "a refused zone changed nothing");
    let o = b.run("UTC", at, &["config", "tz", "--off"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("'tb config tz local'"));
    b.ok("UTC", at, &["config", "tz", "local"]);
    assert_eq!(b.ok("UTC", at, &["config", "tz"]).trim(), "local");
    assert!(!b.ok("UTC", at, &["config"]).contains("tz "), "cleared = no longer listed");
}

/// The rule for this whole series: a board that sets nothing and dates nothing reads exactly
/// as it did. (The render goldens cover the board itself; this pins the text commands.)
#[test]
fn a_board_with_no_due_dates_and_no_settings_is_unchanged() {
    let b = Board::new();
    let at = "2026-10-01T12:00:00Z";
    b.ok("UTC", at, &["add", "widgets: gh#7 fix it", "-d", "Done = fixed", "--check", "repro"]);
    b.ok("UTC", at, &["add", "second"]);
    b.ok("UTC", at, &["take", "2"]);
    assert_eq!(
        b.ok("UTC", at, &["config"]),
        "wip           3\ntheme         dark\nlayout        auto\ngithub        off\ngithub-panel  shown\nagents-panel  shown\n",
        "the settings listing of a board that sets nothing"
    );
    let keys: Vec<String> = b.json("UTC", at, &["config", "--json"])["config"].as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["agents-panel", "github", "github-panel", "layout", "theme", "wip"]);
    assert_eq!(b.ok("UTC", at, &["list"]), "todo    #1 gh#7 fix it  [widgets - 0m - 0/1]\ndoing   #2 second  [tester - 0m]\n");
    assert_eq!(b.ok("UTC", at, &["show", "2"]), "#2 second\ndoing - tester - 0m\n\n12:00 added by tester\n12:00 taken by tester\n");
    for cmd in [vec!["board"], vec!["list"], vec!["show", "1"]] {
        assert!(!b.ok("UTC", at, &cmd).contains("due"), "{cmd:?} mentions a due date on a board without any");
    }
    for id in [1, 2] {
        let (due, days_left, due_state) = b.due_of("America/Los_Angeles", at, id);
        assert!(due.is_null() && days_left.is_null() && due_state.is_null(), "#{id}: additive fields are null without a date");
    }
    assert_eq!(b.json("UTC", at, &["board", "--json"])["v"], 1, "schema version unchanged");
}

/// Text another writer left in the column (tb itself never wrote it before `--due`) is kept
/// and reported as is; tb derives nothing from it.
#[test]
fn older_free_text_in_the_due_column_is_reported_as_is() {
    let b = Board::new();
    let at = "2026-10-01T12:00:00Z";
    b.ok("UTC", at, &["add", "ops: rotate API tokens"]);
    raw(&b.db).execute("UPDATE cards SET due='Sep 22' WHERE id=1", []).unwrap();
    let (due, days_left, due_state) = b.due_of("UTC", at, 1);
    assert_eq!(due, "Sep 22");
    assert!(days_left.is_null() && due_state.is_null());
    assert!(b.ok("UTC", at, &["show", "1"]).contains("due Sep 22"));
    b.ok("UTC", at, &["edit", "1", "--due", "2026-10-09"]);
    let d = b.json("UTC", at, &["show", "1", "--json"]);
    assert_eq!(d["due"], "2026-10-09");
    assert_eq!(d["events"].as_array().unwrap().last().unwrap()["text"], "Sep 22 -> 2026-10-09");
}

#[test]
fn a_done_card_carries_no_due_warning() {
    let b = Board::new();
    let at = "2026-10-20T12:00:00Z";
    b.ok("UTC", at, &["config", "tz", "UTC"]);
    b.ok("UTC", at, &["add", "late one", "--due", "2026-10-09"]);
    let (_, days, state) = b.due_of("UTC", at, 1);
    assert_eq!((days.as_i64(), state.as_str()), (Some(-11), Some("overdue")));
    // a write result carries the same fields as a read
    let taken = b.json("UTC", at, &["next", "--json"]);
    assert_eq!((taken["card"]["days_left"].as_i64(), taken["card"]["due_state"].as_str()), (Some(-11), Some("overdue")));
    b.ok("UTC", at, &["done", "1"]); // -> review
    b.ok("UTC", at, &["done", "1", "--as", "checker"]); // -> done
    let (due, days, state) = b.due_of("UTC", at, 1);
    assert_eq!(due, "2026-10-09", "the date itself stays on the card");
    assert!(days.is_null() && state.is_null(), "finished work is not overdue");
    b.ok("UTC", at, &["move", "1", "todo"]);
    assert_eq!(b.due_of("UTC", at, 1).2, "overdue", "reopened: the warning is back");
}
