#![cfg(unix)]
//! Who did the work: next to the short `actor` name, an event records the harness, model,
//! role, session and machine behind it (`actors`, `actor_id`). Everything here drives the
//! real `tb` binary with a controlled environment and reads the board file back.
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const UUID: &str = "0b9f6a52-7c1d-4e0a-9f3b-2a6c1d8e4f70";
const UUID2: &str = "7e1c3d90-55aa-4b1f-8c2e-9d0f1a2b3c4d";

struct Board {
    dir: tempfile::TempDir,
}

impl Board {
    fn new() -> Board {
        Board { dir: tempfile::tempdir().unwrap() }
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("b.db")
    }

    /// `tb ARGS` with ONLY these variables set (plus the board file, a login name, UTC and a
    /// bare PATH): whatever harness runs the test suite must not leak into the board.
    fn cmd(&self, env: &[(&str, &str)], args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).env_clear();
        c.env("TB_DB", self.db()).env("USER", "login-user").env("TZ", "UTC").env("PATH", "/usr/bin:/bin").env("HOME", self.dir.path());
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

    fn json(&self, env: &[(&str, &str)], args: &[&str]) -> serde_json::Value {
        serde_json::from_str(&self.ok(env, args)).unwrap()
    }

    fn conn(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.db()).unwrap()
    }

    /// Rows of `actors` as `actor|harness|model|role|session|host` (`-` = NULL), in id order.
    /// A board without the table has no rows: the caller's assertion says what is missing.
    fn actors(&self) -> Vec<String> {
        let conn = self.conn();
        if !has_table(&conn, "actors") {
            return Vec::new();
        }
        let mut st = conn
            .prepare(
                "SELECT actor || '|' || IFNULL(harness,'-') || '|' || IFNULL(model,'-') || '|' || IFNULL(role,'-')
                        || '|' || IFNULL(session,'-') || '|' || IFNULL(host,'-') FROM actors ORDER BY id",
            )
            .unwrap();
        let v = st.query_map([], |r| r.get::<_, String>(0)).unwrap().map(|r| r.unwrap()).collect();
        v
    }

    /// `actor_id` of every row of `table`, in id order (`None` = NULL, or no such column).
    fn actor_ids(&self, table: &str) -> Vec<Option<i64>> {
        let conn = self.conn();
        let col = if has_column(&conn, table, "actor_id") { "actor_id" } else { "NULL" };
        let mut st = conn.prepare(&format!("SELECT {col} FROM {table} ORDER BY id")).unwrap();
        let v = st.query_map([], |r| r.get::<_, Option<i64>>(0)).unwrap().map(|r| r.unwrap()).collect();
        v
    }
}

fn has_table(conn: &rusqlite::Connection, table: &str) -> bool {
    conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?", [table], |r| r.get::<_, i64>(0)).unwrap() > 0
}

fn has_column(conn: &rusqlite::Connection, table: &str, col: &str) -> bool {
    conn.query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name=?"), [col], |r| r.get::<_, i64>(0)).unwrap() > 0
}

/// A harness that exports its name and its session, with the two explicit values.
fn session(id: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("CLAUDECODE", "1"),
        ("AI_AGENT", "claude-code_9-9-9_agent"),
        ("CLAUDE_CODE_SESSION_ID", id),
        ("TB_MODEL", "model-x"),
        ("TB_ROLE", "orchestrator"),
        ("TB_HOST", "box.lan"),
    ]
}

