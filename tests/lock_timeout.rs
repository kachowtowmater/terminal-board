//! A take that gives up leaves nothing behind (src/lock.rs): no thread, no open file on the
//! lock. Alone in its own test binary, because it counts this process's threads — any other
//! test in the same binary adds or removes harness threads while it counts.
#![cfg(unix)]
use std::time::Duration;
use terminal_board::lock::{self, Error, Mode};

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
