//! How `lock::take` waits (src/lock.rs): first come, first served, so a waiter is not starved
//! by a process that lets go and takes the lock again at once, and a waiter that gives up or
//! dies never holds up the queue. (That a take which gives up leaves no thread or open file
//! behind is in `tests/lock_timeout.rs`, alone in its binary: it counts this process's threads.)
#![cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use terminal_board::lock::{self, Error, Mode};

/// The tests here run one at a time (each spawns threads or a child of its own).
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A holder that lets go and asks again at once (a loop of writes) must not keep the lock from
/// a process that was already waiting for it.
#[test]
fn a_waiter_is_not_starved_by_a_holder_that_takes_it_again_at_once() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let file = lock::sibling(&dir.path().join("board.db"));
    let stop = Arc::new(AtomicBool::new(false));
    let hog = {
        let (file, stop) = (file.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let g = lock::take(&file, Mode::Exclusive, Duration::from_secs(10)).unwrap();
                std::thread::sleep(Duration::from_millis(20));
                drop(g);
            }
        })
    };
    std::thread::sleep(Duration::from_millis(50)); // the hog is looping
    for round in 0..5 {
        let got = lock::take(&file, Mode::Exclusive, Duration::from_secs(3));
        assert!(got.is_ok(), "round {round}: a waiter lost to a holder re-taking the lock for 3s: {:?}", got.err());
        drop(got);
    }
    stop.store(true, Ordering::Relaxed);
    hog.join().unwrap();
}

/// The queue beside a lock file (`src/lock.rs`, `queue_dir`): numbered ticket files.
fn tickets(file: &std::path::Path) -> Vec<u64> {
    let q = file.with_file_name(format!("{}.q", file.file_name().unwrap().to_str().unwrap()));
    let mut v: Vec<u64> = std::fs::read_dir(q)
        .map(|rd| rd.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).collect())
        .unwrap_or_default();
    v.sort_unstable();
    v
}

/// Wait (bounded) until the queue holds `n` tickets — so waiters are known to have asked in
/// order. A lock with no queue at all (the code before it) gets a fixed pause instead, so the
/// tests still run against it and fail on what it DOES, not on a missing file.
fn until_queued(file: &std::path::Path, n: usize) {
    let start = std::time::Instant::now();
    while tickets(file).len() < n {
        if start.elapsed() > Duration::from_millis(300) && tickets(file).is_empty() {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "never saw {n} waiters queue: {:?}", tickets(file));
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// First come, first served: waiters get the lock in the order they asked for it, whatever
/// the timing of their polls — the release is never a lottery among them.
#[test]
fn waiters_get_the_lock_in_the_order_they_came() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let file = lock::sibling(&dir.path().join("board.db"));
    for round in 0..5 {
        let holder = lock::take(&file, Mode::Exclusive, Duration::ZERO).unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut waiters = Vec::new();
        for i in 0..6 {
            let (f, order) = (file.clone(), order.clone());
            waiters.push(std::thread::spawn(move || {
                let g = lock::take(&f, Mode::Exclusive, Duration::from_secs(10)).unwrap();
                order.lock().unwrap().push(i);
                std::thread::sleep(Duration::from_millis(2));
                drop(g);
            }));
            until_queued(&file, i + 1); // waiter i has its ticket before i+1 asks
        }
        // queued waiters mean a zero wait is refused at once, never queued behind them
        let t = std::time::Instant::now();
        assert!(matches!(lock::take(&file, Mode::Exclusive, Duration::ZERO), Err(Error::Busy(_))));
        assert!(t.elapsed() < Duration::from_secs(1), "a zero-wait take waited");
        drop(holder);
        for w in waiters {
            w.join().unwrap();
        }
        assert_eq!(*order.lock().unwrap(), (0..6).collect::<Vec<_>>(), "round {round}: not first come, first served");
        assert!(tickets(&file).is_empty(), "round {round}: tickets left behind: {:?}", tickets(&file));
    }
}

/// A waiter that gives up leaves the queue: the next one gets the lock the moment it is free.
#[test]
fn a_waiter_that_gave_up_does_not_hold_up_the_queue() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let file = lock::sibling(&dir.path().join("board.db"));
    let holder = lock::take(&file, Mode::Exclusive, Duration::ZERO).unwrap();
    // first in the queue, and it gives up
    assert!(matches!(lock::take(&file, Mode::Exclusive, Duration::from_millis(50)), Err(Error::Busy(_))));
    assert!(tickets(&file).is_empty(), "the waiter that gave up left its ticket: {:?}", tickets(&file));
    let next = {
        let file = file.clone();
        std::thread::spawn(move || {
            let g = lock::take(&file, Mode::Exclusive, Duration::from_secs(10)).unwrap();
            let got = std::time::Instant::now();
            drop(g);
            got
        })
    };
    until_queued(&file, 1);
    let released = std::time::Instant::now();
    drop(holder);
    let got = next.join().unwrap();
    assert!(got.duration_since(released) < Duration::from_secs(2), "the next waiter waited {:?} after the release", got.duration_since(released));
}

/// The child for the test below: queue for the lock and wait until killed.
const QUEUE_CHILD: &str = "TB_LOCK_QUEUE_CHILD";

#[test]
fn queued_child() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let Ok(file) = std::env::var(QUEUE_CHILD) else { return };
    let _ = lock::take(std::path::Path::new(&file), Mode::Exclusive, Duration::from_secs(60));
}

/// A waiter killed while queued (its process gone, its ticket file left behind) does not hold
/// up the queue: the kernel let go of its ticket with the process, and the next waiter skips it.
#[test]
fn a_waiter_killed_in_the_queue_does_not_hold_up_the_queue() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let file = lock::sibling(&dir.path().join("board.db"));
    let holder = lock::take(&file, Mode::Exclusive, Duration::ZERO).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "queued_child", "--nocapture"])
        .env(QUEUE_CHILD, &file)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    until_queued(&file, 1);
    child.kill().unwrap(); // SIGKILL: no chance to remove its ticket
    child.wait().unwrap();
    // its ticket is still there, as after a real crash (with a queue at all)
    assert!(tickets(&file).len() <= 1);
    let next = {
        let file = file.clone();
        std::thread::spawn(move || {
            let g = lock::take(&file, Mode::Exclusive, Duration::from_secs(10)).unwrap();
            let got = std::time::Instant::now();
            drop(g);
            got
        })
    };
    until_queued(&file, 1);
    let released = std::time::Instant::now();
    drop(holder);
    let got = next.join().unwrap();
    assert!(got.duration_since(released) < Duration::from_secs(2), "the dead waiter held up the queue for {:?}", got.duration_since(released));
    assert!(tickets(&file).is_empty(), "tickets left: {:?}", tickets(&file));
}

/// Uncontended, a take is one try and creates nothing but the lock file itself.
#[test]
fn an_uncontended_take_creates_no_queue() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let file = lock::sibling(&dir.path().join("board.db"));
    for _ in 0..3 {
        drop(lock::take(&file, Mode::Exclusive, Duration::ZERO).unwrap());
        drop(lock::take(&file, Mode::Shared, Duration::from_secs(1)).unwrap());
    }
    let names: Vec<String> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, [".board.db.lock"], "an uncontended take made files");
}
