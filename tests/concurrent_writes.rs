//! #85: concurrent `tb add` used to fail instantly with "database is locked" instead of
//! waiting out the store's own 10s busy timeout. Root cause: `add` (and several other write
//! paths) opened a plain (deferred) transaction that reads first (e.g. `bottom_of`, finding
//! the next position) and only writes second — SQLite does not run the busy handler for the
//! read-to-write lock UPGRADE, only for a fresh lock request, so two concurrent writers each
//! holding a read lock hit SQLITE_BUSY immediately. Starting every write transaction as
//! `BEGIN IMMEDIATE` (store.rs, store/links.rs, store/blocks.rs) takes the write lock on the
//! first statement, so the busy timeout applies and a second writer queues instead of failing.
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Board {
    dir: tempfile::TempDir,
}

impl Board {
    fn new() -> Board {
        Board { dir: tempfile::tempdir().unwrap() }
    }
    fn db(&self) -> PathBuf {
        self.dir.path().join("board.db")
    }
    fn cmd(&self, who: &str, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args)
            .env("HOME", self.dir.path().join("home"))
            .env("TB_DB", self.db())
            .env("TB_AS", who)
            .env("TB_NO_HERDR", "1")
            .env_remove("HERDR_AGENT_NAME")
            .env_remove("TB_BOARD")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }
    fn run(&self, who: &str, args: &[&str]) -> Output {
        self.cmd(who, args).output().unwrap()
    }
    fn ok(&self, who: &str, args: &[&str]) -> String {
        let o = self.run(who, args);
        assert!(o.status.success(), "{args:?} as {who} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    }
}

/// The card #85 reproducer, run as an in-process test instead of a shell one-liner: seed a
/// card, then 20 rounds of 4 simultaneous `tb add` processes (started together with `spawn`,
/// waited on before the next round — the same shape as shell `&` + `wait`). On the deferred-
/// transaction bug this measured ok=20 refused=60 (60 instant "database is locked" failures
/// out of 80). With `BEGIN IMMEDIATE`, a contended writer queues behind the busy timeout
/// instead of failing, so all 80 succeed.
#[test]
fn concurrent_add_storm_all_succeed() {
    let b = Board::new();
    b.ok("seed", &["add", "seed: card"]);
    let rounds = 20;
    let per_round = 4;
    let mut ok = 0i64;
    let mut refused = 0i64;
    let mut refusal_texts: Vec<String> = Vec::new();
    let start = Instant::now();
    for round in 0..rounds {
        let children: Vec<_> = (0..per_round)
            .map(|i| {
                let title = format!("r{round}c{i}: storm card");
                b.cmd("rv", &["add", &title]).spawn().unwrap()
            })
            .collect();
        for child in children {
            let o = child.wait_with_output().unwrap();
            if o.status.success() {
                ok += 1;
            } else {
                refused += 1;
                let text = String::from_utf8_lossy(&o.stderr).trim().to_string();
                if refusal_texts.len() < 3 {
                    refusal_texts.push(text);
                }
            }
        }
    }
    let elapsed = start.elapsed();
    let total = rounds * per_round;
    println!("storm: ok={ok} refused={refused} of {total} in {elapsed:?}");
    for t in &refusal_texts {
        println!("  refusal: {t}");
    }
    // no data loss either way: every reported success is a real row, and nothing more
    let list = b.ok("seed", &["list", "--json"]);
    let cards: serde_json::Value = serde_json::from_str(&list).unwrap();
    let n = cards.as_array().unwrap().len() as i64;
    assert_eq!(n, 1 + ok, "board holds {n} cards but {ok} adds reported success (+1 seed)");
    assert_eq!(
        refused, 0,
        "storm: {ok} ok / {refused} refused out of {total} (expected all to succeed — see refusal text above)"
    );
}

/// The same storm on a board that does not exist yet: every `add` opens a new file, and the
/// switch into WAL mode (which SQLite does without its busy handler) used to fail with
/// "database is locked" whenever two of them got there together. 20 rounds of 8 simultaneous
/// adds, each round on a new board: every add must succeed and every card must be there.
#[test]
fn concurrent_adds_on_a_new_board_all_succeed() {
    let rounds = 20;
    let per_round = 8;
    for round in 0..rounds {
        let b = Board::new();
        let children: Vec<_> = (0..per_round)
            .map(|i| b.cmd("rv", &["add", &format!("r{round}c{i}: new board card")]).spawn().unwrap())
            .collect();
        for child in children {
            let o = child.wait_with_output().unwrap();
            assert!(
                o.status.success(),
                "round {round}: an add on a new board failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
        }
        let list = b.ok("rv", &["list", "--json"]);
        let cards: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!(cards.as_array().unwrap().len(), per_round, "round {round}: cards on the new board");
    }
}

/// The ordinary case (#85): one process, nothing to contend with. `BEGIN IMMEDIATE` must not
/// make an uncontended writer slower than a deferred transaction would — there is no lock to
/// wait for either way, so the two behave the same when nobody else is writing.
#[test]
fn single_process_add_stays_fast() {
    let b = Board::new();
    let n = 50u32;
    let start = Instant::now();
    for i in 0..n {
        b.ok("solo", &["add", &format!("t{i}: card")]);
    }
    let elapsed = start.elapsed();
    let per = elapsed / n;
    println!("single-process: {n} adds in {elapsed:?} ({per:?} each)");
    // generous bound: this catches a real regression (e.g. an extra round trip added per
    // write), not ordinary machine-to-machine variance in process-spawn overhead
    assert!(per < Duration::from_millis(500), "an uncontended add averaged {per:?} — investigate before calling this 'no slower'");
}
