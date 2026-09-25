#![cfg(unix)]
//! Who made a board (#137): `tb new` and create-on-first-use record the creator in the board's
//! own file; `tb boards --json` / `--archived --json` give it as `created_by`; `--long` prints
//! it; a board without a record reads it from `board-creations.log`. Drives the real binary in
//! a temp HOME (boards mode, no `TB_DB`) with a controlled environment.
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

struct Home(tempfile::TempDir);

impl Home {
    fn new() -> Home {
        Home(tempfile::tempdir().unwrap())
    }

    fn state(&self) -> PathBuf {
        self.0.path().join(".local/state/terminal-board")
    }

    fn run(&self, env: &[(&str, &str)], args: &[&str]) -> std::process::Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).env_clear();
        c.env("HOME", self.0.path()).env("USER", "login-user").env("TZ", "UTC").env("PATH", "/usr/bin:/bin");
        c.envs(env.iter().copied());
        c.output().unwrap()
    }

    fn ok(&self, env: &[(&str, &str)], args: &[&str]) -> String {
        let o = self.run(env, args);
        assert!(o.status.success(), "tb {args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(&[], args)).unwrap()
    }
}

fn board<'a>(rows: &'a Value, name: &str) -> &'a Value {
    rows.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap_or_else(|| panic!("no row {name}: {rows}"))
}

const AGENT: [(&str, &str); 5] =
    [("TB_HARNESS", "claude-code"), ("TB_MODEL", "m-1"), ("TB_ROLE", "coder"), ("TB_SESSION", "sess-1"), ("TB_HOST", "box.lan")];

#[test]
fn tb_new_records_the_creator_with_the_identity_of_a_card_actor() {
    let h = Home::new();
    h.ok(&AGENT, &["new", "made", "--as", "maker"]);
    let rows = h.json(&["boards", "--json"]);
    let c = &board(&rows, "made")["created_by"];
    assert_eq!(c["actor"], "maker");
    assert_eq!(c["harness"], "claude-code");
    assert_eq!(c["model"], "m-1");
    assert_eq!(c["role"], "coder");
    assert_eq!(c["session"], "sess-1");
    assert_eq!(c["host"], "box", "the host is stored as actors store it (first label)");
    assert_eq!(c["source"], "board");
    let at = c["at"].as_str().unwrap();
    assert!(at.len() == 20 && at.ends_with('Z') && at.as_bytes()[10] == b'T', "RFC 3339 UTC: {at}");
}

#[test]
fn create_on_first_use_records_who_made_it_and_later_writers_change_nothing() {
    let h = Home::new();
    h.ok(&[("TB_SESSION", "s-auto"), ("TB_HARNESS", "omp")], &["auto", "add", "first card", "--as", "autouser"]);
    h.ok(&[("TB_SESSION", "s-other")], &["auto", "add", "second card", "--as", "someone-else"]);
    let rows = h.json(&["boards", "--json"]);
    let c = &board(&rows, "auto")["created_by"];
    assert_eq!((c["actor"].as_str(), c["session"].as_str()), (Some("autouser"), Some("s-auto")));
}

#[test]
fn a_person_records_a_name_and_a_time_only() {
    let h = Home::new();
    h.ok(&[], &["home", "add", "call the plumber"]);
    let rows = h.json(&["boards", "--json"]);
    let c = &board(&rows, "home")["created_by"];
    assert_eq!(c["actor"], "login-user");
    for k in ["harness", "model", "role", "session", "host"] {
        assert!(c[k].is_null(), "{k} must be null for a person: {c}");
    }
    assert!(c["at"].is_string());
}

#[test]
fn long_shows_the_creators_and_an_archived_board_keeps_its_creator() {
    let h = Home::new();
    h.ok(&AGENT, &["new", "made", "--as", "maker"]);
    h.ok(&[], &["other", "add", "x", "--as", "otheruser"]);
    let long = h.ok(&[], &["boards", "--long"]);
    assert!(long.contains("created by maker (claude-code m-1 coder, session sess-1 on box)"), "{long}");
    assert!(long.contains("created by otheruser "), "{long}");
    h.ok(&[], &["boards", "archive", "made"]);
    let rows = h.json(&["boards", "--archived", "--json"]);
    let c = &board(&rows, "made")["created_by"];
    assert_eq!((c["actor"].as_str(), c["source"].as_str()), (Some("maker"), Some("board")));
    let long = h.ok(&[], &["boards", "--archived", "--long"]);
    assert!(long.contains("created by maker"), "{long}");
}

