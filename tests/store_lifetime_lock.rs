//! A board file's lifetime is locked, not just its bytes (#112): `Store::open` takes the
//! SHARED lock (`crate::lock`) for as long as it lives; a command that moves or replaces a
//! board file takes the EXCLUSIVE half across the whole operation (`store::lock_for_move`,
//! `store::link_into_place`). This PR adds no `archive`/`restore` command — that is future
//! work on top of this — so the "mover" here is the same primitive a real one would call,
//! driven directly, real processes racing real `Store::open`s exactly as `tb add` does.
//!
//! Two races, the ones measured (unlocked) on the branch that found this bug (#80/#112):
//! - `restore_vs_add`: a board is "archived" (moved out of its slot); racers either try to
//!   restore it or `add` a fresh card, at the same instant, board-name creation racing itself.
//! - `archive_vs_add`: a live board is "archived" (moved to a retired name) while writers try
//!   to `add` to the slot it just vacated.
//!
//! `TB_RACE_RACERS` (comma list, default `2,4`) and `TB_RACE_ROUNDS` (default `30`) size the
//! run — the acceptance numbers (2/4/8 racers × 100+ rounds) are reproduced with
//! `TB_RACE_RACERS=2,4,8 TB_RACE_ROUNDS=150 cargo test --release --test store_lifetime_lock`.
//! `TB_LOCK_WAIT_MS` (the crate's own hook) can shrink the wait for the refusal/timing tests.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use terminal_board::store::{self, Store};

const ROLE: &str = "TB_LOCK_TEST_ROLE";
const FILE: &str = "TB_LOCK_TEST_FILE";

fn sizes() -> (Vec<usize>, u32) {
    let racers = std::env::var("TB_RACE_RACERS")
        .ok()
        .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![2usize, 4]);
    let rounds = std::env::var("TB_RACE_ROUNDS").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
    (racers, rounds)
}

fn me() -> PathBuf {
    std::env::current_exe().unwrap()
}