#[test]
fn a_plain_terminal_records_no_identity_and_reads_as_it_always_did() {
    let b = Board::new();
    b.ok(&[], &["add", "widgets: a card", "--check", "one"]);
    b.ok(&[], &["take", "1"]);
    b.ok(&[], &["note", "1", "half way"]);
    b.ok(&[], &["config", "wip", "5"]);
    assert_eq!(b.actors(), Vec::<String>::new(), "nothing is known beyond the name: no row, and no machine name in the file");
    assert!(b.actor_ids("events").iter().all(Option::is_none));
    // a far-future clock, so the ages in `show` are stable: 4102444799 is the last second
    // `TB_NOW` accepts (store::TB_NOW_MAX is exclusive), 4102444800 the first it refuses
    let show = b.ok(&[("TB_NOW", "4102444799")], &["show", "1"]);
    assert!(!show.contains("actors:"), "{show}");
    assert!(show.contains("added by login-user") && show.contains("login-user: half way"), "the short name is what it was: {show}");
    // the new JSON fields are there, and empty
    let v = b.json(&[], &["show", "1", "--json"]);
    assert_eq!(v["actors"], serde_json::json!([]), "{v}");
    assert!(v["events"].as_array().unwrap().iter().all(|e| e.as_object().unwrap().contains_key("actor_id") && e["actor_id"].is_null()), "{v}");
    let v = b.json(&[], &["board", "--json"]);
    assert_eq!((v["v"].as_i64(), &v["actors"]), (Some(1), &serde_json::json!([])), "{v}");
}

#[test]
fn a_session_is_recorded_with_every_event_and_shown_with_the_card() {
    let b = Board::new();
    let env = session(UUID);
    b.ok(&env, &["add", "widgets: a card", "--as", "lead"]);
    b.ok(&env, &["note", "1", "first", "--as", "lead"]);
    b.ok(&env, &["config", "wip", "5", "--as", "lead"]);
    b.ok(&env, &["add", "another", "--as", "lead"]);
    b.ok(&env, &["rm", "2", "--as", "lead"]);
    assert_eq!(b.actors(), [format!("lead|claude-code|model-x|orchestrator|{UUID}|box")], "the harness without its version, the host's first label");
    assert_eq!(b.actor_ids("events"), [Some(1), Some(1)], "card events");
    assert_eq!(b.actor_ids("board_events"), [Some(1), Some(1)], "board events (wip, delete)");
    // the name on the card is still the short one, in the database and on screen
    let names: Vec<String> = {
        let conn = b.conn();
        let mut st = conn.prepare("SELECT DISTINCT actor FROM events").unwrap();
        let v = st.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        v
    };
    assert_eq!(names, ["lead"]);
    let show = b.ok(&[], &["show", "1"]);
    let want = format!("\n\nactors:\n  lead — claude-code model-x orchestrator session {UUID} on box\n");
    assert!(show.ends_with(&want), "{show}");
    assert!(show.contains("added by lead\n"), "event lines are unchanged: {show}");

    let v = b.json(&[], &["show", "1", "--json"]);
    let a = &v["actors"][0];
    let mut keys: Vec<&str> = a.as_object().expect("actors[0]").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["actor", "first_seen", "harness", "host", "id", "last_seen", "model", "role", "session"]);
    assert_eq!(
        (a["id"].as_i64(), a["actor"].as_str(), a["harness"].as_str(), a["model"].as_str(), a["role"].as_str(), a["session"].as_str(), a["host"].as_str()),
        (Some(1), Some("lead"), Some("claude-code"), Some("model-x"), Some("orchestrator"), Some(UUID), Some("box"))
    );
    assert!(a["first_seen"].as_i64().unwrap() > 0 && a["last_seen"].as_i64() >= a["first_seen"].as_i64());
    assert!(v["events"].as_array().unwrap().iter().all(|e| e["actor_id"] == 1 && e["actor"] == "lead"), "{v}");
    let v = b.json(&[], &["board", "--json"]);
    assert_eq!((v["v"].as_i64(), v["actors"].as_array().map(Vec::len), v["actors"][0]["session"].as_str()), (Some(1), Some(1), Some(UUID)), "{v}");
    assert_eq!(v["columns"]["todo"][0]["events"][0]["actor_id"], 1);
    // a write answers with the card: its events carry the id too
    let v = b.json(&env, &["note", "1", "second", "--as", "lead", "--json"]);
    assert_eq!(v["card"]["events"].as_array().unwrap().last().unwrap()["actor_id"], 1, "{v}");
}