#[test]
fn setup_records_the_creator_of_the_board_it_creates() {
    let h = Home::new();
    // --yes accepts defaults without a terminal; the board it creates is 'default'
    h.ok(&[("TB_SESSION", "sess-setup"), ("TB_HARNESS", "omp")],
         &["setup", "--yes", "--no-github", "--no-agents", "--as", "setupper"]);
    let rows = h.json(&["boards", "--json"]);
    let c = &board(&rows, "default")["created_by"];
    assert_eq!((c["actor"].as_str(), c["session"].as_str(), c["source"].as_str()),
               (Some("setupper"), Some("sess-setup"), Some("board")), "the wizard's creator: {c}");
    // a later session re-running setup must not take the record over
    h.ok(&[("TB_SESSION", "sess-later")], &["setup", "--yes", "--no-github", "--no-agents", "--as", "second-comer"]);
    let rows = h.json(&["boards", "--json"]);
    let c = &board(&rows, "default")["created_by"];
    assert_eq!((c["actor"].as_str(), c["session"].as_str()), (Some("setupper"), Some("sess-setup")),
               "the first record stays: {c}");
}

#[test]
fn a_tb_db_board_records_its_creator_on_first_read() {
    let h = Home::new();
    let db = h.0.path().join("pinned.db");
    h.ok(&[("TB_DB", db.to_str().unwrap()), ("TB_SESSION", "sess-pin")], &["boards", "--json", "--as", "pinner"]);
    // the pinned file is not in the boards dir, so read it through the store itself
    let store = rusqlite::Connection::open(&db).unwrap();
    let (actor, session, n): (String, String, i64) = store
        .query_row("SELECT actor, session, (SELECT COUNT(*) FROM board_creator) FROM board_creator", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(n, 1, "exactly one creator row");
    assert_eq!((actor.as_str(), session.as_str()), ("pinner", "sess-pin"));
    // a second reader changes nothing
    h.ok(&[("TB_DB", db.to_str().unwrap()), ("TB_SESSION", "sess-2")], &["boards", "--json", "--as", "late-reader"]);
    let n: i64 = rusqlite::Connection::open(&db).unwrap().query_row("SELECT COUNT(*) FROM board_creator", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "the first record stays");
}

#[test]
fn a_board_with_its_own_record_ignores_a_log_line_naming_someone_else() {
    let h = Home::new();
    h.ok(&[("TB_SESSION", "sess-own")], &["new", "owned", "--as", "owner"]);
    std::fs::write(
        h.state().join("board-creations.log"),
        "2026-09-18T18:52:20Z\towned\tactor=impostor\tsession=sess-log\thost=loghost\tworkspace=-\tproject=owned\tinferred\n",
    )
    .unwrap();
    let rows = h.json(&["boards", "--json"]);
    let c = &board(&rows, "owned")["created_by"];
    assert_eq!((c["actor"].as_str(), c["session"].as_str(), c["source"].as_str()),
               (Some("owner"), Some("sess-own"), Some("board")), "the board's own record wins: {c}");
}

#[test]
fn a_board_without_a_record_reads_its_creator_from_the_creations_log() {
    let h = Home::new();
    h.ok(&[], &["legacy", "add", "old card", "--as", "olduser"]);
    // a board from before #137: its file has an empty creator table
    let db = h.state().join("boards/legacy.db");
    rusqlite::Connection::open(&db).unwrap().execute("DELETE FROM board_creator", []).unwrap();
    assert!(board(&h.json(&["boards", "--json"]), "legacy")["created_by"].is_null(), "no record, no log: null");
    std::fs::write(
        h.state().join("board-creations.log"),
        "2026-09-18T18:52:20Z\tlegacy\tactor=logged-maker\tsession=sess-log\thost=loghost\tworkspace=-\tproject=legacy\tinferred\n",
    )
    .unwrap();
    let rows = h.json(&["boards", "--json"]);
    let c = &board(&rows, "legacy")["created_by"];
    assert_eq!(c["actor"], "logged-maker");
    assert_eq!(c["session"], "sess-log");
    assert_eq!(c["host"], "loghost");
    assert_eq!(c["at"], "2026-09-18T18:52:20Z");
    assert_eq!(c["source"], "log");
    // reading it never writes it into the board
    let n: i64 = rusqlite::Connection::open(&db).unwrap().query_row("SELECT COUNT(*) FROM board_creator", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}