/// Spawn a child that re-runs `test_name`, in role `role`, against board file `path`.
fn spawn(test_name: &str, role: &str, path: &Path) -> std::process::Child {
    Command::new(me())
        .args([test_name, "--exact", "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .env(FILE, path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Titles on the board at `path` — or, when there is no file there, an empty list without
/// ever calling `Store::open` on it: `open` CREATES what it does not find, which is exactly
/// the defect under test and must never happen as a side effect of an assertion helper.
fn titles(path: &Path) -> Vec<String> {
    if !path.exists() {
        return Vec::new();
    }
    let Ok(s) = Store::open(path) else { return Vec::new() };
    s.snapshot().map(|snap| store::COLUMNS.iter().flat_map(|c| snap.in_column(c)).map(|c| c.title.clone()).collect()).unwrap_or_default()
}

// Result markers go to STDERR, never stdout: cargo test's own harness prints `test NAME ... `
// on stdout with NO trailing newline until the test finishes, so a `println!` from inside the
// test body lands mid-line, right after that prefix — `line.strip_prefix("OK ")` then never
// matches (the line is "test NAME ... OK …", not "OK …"). Stderr is untouched by the harness,
// so a marker there is always alone on its own line, whatever cargo test's own format does.

/// child role `add:NAME`: one `tb add`-shaped write. Reports `OK <title>` or `FAIL <err>`.
fn do_add(path: &Path, title: &str) {
    match Store::open(path).and_then(|s| s.add(title, "", &[], "racer")) {
        Ok(_) => eprintln!("OK {title}"),
        Err(e) => eprintln!("FAIL {e}"),
    }
}

/// child role `restore`: one restore attempt, `archived` -> `path`. Reports `OK`, `LOST-RACE`
/// (a clean refusal — the slot filled, or another restorer already consumed the source) or
/// `FAIL <err>` for anything else.
fn do_restore(path: &Path, archived: &Path) {
    match store::lock_for_move(path) {
        Ok(_guard) => match store::link_into_place(archived, path) {
            Ok(()) => eprintln!("OK"),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists || e.kind() == std::io::ErrorKind::NotFound => eprintln!("LOST-RACE"),
            Err(e) => eprintln!("FAIL {e}"),
        },
        Err(e) => eprintln!("FAIL {e}"),
    }
}

/// child role `archive`: one archive attempt, `path` -> a destination unique to THIS archiver
/// (its own pid, the way a real implementation would use its own timestamp) — never a name
/// shared with other racers, so a plain rename to it is correct exactly because two archivers
/// can never legitimately target the same file (the anti-clobber primitive is for placing
/// INTO a shared slot, which `restore` does — see `do_restore`). Reports `OK <dest>`,
/// `LOST-RACE` (nothing there once this archiver's turn came — an `add` may since have
/// recreated it, which is its own, separate `OK <dest>` for the fresh board it took away) or
/// `FAIL <err>`.
fn do_archive(path: &Path) {
    match store::lock_for_move(path) {
        Ok(_guard) => {
            if !path.exists() {
                eprintln!("LOST-RACE");
            } else {
                let dest = path.with_extension(format!("archived-{}", std::process::id()));
                match std::fs::rename(path, &dest) {
                    Ok(()) => eprintln!("OK {}", dest.display()),
                    Err(e) => eprintln!("FAIL {e}"),
                }
            }
        }
        Err(e) => eprintln!("FAIL {e}"),
    }
}

fn seed(path: &Path) {
    let s = Store::open(path).unwrap();
    s.add("one", "", &[], "seed").unwrap();
    s.add("two", "", &[], "seed").unwrap();
}

/// The proof, and the child entry point (a child re-runs this same test with a role set).
#[test]
fn restore_vs_add() {
    if let (Ok(role), Ok(path)) = (std::env::var(ROLE), std::env::var(FILE).map(PathBuf::from)) {
        let mut parts = role.splitn(2, ':');
        match parts.next().unwrap() {
            "add" => return do_add(&path, parts.next().unwrap()),
            "restore" => return do_restore(&path, &path.with_extension("archived")),
            other => panic!("unknown role {other}"),
        }
    }
    let (racer_counts, rounds) = sizes();
    let dir = tempfile::tempdir().unwrap();
    let mut summary = String::new();
    for &n in &racer_counts {
        let mut lost_committed = 0u32;
        let mut lost_seed = 0u32;
        for rnd in 0..rounds {
            let path = dir.path().join(format!("r{n}-{rnd}.db"));
            let archived = path.with_extension("archived");
            seed(&path);
            // archive it up front, synchronously — the race is only about getting it BACK
            {
                let _g = store::lock_for_move(&path).unwrap();
                std::fs::rename(&path, &archived).unwrap();
            }
            let nr = n / 2;
            let mut kids: Vec<(&str, std::process::Child)> = Vec::new();
            for _ in 0..nr {
                kids.push(("restore", spawn("restore_vs_add", "restore", &path)));
            }
            for j in 0..(n - nr) {
                kids.push(("add", spawn("restore_vs_add", &format!("add:w{rnd}x{j}"), &path)));
            }
            let mut committed = Vec::new();
            for (kind, k) in kids {
                let o = k.wait_with_output().unwrap();
                let err = String::from_utf8_lossy(&o.stderr).to_string();
                assert!(o.status.success(), "{kind} child crashed: {}{err}", String::from_utf8_lossy(&o.stdout));
                if kind == "add" {
                    if let Some(t) = err.lines().find_map(|l| l.strip_prefix("OK ")) {
                        committed.push(t.to_string());
                    }
                }
            }
            // every add that reported OK must be in whatever is live afterward
            let live = titles(&path);
            for t in &committed {
                if !live.contains(t) {
                    lost_committed += 1;
                }
            }
            // the seed cards must survive somewhere: restored live, or still in the untouched
            // archive (a restore that lost the race never removes its source — see
            // `link_into_place`). Guarded on `exists()` first: `titles` goes through
            // `Store::open`, which creates what it does not find — exactly the bug under
            // test, never something this check should trigger by accident.
            let found_seed = live.contains(&"one".to_string()) || (archived.exists() && titles(&archived).contains(&"one".to_string()));
            if !found_seed {
                lost_seed += 1;
            }
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(&archived);
        }
        summary.push_str(&format!("restore_vs_add n={n} rounds={rounds} lost_committed_adds={lost_committed} lost_seed_rounds={lost_seed}\n"));
    }
    println!("{summary}");
    assert!(summary.lines().all(|l| l.contains("lost_committed_adds=0") && l.contains("lost_seed_rounds=0")), "{summary}");
}

#[test]
fn archive_vs_add() {
    if let (Ok(role), Ok(path)) = (std::env::var(ROLE), std::env::var(FILE).map(PathBuf::from)) {
        let mut parts = role.splitn(2, ':');
        match parts.next().unwrap() {
            "add" => return do_add(&path, parts.next().unwrap()),
            "archive" => return do_archive(&path),
            other => panic!("unknown role {other}"),
        }
    }
    let (racer_counts, rounds) = sizes();
    let dir = tempfile::tempdir().unwrap();
    let mut summary = String::new();
    for &n in &racer_counts {
        let mut lost_seed = 0u32;
        let mut zero_byte = 0u32;
        for rnd in 0..rounds {
            let path = dir.path().join(format!("a{n}-{rnd}.db"));
            seed(&path);
            let na = n / 2;
            let mut kids: Vec<(&str, std::process::Child)> = Vec::new();
            for _ in 0..na {
                kids.push(("archive", spawn("archive_vs_add", "archive", &path)));
            }
            for j in 0..(n - na) {
                kids.push(("add", spawn("archive_vs_add", &format!("add:x{rnd}x{j}"), &path)));
            }
            let mut archived_at = Vec::new();
            for (kind, k) in kids {
                let o = k.wait_with_output().unwrap();
                let err = String::from_utf8_lossy(&o.stderr).to_string();
                assert!(o.status.success(), "{kind} child crashed: {}{err}", String::from_utf8_lossy(&o.stdout));
                if kind == "archive" {
                    if let Some(p) = err.lines().find_map(|l| l.strip_prefix("OK ")) {
                        archived_at.push(PathBuf::from(p.trim()));
                    }
                }
            }
            // the seed cards must survive somewhere: still live (every archiver lost the
            // race, or an `add` recreated the slot after a winning archive took it away), or
            // in ANY of the (possibly several — each archiver's own unique destination, #112)
            // archived copies. An archiver that moved an `add`-recreated board away is a
            // legitimate second archive, not a loss (`race88b.py` called this "by design" too).
            let found_seed = (path.exists() && titles(&path).contains(&"one".to_string()))
                || archived_at.iter().any(|p| titles(p).contains(&"one".to_string()));
            if !found_seed {
                lost_seed += 1;
            }
            let mut all = archived_at.clone();
            all.push(path.clone());
            for p in &all {
                if p.exists() && std::fs::metadata(p).unwrap().len() == 0 {
                    zero_byte += 1;
                }
            }
            for p in &all {
                let _ = std::fs::remove_file(p);
            }
        }
        summary.push_str(&format!("archive_vs_add n={n} rounds={rounds} lost_seed_rounds={lost_seed} zero_byte={zero_byte}\n"));
    }
    println!("{summary}");
    assert!(summary.lines().all(|l| l.contains("lost_seed_rounds=0") && l.contains("zero_byte=0")), "{summary}");
}

/// A board genuinely held open refuses a move, naming the process — and a killed holder
/// leaves nothing stale: the wait it would have made a mover sit out ends the instant the
/// process dies, well under the bound, because the lock is the kernel's.
#[test]
fn a_held_open_board_refuses_a_move_and_a_killed_holder_frees_it_immediately() {
    if let (Ok(role), Ok(path)) = (std::env::var(ROLE), std::env::var(FILE).map(PathBuf::from)) {
        if role == "hold" {
            let _s = Store::open(&path).unwrap();
            std::thread::sleep(Duration::from_secs(30));
            return;
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("held.db");
    seed(&path);
    std::env::set_var("TB_LOCK_WAIT_MS", "300");
    let mut holder = spawn("a_held_open_board_refuses_a_move_and_a_killed_holder_frees_it_immediately", "hold", &path);
    std::thread::sleep(Duration::from_millis(150)); // let it actually open and take the shared lock
    let e = store::lock_for_move(&path).unwrap_err();
    let msg = e.to_string();
    assert!(msg.contains("still open in another process"), "{msg}");
    assert!(msg.contains(&format!("{}", holder.id())) || msg.contains("close whatever process"), "refusal did not name a process: {msg}");
    // kill -9 it: the lock must release with the process, not linger for the rest of the wait
    unsafe { libc::kill(holder.id() as i32, libc::SIGKILL) };
    let start = Instant::now();
    let g = store::lock_for_move(&path);
    assert!(g.is_ok(), "the lock stayed busy after its holder was killed");
    assert!(start.elapsed() < Duration::from_millis(300), "took {:?} — a killed holder should free the lock almost immediately", start.elapsed());
    let _ = holder.kill();
    let _ = holder.wait();
    std::env::remove_var("TB_LOCK_WAIT_MS");
}

/// `link_into_place` never clobbers: a file already at the destination is left exactly as it
/// was, and the source is never removed on that path (the caller can retry, or report it).
#[test]
fn link_into_place_refuses_rather_than_clobbers() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("from.db");
    let to = dir.path().join("to.db");
    std::fs::write(&from, b"archived").unwrap();
    std::fs::write(&to, b"already-there").unwrap();
    let e = store::link_into_place(&from, &to).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists, "{e}");
    assert_eq!(std::fs::read(&to).unwrap(), b"already-there", "the existing file was touched");
    assert_eq!(std::fs::read(&from).unwrap(), b"archived", "the source was removed despite the failed link");

    let to2 = dir.path().join("to2.db");
    store::link_into_place(&from, &to2).unwrap();
    assert_eq!(std::fs::read(&to2).unwrap(), b"archived");
    assert!(!from.exists(), "the source must be gone once the link succeeded");
}