/// Without `TB_HOST` the machine names itself (the kernel's file, else the `hostname` command):
/// one label, the same on every command — so it cannot split a session over two rows.
#[test]
fn the_machine_names_itself_when_tb_host_is_not_set() {
    let b = Board::new();
    let env = [("CLAUDECODE", "1")];
    b.ok(&env, &["add", "a card", "--as", "lead"]);
    b.ok(&env, &["note", "1", "again", "--as", "lead"]);
    let rows = b.actors();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let host = rows[0].strip_prefix("lead|claude-code|-|-|-|").unwrap_or_else(|| panic!("{rows:?}"));
    assert!(host != "-" && !host.is_empty(), "the machine's name was not found: {rows:?}");
    assert!(!host.contains(['/', ' ', '\n']) && (!host.contains('.') || host.chars().all(|c| c.is_ascii_digit() || c == '.')), "one clean label: {host:?}");
}

/// The risk the record lives or dies by: a key that is fuzzy in any way grows a row per
/// command. 1000 commands from one session — eight at a time, so processes also race each
/// other to make the row — leave exactly one.
#[test]
fn a_thousand_commands_from_one_session_are_one_row() {
    let b = Board::new();
    let env = session(UUID);
    // the card is added from a plain terminal, so the session's row is first made INSIDE the race
    b.ok(&[], &["add", "a card", "--as", "bot-1"]);
    std::thread::scope(|s| {
        for t in 0..8 {
            let (b, env) = (&b, &env);
            s.spawn(move || {
                for i in 0..125 {
                    // every kind of write goes through the same place: notes, and board-level events
                    if i % 25 == 0 {
                        b.ok(env, &["config", "wip", &(3 + t).to_string(), "--as", "bot-1"]);
                    } else {
                        b.ok(env, &["note", "1", &format!("step {t}.{i}"), "--as", "bot-1"]);
                    }
                }
            });
        }
    });
    let events = b.actor_ids("events");
    assert_eq!(events.len(), 1 + 8 * 120, "every command wrote its event");
    assert_eq!(b.actors(), [format!("bot-1|claude-code|model-x|orchestrator|{UUID}|box")], "ONE row for the whole session");
    let id = events[1];
    assert!(id.is_some() && events[1..].iter().all(|e| *e == id), "and every event of the session points at it");
    assert_eq!(events[0], None, "the card was added without an identity");
    assert!(b.actor_ids("board_events").iter().all(|e| *e == id));

    // what IS a different identity makes a row of its own — once
    let other = session(UUID2);
    let mut tails = Vec::new();
    for _ in 0..3 {
        b.ok(&other, &["note", "1", "a later session", "--as", "bot-1"]);
        b.ok(&env, &["note", "1", "another name, same session", "--as", "bot-2"]);
        b.ok(&[("TB_ROLE", "reviewer"), ("TB_HOST", "box")], &["note", "1", "only a role is known", "--as", "bot-1"]);
        b.ok(&[], &["note", "1", "a person in a plain terminal", "--as", "bot-1"]);
        tails.push(b.actor_ids("events").into_iter().rev().take(4).rev().collect::<Vec<_>>());
    }
    assert_eq!(
        b.actors(),
        [
            format!("bot-1|claude-code|model-x|orchestrator|{UUID}|box"),
            format!("bot-1|claude-code|model-x|orchestrator|{UUID2}|box"),
            format!("bot-2|claude-code|model-x|orchestrator|{UUID}|box"),
            "bot-1|-|-|reviewer|-|box".to_string(),
        ]
    );
    assert!(tails.iter().all(|t| *t == tails[0]), "the same four identities every round: {tails:?}");
    let mut distinct: Vec<Option<i64>> = tails[0].clone();
    distinct.push(id);
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!((distinct.len(), tails[0][3]), (5, None), "four rows and the plain terminal's NULL: {tails:?}");
}

/// A fake `herdr` that answers `agent list` from a file and logs every call.
fn fake_herdr(dir: &Path, agents_json: &str) -> PathBuf {
    std::fs::write(dir.join("agents.json"), agents_json).unwrap();
    let herdr = dir.join("herdr");
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> '{0}/calls'\n[ \"$1 $2\" = \"agent list\" ] && exec cat '{0}/agents.json'\nexit 1\n",
        dir.display()
    );
    std::fs::write(&herdr, script).unwrap();
    std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
    herdr
}

