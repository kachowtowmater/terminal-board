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
use std::time::Instant;

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
/// wait for either way. That is a property of what tb DOES, so it is counted, not timed (a
/// 500ms-per-add bound failed on a busy disk with nothing wrong in tb):
///
/// - every deliberate wait in the store (a SQLite busy retry, a WAL-switch retry, a back-off
///   sleep) goes through `waits::pause`, which `TB_TRACE_WAITS` records — and none may happen;
/// - `TB_LOCK_WAIT_MS=0` turns any wait for a board lock into an immediate refusal, so an add
///   that had to queue for a lock (even one of its own) fails instead of taking longer.
///
/// `store_sleeps_only_through_the_wait_trace` below keeps a plain sleep from being added to
/// the store where the trace cannot see it. What this covers is the store's write path, the
/// one an `add` shares with every write; a lock wait that QUEUES is caught by
/// `TB_LOCK_WAIT_MS=0`. The command layer in `src/main.rs` (what runs before the store is
/// opened) is not traced: a sleep there is outside what this proves.
#[test]
fn single_process_add_stays_fast() {
    let b = Board::new();
    let trace = b.dir.path().join("waits.log");
    let n = 50u32;
    let start = Instant::now();
    for i in 0..n {
        let o = b
            .cmd("solo", &["add", &format!("t{i}: card")])
            .env("TB_TRACE_WAITS", &trace)
            .env("TB_LOCK_WAIT_MS", "0")
            .output()
            .unwrap();
        assert!(o.status.success(), "add {i} had to wait for a lock: {}", String::from_utf8_lossy(&o.stderr));
    }
    // for the log only: the time depends on the machine and its disk, the waits do not
    println!("single-process: {n} adds in {:?}", start.elapsed());
    let waits = std::fs::read_to_string(&trace).unwrap_or_default();
    assert!(waits.is_empty(), "an uncontended add waited ({} times):\n{waits}", waits.lines().count());
    let list = b.ok("solo", &["list", "--json"]);
    let cards: serde_json::Value = serde_json::from_str(&list).unwrap();
    assert_eq!(cards.as_array().unwrap().len(), n as usize, "every add landed");
}

/// The store's waits are all visible to the trace: nothing in `src/store.rs` or `src/store/`
/// sleeps except through `waits::pause`, so a sleep added to the store's write path shows up
/// in `single_process_add_stays_fast` instead of only making it slower. (Test modules sleep
/// on purpose and are not scanned: each file is read up to its `#[cfg(test)]`.)
#[test]
fn store_sleeps_only_through_the_wait_trace() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![src.join("store.rs")];
    for e in std::fs::read_dir(src.join("store")).unwrap().flatten() {
        files.push(e.path());
    }
    let mut hits = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        for (n, line) in text.lines().enumerate().take_while(|(_, l)| l.trim() != "#[cfg(test)]") {
            let code = line.split("//").next().unwrap_or("");
            if code.contains("thread::sleep(") || code.contains("busy_timeout(") {
                hits.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(hits.is_empty(), "sleep through waits::pause (and set the busy wait with waits::busy) so the wait trace sees it:\n{}", hits.join("\n"));
}
