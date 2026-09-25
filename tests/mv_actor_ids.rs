#![cfg(unix)]
//! `tb mv ID --to BOARD` when the card's history carries identities (`actors`, `actor_id`).
//!
//! The ids of `actors` are per board (`AUTOINCREMENT`), and `events.actor_id` references
//! them — so a move that copied the ids verbatim hit `FOREIGN KEY constraint failed` and the
//! card could not move at all (#138). Everything here drives the real `tb` binary with a
//! controlled environment (so whatever harness runs the suite never leaks in) and asserts
//! through the CLI and the database itself:
//! - the move exits 0 and the card is gone from the source, on the destination;
//! - the history is intact, with each event's `actor_id` pointing at an `actors` row the
//!   destination REALLY has, showing the same session the source showed;
//! - a second move to a board that already holds one of the identities finds that row
//!   instead of duplicating it (the match is the identity, never the id);
//! - a card whose events have no identity moves exactly as it always did.
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const SESSION_A: &str = "0b9f6a52-7c1d-4e0a-9f3b-2a6c1d8e4f70";
const SESSION_B: &str = "7e1c3d90-55aa-4b1f-8c2e-9d0f1a2b3c4d";

/// A harness that exports its name and its session (the same shape `tests/actors.rs` uses).
/// The explicit `TB_*` names, not what a real harness exports: the harness running this
/// suite (this pane is omp, inside herdr) must not leak in — `env_clear` already strips
/// everything, and these are the values `Identity::resolve` trusts first.
const ALICE: &[(&str, &str)] = &[
    ("TB_HARNESS", "claude-code"),
    ("TB_SESSION", SESSION_A),
    ("TB_MODEL", "model-x"),
    ("TB_ROLE", "coder"),
    ("TB_HOST", "box.lan"),
];
/// A second identity on the same card, so the move has more than one row to carry.
const BOB: &[(&str, &str)] = &[
    ("TB_HARNESS", "claude-code"),
    ("TB_SESSION", SESSION_B),
    ("TB_MODEL", "model-y"),
    ("TB_ROLE", "reviewer"),
    ("TB_HOST", "gatehost"),
];

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home { dir: tempfile::tempdir().unwrap() }
    }
    fn board_file(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!(".local/state/terminal-board/boards/{name}.db"))
    }
    fn cmd(&self, env: &[(&str, &str)], args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).env_clear().stdin(Stdio::null());
        c.env("HOME", self.dir.path()).env("USER", "login-user").env("TZ", "UTC").env("PATH", "/usr/bin:/bin");
        c.env("NO_HERDR", "1").env("TB_NOW", "1789000000").env("TB_CONFIG", self.dir.path().join("cfg.json"));
        c.envs(env.iter().copied());
        c
    }
    fn run(&self, env: &[(&str, &str)], args: &[&str]) -> Output {
        self.cmd(env, args).output().unwrap()
    }
    fn ok(&self, env: &[(&str, &str)], args: &[&str]) -> String {
        let o = self.run(env, args);
        assert!(o.status.success(), "tb {args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }
    fn conn(&self, board: &str) -> rusqlite::Connection {
        rusqlite::Connection::open(self.board_file(board)).unwrap()
    }
    /// The sessions every `actors` row on `board` holds, in id order.
    fn sessions(&self, board: &str) -> Vec<String> {
        assert!(self.board_file(board).exists(), "no board file for '{board}'");
        let conn = self.conn(board);
        let mut st = conn.prepare("SELECT session FROM actors ORDER BY id").unwrap();
        let v = st.query_map([], |r| r.get::<_, Option<String>>(0)).unwrap().map(|r| r.unwrap().unwrap()).collect();
        v
    }
    /// `(card_id, ts, kind, actor_id)` of every event on `board`, in id order.
    fn events(&self, board: &str) -> Vec<(i64, i64, String, Option<i64>)> {
        let conn = self.conn(board);
        let mut st = conn.prepare("SELECT card_id, ts, kind, actor_id FROM events ORDER BY id").unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(|r| r.unwrap()).collect()
    }
}

/// The board plain `tb` opens — the SOURCE of every move here.
const SRC: &str = "default";

/// The fixture every test uses: card 1 on the default board with events written by TWO
/// distinct identities (alice and bob), card 2 there with none, and a card already on `dst`
/// (added by alice, so `dst` already holds alice's identity row, id 1).
fn boards() -> Home {
    let h = Home::new();
    h.ok(ALICE, &["add", "bug: the moved card", "--as", "alice"]);
    h.ok(ALICE, &["take", "1", "--as", "alice"]);
    h.ok(ALICE, &["note", "1", "progress by alice", "--as", "alice"]);
    h.ok(BOB, &["note", "1", "note by bob", "--as", "bob"]);
    h.ok(&[], &["add", "plain: no identity", "--as", "carol"]);
    h.ok(ALICE, &["dst", "add", "a card already here", "--as", "alice"]);
    h
}

/// The #138 repro: a card whose every event has an `actor_id` could not move at all.
#[test]
fn a_card_with_identity_events_moves_and_keeps_them() {
    let h = boards();
    // the precondition: two sessions behind card 1's events on the source; only alice's on dst
    assert_eq!(h.sessions(SRC), [SESSION_A, SESSION_B]);
    assert_eq!(h.sessions("dst"), [SESSION_A]);

    let said = h.ok(ALICE, &["mv", "1", "--to", "dst", "--as", "alice"]);
    assert!(said.contains("moved to 'dst' as #2"), "{said}");
    assert!(said.contains("4 events"), "added, taken and two notes: {said}");

    // gone from the source; its board log names where it went
    assert_eq!(h.events(SRC).iter().filter(|e| e.0 == 1).count(), 0, "card 1's events left with it");
    let log = h.ok(&[], &[SRC, "log", "--as", "carol"]);
    assert!(log.contains("moved-out"), "{log}");

    // there, with its whole history and the identity behind every event
    let show = h.ok(&[], &["dst", "show", "2", "--as", "carol"]);
    assert!(show.contains("progress by alice"), "{show}");
    assert!(show.contains("note by bob"), "{show}");
    assert!(show.contains(&format!("session {SESSION_A}")), "{show}");
    assert!(show.contains(&format!("session {SESSION_B}")), "{show}");

    // the ids are the DESTINATION's: alice's row is the one dst already had (found by
    // identity, not copied by id), bob's is new there
    let dst = h.sessions("dst");
    assert_eq!(dst, [SESSION_A, SESSION_B], "{dst:?}");
    let moved = h.events("dst").iter().filter(|e| e.0 == 2 && e.2 == "note").map(|e| e.3).collect::<Vec<_>>();
    assert_eq!(moved, [Some(1), Some(2)], "each note points at its own identity row: {moved:?}");
    // and every actor_id on dst resolves to a row dst really has
    let dangling: i64 = h
        .conn("dst")
        .query_row("SELECT COUNT(*) FROM events WHERE actor_id IS NOT NULL AND actor_id NOT IN (SELECT id FROM actors)", [], |r| r.get(0))
        .unwrap();
    assert_eq!(dangling, 0, "no event points off the board");
}

/// Two identities that share a row arrive one after the other: the second move finds the row
/// the first made (matched on the identity tuple, never on the id), so one session is one row.
#[test]
fn the_second_move_finds_the_identity_the_first_left() {
    let h = boards();
    h.ok(ALICE, &["mv", "1", "--to", "dst", "--as", "alice"]);
    h.ok(ALICE, &["add", "bug: a second card", "--as", "alice"]);
    h.ok(ALICE, &["note", "2", "same session again", "--as", "alice"]);

    h.ok(ALICE, &["mv", "2", "--to", "dst", "--as", "alice"]);
    let dst = h.sessions("dst");
    assert_eq!(dst, [SESSION_A, SESSION_B], "no duplicate row for the session that moved twice: {dst:?}");
    let rows: i64 = {
        let conn = h.conn("dst");
        conn.query_row("SELECT COUNT(*) FROM actors", [], |r| r.get(0)).unwrap()
    };
    assert_eq!(rows, 2, "{rows} actor rows on the destination");

    // the second card's note points at the row the first move already made
    let note = h.events("dst").iter().find(|e| e.0 == 3 && e.2 == "note").and_then(|e| e.3);
    assert_eq!(note, Some(1), "reused, not re-inserted: {note:?}");
}

/// A card with no identity in its history moves exactly as it always did (no row is invented
/// for a name the destination never knew an identity for).
#[test]
fn a_card_without_identities_moves_as_it_always_did() {
    let h = boards();
    h.ok(&[], &["mv", "2", "--to", "dst", "--as", "carol"]);
    let said = h.ok(&[], &["dst", "show", "2", "--as", "carol"]);
    assert!(said.contains("no identity"), "{said}");
    assert!(!said.contains("actors:"), "no identity, no section: {said}");
    assert_eq!(h.sessions("dst"), [SESSION_A], "nothing invented: dst keeps only the row it had");
    let ids: Vec<Option<i64>> = h.events("dst").iter().filter(|e| e.0 == 2).map(|e| e.3).collect();
    assert!(ids.iter().all(Option::is_none), "its events keep NULL actor_id: {ids:?}");
}
