//! How `lock::take` waits (src/lock.rs): every try is non-blocking on the calling thread, so a
//! take that gives up leaves nothing behind, and a waiter is not starved by a process that lets
//! go and takes the lock again at once.
#![cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use terminal_board::lock::{self, Error, Mode};

/// The two tests count this process's threads and locks: never at the same time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

#[cfg(target_os = "linux")]
fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}

/// `/proc/locks` lines for `file`'s inode that belong to this process: (held, waiting).
#[cfg(target_os = "linux")]
fn my_locks(file: &std::path::Path) -> (usize, usize) {
    use std::os::unix::fs::MetadataExt;
    let ino = std::fs::metadata(file).unwrap().ino();
    let me = std::process::id().to_string();
    let (mut held, mut waiting) = (0, 0);
    for l in std::fs::read_to_string("/proc/locks").unwrap().lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        // "1: FLOCK ADVISORY WRITE pid maj:min:ino ..." or, for a waiter, "1: -> FLOCK ..."
        let (rest, is_wait) = if f.get(1) == Some(&"->") { (&f[2..], true) } else { (&f[1..], false) };
        if rest.len() < 5 || rest[0] != "FLOCK" || rest[3] != me {
            continue;
        }
        if rest[4].rsplit(':').next().and_then(|i| i.parse::<u64>().ok()) != Some(ino) {
            continue;
        }
        if is_wait {
            waiting += 1;
        } else {
            held += 1;
        }
    }
    (held, waiting)
}

/// A take that times out leaves nothing of this process holding or waiting on the lock: no
/// request still queued in the kernel, no extra thread, so the moment the holder lets go the
/// lock is free to the very next try — every time.
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
            assert_eq!(my_locks(&file), (1, 0), "round {round}: after the refused take: (held, waiting)");
            assert_eq!(threads(), before, "round {round}: the refused take left a thread behind");
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