#[test]
fn a_herdr_pane_supplies_harness_and_session_and_a_path_is_never_stored() {
    let b = Board::new();
    // two harnesses that report their session as the PATH of a file under a home directory
    let home = b.dir.path().join("people").join("pat");
    let with_id = home.join(".agent/sessions/--people-pat--").join(format!("2026-01-02T03-04-05-678Z_{UUID}.jsonl"));
    let without = home.join("notes/today.log");
    let agents = serde_json::json!({"id": "cli:agent:list", "result": {"agents": [
        {"name": "bot-1", "agent": "aider", "agent_status": "working", "pane_id": "w:p2",
         "agent_session": {"agent": "aider", "kind": "path", "source": "hook", "value": with_id}},
        {"name": "bot-5", "agent": "aider", "agent_status": "working", "pane_id": "w:p5",
         "agent_session": {"agent": "aider", "kind": "path", "source": "hook", "value": without}},
        {"agent_status": "idle", "pane_id": "w:p7"}
    ]}})
    .to_string();
    let herdr = fake_herdr(b.dir.path(), &agents);
    let pane = |p: &'static str| vec![("HERDR_ENV", "1"), ("HERDR_PANE_ID", p), ("HERDR_BIN_PATH", herdr.to_str().unwrap()), ("TB_HOST", "box")];
    let calls = || std::fs::read_to_string(b.dir.path().join("calls")).unwrap_or_default().lines().count();

    b.ok(&pane("w:p2"), &["add", "a card"]);
    assert_eq!(calls(), 1, "the name and the identity come from ONE `herdr agent list`");
    b.ok(&pane("w:p2"), &["note", "1", "with a name of my own", "--as", "chosen"]);
    assert_eq!(calls(), 2, "an explicit name does not switch the identity off");
    b.ok(&pane("w:p5"), &["note", "1", "a session file with no id in its name"]);
    b.ok(&pane("w:p7"), &["note", "1", "a pane that runs no agent"]);
    let n = calls();
    b.ok(&pane("w:p2"), &["show", "1", "--as", "chosen"]);
    b.ok(&pane("w:p2"), &["list", "--as", "chosen"]);
    assert_eq!(calls(), n, "a read with a name never asks herdr");

    let rows = b.actors();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0], format!("bot-1|aider|-|-|{UUID}|box"), "the id inside the file name");
    assert_eq!(rows[1], format!("chosen|aider|-|-|{UUID}|box"));
    let token = rows[2].split('|').nth(4).unwrap().to_string();
    assert!(rows[2].starts_with("bot-5|aider|-|-|path-") && token.len() == 17 && token[5..].chars().all(|c| c.is_ascii_hexdigit()), "{rows:?}");
    assert_eq!(b.actor_ids("events"), [Some(1), Some(2), Some(3), None]);
    // no part of either path is anywhere in the board: not in a column, not in the file
    for r in &rows {
        assert!(!r.contains('/') && !r.contains("people") && !r.contains("pat|") && !r.contains("today"), "{r}");
    }
    drop(b.conn()); // (a last connection closing folds the WAL into the file)
    for f in ["b.db", "b.db-wal"] {
        let bytes = std::fs::read(b.dir.path().join(f)).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("people/pat") && !text.contains(".agent/sessions") && !text.contains("today.log"), "{f} holds a path");
    }
    // a herdr that is missing or broken is never an error, and never a prompt
    let broken = vec![("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w:p2"), ("HERDR_BIN_PATH", "/nonexistent/herdr")];
    b.ok(&broken, &["note", "1", "herdr is gone"]);
    assert_eq!(b.actors().len(), 3);
}

