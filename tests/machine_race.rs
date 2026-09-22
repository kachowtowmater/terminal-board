//! One writer at a time in the machine-local settings file.
//!
//! The file holds one top-level key per feature, and it is about to hold a trust store, where
//! a silently reverted revocation fails OPEN. So a write by one process must never undo a
//! write by another. This test proves it with REAL PROCESSES: N writers, each owning its own
//! key, each counting up, plus the `tb` binary itself writing its own key, plus readers
//! looking for a torn file.
//!
//! The same race is run twice: once through `machine::update` (which takes the lock), and once
//! through the read-modify-write this file used to do without one — same temp file, same
//! fsync, same rename — so the numbers come from one machine in one run.
#![cfg(unix)]
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// A writer child: `KEY:ROUNDS:MODE` (mode `locked` or `unlocked`).
const ROLE: &str = "TB_RACE_ROLE";
const FILE: &str = "TB_RACE_FILE";

fn settings(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

/// What the old code did: read, change, write a temp file, fsync, rename. No lock.
fn unlocked_write(path: &Path, key: &str, value: i64) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut map: BTreeMap<String, Value> = if text.trim().is_empty() { BTreeMap::new() } else { serde_json::from_str(&text).unwrap_or_default() };
    map.insert(key.to_string(), json!(value));
    let out = serde_json::to_string_pretty(&map).unwrap();
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut f = std::fs::File::create(&tmp).unwrap();
    f.write_all(out.as_bytes()).unwrap();
    f.sync_all().unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

/// The child process: count from 1 to `rounds` in its own key.
fn writer(spec: &str, path: &Path) {
    let mut parts = spec.splitn(3, ':');
    let key = parts.next().unwrap().to_string();
    let rounds: i64 = parts.next().unwrap().parse().unwrap();
    let locked = parts.next().unwrap() == "locked";
    std::env::set_var("TB_CONFIG", path);
    for round in 1..=rounds {
        if locked {
            terminal_board::machine::update(|m| {
                m.insert(key.clone(), json!(round));
            })
            .unwrap_or_else(|e| panic!("{key} round {round}: {}", e.0));
        } else {
            unlocked_write(path, &key, round);
        }
    }
}

/// A child that reads as fast as it can and reports how often the file was not readable JSON.
fn reader(rounds: i64, path: &Path) {
    let mut torn = 0;
    for _ in 0..rounds {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if !text.trim().is_empty() && serde_json::from_str::<BTreeMap<String, Value>>(&text).is_err() {
            torn += 1;
        }
    }
    println!("torn {torn}");
    std::process::exit(if torn == 0 { 0 } else { 9 });
}

/// Run one race: `writers` children × `rounds` writes each, in their own keys.
/// Returns (saves, survived, lost, torn reads).
fn race(dir: &Path, writers: usize, rounds: i64, locked: bool) -> (i64, i64, i64, i64) {
    let path = settings(dir);
    let _ = std::fs::remove_file(&path);
    let me = std::env::current_exe().unwrap();
    let child = |role: &str| {
        Command::new(&me)
            .args(["the_race", "--exact", "--nocapture", "--test-threads=1"])
            .env(ROLE, role)
            .env(FILE, &path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut kids: Vec<_> = (1..=writers).map(|w| child(&format!("key{w}:{rounds}:{}", if locked { "locked" } else { "unlocked" }))).collect();
    let mut readers = vec![child(&format!("read:{}:x", rounds * 4))];
    for k in kids.iter_mut().chain(readers.iter_mut()) {
        let _ = k.id();
    }
    let mut torn = 0;
    for k in kids {
        let o = k.wait_with_output().unwrap();
        assert!(o.status.success(), "a writer failed: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    }
    for r in readers {
        let o = r.wait_with_output().unwrap();
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        let n: i64 = out.lines().find_map(|l| l.strip_prefix("torn ")).and_then(|n| n.trim().parse().ok()).unwrap_or(0);
        torn += n;
    }
    // every writer's last save must be what its key holds
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let map: BTreeMap<String, Value> = serde_json::from_str(&text).expect("the settings file is valid JSON at the end");
    let saves = writers as i64 * rounds;
    let mut survived = 0;
    for w in 1..=writers {
        if map.get(&format!("key{w}")).and_then(Value::as_i64) == Some(rounds) {
            survived += rounds;
        } else {
            // a key that fell behind: everything after its surviving value was reverted
            survived += map.get(&format!("key{w}")).and_then(Value::as_i64).unwrap_or(0);
        }
    }
    (saves, survived, saves - survived, torn)
}

/// The proof. Also the child entry point: a child re-runs this same test with a role set.
#[test]
fn the_race() {
    let path = std::env::var(FILE).map(PathBuf::from);
    if let (Ok(role), Ok(path)) = (std::env::var(ROLE), path) {
        let mut parts = role.splitn(3, ':');
        if parts.next() == Some("read") {
            let rounds: i64 = parts.next().unwrap().parse().unwrap();
            reader(rounds, &path);
            return;
        }
        return writer(&role, &path);
    }
    let dir = tempfile::tempdir().unwrap();
    let mut table = String::from("shape             saves survived lost      torn\n");
    let mut worst_locked = 0;
    for (writers, rounds) in [(3usize, 60i64), (5, 40)] {
        for locked in [false, true] {
            let (saves, survived, lost, torn) = race(dir.path(), writers, rounds, locked);
            table.push_str(&format!(
                "{:<9} {:<7} {saves:<5} {survived:<8} {lost:<3} ({:>4.1}%) {torn}\n",
                format!("{writers}x{rounds}"),
                if locked { "LOCKED" } else { "unlocked" },
                100.0 * lost as f64 / saves as f64
            ));
            if locked {
                worst_locked = worst_locked.max(lost);
                assert_eq!(lost, 0, "a locked write was lost — the settings file is not safe for a trust store\n{table}");
                assert_eq!(torn, 0, "a reader saw a torn settings file\n{table}");
            }
        }
    }
    println!("{table}");
    assert_eq!(worst_locked, 0);
}

/// The real `tb` binary writing its own key while another process writes a different one:
/// the lock is inside `machine::update`, so every writer takes it, whatever the command.
#[test]
fn the_tb_binary_and_another_writer_share_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let path = home.join("config.json");
    let tb = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).env("HOME", home).env("TB_CONFIG", &path).env("TB_AS", "tester").env("TB_NO_HERDR", "1");
        for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD"] {
            c.env_remove(k);
        }
        c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        c.output().unwrap()
    };
    for b in ["one", "two"] {
        assert!(tb(&[b, "add", "x: a card"]).status.success());
    }
    // a foreign key, written by a child, while tb flips its own key back and forth
    let me = std::env::current_exe().unwrap();
    let foreign = Command::new(&me)
        .args(["the_race", "--exact", "--nocapture", "--test-threads=1"])
        .env(ROLE, "hooks:80:locked")
        .env(FILE, &path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    for i in 0..40 {
        let board = if i % 2 == 0 { "one" } else { "two" };
        let o = tb(&["boards", "--default", board]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        std::thread::sleep(Duration::from_millis(1));
    }
    let o = foreign.wait_with_output().unwrap();
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let map: BTreeMap<String, Value> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(map.get("hooks"), Some(&json!(80)), "tb reverted another writer's key: {map:?}");
    assert_eq!(map.get("default_board"), Some(&json!("two")), "the other writer reverted tb's key: {map:?}");
}

/// A lock nobody holds never blocks anyone: the `.lock` file is left behind on purpose, and
/// its existence means nothing (the lock itself dies with the process that held it).
#[test]
fn a_leftover_lock_file_blocks_nobody() {
    let dir = tempfile::tempdir().unwrap();
    let path = settings(dir.path());
    std::env::set_var("TB_CONFIG", &path);
    terminal_board::machine::update(|m| {
        m.insert("default_board".into(), json!("work"));
    })
    .unwrap();
    let lock = dir.path().join(".config.json.lock");
    assert!(lock.exists(), "no lock file was made");
    // a lock file left by a process that is long gone (this is exactly what a crash leaves)
    drop(std::fs::File::create(&lock).unwrap());
    terminal_board::machine::update(|m| {
        m.insert("default_board".into(), json!("home"));
    })
    .expect("a leftover lock file wedged the settings");
    assert_eq!(terminal_board::machine::load().unwrap()["default_board"], "home");
    std::env::remove_var("TB_CONFIG");
}
