//! How `lock::take` waits (src/lock.rs): every try is non-blocking on the calling thread, so a
//! take that gives up leaves nothing behind, and a waiter is not starved by a process that lets
//! go and takes the lock again at once.
#![cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use terminal_board::lock::{self, Error, Mode};

/// The two tests count this process's threads and open files: never at the same time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

#[cfg(target_os = "linux")]
fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}

/// How many of this process's open files are `file` (the lock file itself).
#[cfg(target_os = "linux")]
fn open_fds(file: &std::path::Path) -> usize {
    let file = std::fs::canonicalize(file).unwrap();
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter(|t| *t == file)
        .count()
}

/// A take that times out leaves nothing of this process holding or waiting on the lock: no
/// extra thread (waiting on a lock in the kernel takes a thread blocked in `flock`), no open
/// file on it but the holder's, and the moment the holder lets go the lock is free to the very
/// next try — every time. (`/proc/locks` is not used: read while other processes take and drop
/// locks it can skip or repeat entries, so it is no ground truth under load.)
#[test]
fn a_take_that_times_out_leaves_nothing_waiting_or_holding() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let file = lock::sibling(&dir.path().join("board.db"));
    #[cfg(target_os = "linux")]
    let before = threads();
    for round in 0..20 {
        let holder = lock::take(&file, Mode::Exclusive, Duration::ZERO).expect("nobody holds it yet");
        match lock::take(&file, Mode::Exclusive, Duration::from_millis(30)) {
            Err(Error::Busy(_)) => {}
            other => panic!("round {round}: a held lock was not refused: {other:?}"),
        }
        #[cfg(target_os = "linux")]
        {
            // waiting on a lock in the kernel takes a thread blocked in `flock`: none may be left
            assert_eq!(threads(), before, "round {round}: the refused take left a thread behind");
            // and no open file on the lock but the holder's own
            assert_eq!(open_fds(&file), 1, "round {round}: the refused take left the lock file open");
        }
        drop(holder);
        // free to every try from here on: nothing left over can be granted it in between
        for i in 0..20 {
            let g = lock::take(&file, Mode::Exclusive, Duration::ZERO);
            assert!(g.is_ok(), "round {round} try {i}: the lock was held after its only holder let go");
            drop(g);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

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