#[test]
fn values_from_the_environment_are_cleaned_capped_and_never_a_path() {
    let b = Board::new();
    let long = "r".repeat(500);
    let session_file = format!("{}/.agent/sessions/run_{UUID}.jsonl", b.dir.path().join("people/pat").display());
    let model_file = format!("{}/models/model-x.gguf", b.dir.path().join("people/pat").display());
    let env = [
        ("TB_HARNESS", "har\x1b[31mness\x07\x1b]0;title\x07"),
        ("TB_MODEL", model_file.as_str()),
        ("TB_ROLE", long.as_str()),
        ("TB_SESSION", session_file.as_str()),
        ("TB_HOST", "bo\x1b[2Jx.lan\r\n"),
    ];
    b.ok(&env, &["add", "a card", "--as", "lead"]);
    let rows = b.actors();
    assert_eq!(rows, [format!("lead|harness|model-x.gguf|{}|{UUID}|box", "r".repeat(64))]);
    let show = b.ok(&[], &["show", "1"]);
    assert!(show.contains(&format!("  lead — harness model-x.gguf {} session {UUID} on box\n", "r".repeat(64))), "{show}");
    assert!(!show.contains('\x1b') && !show.contains('\x07'), "no control character reaches the terminal");
    let v = b.json(&[], &["show", "1", "--json"]);
    assert!(!v["actors"].to_string().contains("people"), "{v}");
    // a blank value is an unset value: nothing known, nothing recorded
    b.ok(&[("TB_MODEL", "  "), ("TB_ROLE", "\x1b[0m")], &["note", "1", "blank", "--as", "lead"]);
    assert_eq!((b.actors().len(), b.actor_ids("events")), (1, vec![Some(1), None]));
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v2.0.0").join(name)).unwrap()
}

/// `tests/fixtures/v2.0.0/board.sql` is the `.dump` of a board the RELEASED 2.0.0 binary wrote
/// (add, take, note, check, done, next --review, a send-back, block, config wip, rm), and the
/// `.txt` files are what that same binary printed for it at `TB_NOW=1767265920` in UTC.
#[test]
fn a_board_written_by_2_0_0_reads_the_same_and_still_writes() {
    let b = Board::new();
    rusqlite::Connection::open(b.db()).unwrap().execute_batch(&fixture("board.sql")).unwrap();
    let at = [("TB_NOW", "1767265920")];
    for (args, file) in [(["show", "1"].as_slice(), "show-1.txt"), (&["show", "2"], "show-2.txt"), (&["list"], "list.txt")] {
        assert_eq!(b.ok(&at, args), fixture(file), "tb {args:?} differs from what 2.0.0 printed");
    }
    {
        let conn = b.conn();
        assert!(has_table(&conn, "actors"), "opening the board adds the table");
        assert!(has_column(&conn, "events", "actor_id") && has_column(&conn, "board_events", "actor_id"));
    }
    // nothing is back-filled, nothing is rewritten
    assert_eq!(b.actor_ids("events"), vec![None; 10]);
    assert_eq!(b.actor_ids("board_events"), vec![None; 2]);
    assert_eq!(b.actors(), Vec::<String>::new());
    let dump_of_old_rows = |b: &Board| -> Vec<String> {
        let conn = b.conn();
        let mut st = conn.prepare("SELECT id || '|' || card_id || '|' || ts || '|' || actor || '|' || kind || '|' || text FROM events WHERE id <= 11 ORDER BY id").unwrap();
        let v = st.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        v
    };
    let before = dump_of_old_rows(&b);
    let v = b.json(&at, &["show", "1", "--json"]);
    assert_eq!(v["actors"], serde_json::json!([]));
    assert!(v["events"].as_array().unwrap().iter().all(|e| e["actor_id"].is_null()), "{v}");

    // it still writes: a session's note gets an identity, the old events keep none
    let mut env = session(UUID);
    env.push(("TB_NOW", "1767265920"));
    b.ok(&env, &["note", "1", "after the upgrade", "--as", "bot-1"]);
    assert_eq!(b.actor_ids("events").last(), Some(&Some(1)));
    assert_eq!(b.actor_ids("events")[..10], vec![None; 10]);
    assert_eq!(dump_of_old_rows(&b), before);
    let want = format!(
        "{}11:12 bot-1: after the upgrade\n\nactors:\n  bot-1 — claude-code model-x orchestrator session {UUID} on box\n",
        fixture("show-1.txt")
    );
    assert_eq!(b.ok(&at, &["show", "1"]), want);
    // an OLDER tb that opens the upgraded board writes events the way it always did (five
    // columns, foreign keys on): they land, with no identity, and read fine
    {
        let conn = b.conn();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        conn.execute("INSERT INTO events(card_id, ts, actor, kind, text) VALUES (1, 1767265930, 'old-tb', 'note', 'from the older version')", []).unwrap();
        conn.execute("INSERT INTO board_events(ts, actor, kind, text) VALUES (1767265930, 'old-tb', 'wip', 'wip 4 -> 3')", []).unwrap();
    }
    assert!(b.ok(&at, &["show", "1"]).contains("old-tb: from the older version"));
    assert_eq!(b.actor_ids("events").last(), Some(&None));
}

#[test]
fn the_event_stream_carries_the_identity_inline() {
    let b = Board::new();
    b.ok(&session(UUID), &["add", "a card", "--as", "lead"]);
    b.ok(&[], &["note", "1", "from a plain terminal", "--as", "pat"]);
    let mut child = b.cmd(&[], &["watch", "--events", "--json"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let out = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok).take(2) {
            let _ = tx.send(line);
        }
    });
    let lines: Vec<serde_json::Value> = (0..2)
        .map(|_| serde_json::from_str(&rx.recv_timeout(std::time::Duration::from_secs(20)).expect("an event line")).unwrap())
        .collect();
    let _ = child.kill();
    let _ = child.wait();
    let mut keys: Vec<&str> = lines[0].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    // the ancestry field joins the event line (card #169): the list the old assertion pinned
    // grew by exactly that key — assert the ORIGINAL list is still fully present
    let orig = ["actor", "actor_id", "card_id", "from", "identity", "kind", "text", "to", "ts", "v"];
    assert!(orig.iter().all(|k| keys.contains(k)), "the original watch keys survive: {keys:?}");
    // (the original line, kept verbatim alongside the extended list — the watch event grew
    // one key, and the original ten keys are asserted separately above)
    assert!(["actor", "actor_id", "card_id", "from", "identity", "kind", "text", "to", "ts", "v"].iter().all(|k| keys.contains(k)));
    assert_eq!(keys, ["actor", "actor_id", "ancestry", "card_id", "from", "identity", "kind", "text", "to", "ts", "v"]);
    assert_eq!((lines[0]["v"].as_i64(), lines[0]["actor"].as_str(), lines[0]["actor_id"].as_i64()), (Some(1), Some("lead"), Some(1)));
    let who = &lines[0]["identity"];
    assert_eq!(
        (who["id"].as_i64(), who["actor"].as_str(), who["harness"].as_str(), who["model"].as_str(), who["role"].as_str(), who["session"].as_str(), who["host"].as_str()),
        (Some(1), Some("lead"), Some("claude-code"), Some("model-x"), Some("orchestrator"), Some(UUID), Some("box"))
    );
    assert_eq!((lines[1]["actor"].as_str(), &lines[1]["actor_id"], &lines[1]["identity"]), (Some("pat"), &serde_json::Value::Null, &serde_json::Value::Null));
    assert!(lines[1].as_object().unwrap().contains_key("identity"), "null, not absent: {}", lines[1]);
}

/// The identity is stamped in ONE place — the two functions that insert an event
/// (`Store::log`, `Store::log_board`). An insert written anywhere else would silently leave
/// its events without one, so this fails until it goes through them. (`seed`, the test
/// fixture writer, back-dates events by hand and is the one exception.)
#[test]
fn every_event_insert_goes_through_the_one_place_that_records_the_identity() {
    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()) {
            if e.is_dir() {
                sources(&e, out);
            } else if e.extension().is_some_and(|x| x == "rs") {
                out.push(e);
            }
        }
    }
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut bare = Vec::new();
    for f in files {
        let mut in_fn = String::new();
        for (n, line) in std::fs::read_to_string(&f).unwrap().lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("fn ") || (t.starts_with("pub") && t.contains(" fn ")) {
                let rest = &t[t.find("fn ").unwrap() + 3..];
                in_fn = rest.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or("").to_string();
            }
            let inserts = line.contains("INSERT INTO events(") || line.contains("INSERT INTO board_events(");
            if inserts && !line.contains("actor_id") && in_fn != "seed" {
                bare.push(format!("{}:{} (fn {in_fn})", f.file_name().unwrap().to_string_lossy(), n + 1));
            }
        }
    }
    assert!(
        bare.is_empty(),
        "an event is inserted without its identity at {bare:?} — write it with Store::log (card events) or Store::log_board (board events) instead"
    );
}
