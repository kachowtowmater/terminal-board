//! Advisory whole-file locking (`flock`), shared by whatever in tb needs "one writer, or many
//! readers, at a time" on a file that is not itself a SQLite database — first the
//! machine-local settings (`crate::machine`), now a board file's own lifetime
//! (`crate::store::Store::open`, `crate::store::lock_for_move`).
//!
//! Two modes, the same distinction `flock(2)` makes: [`Mode::Shared`] — any number of holders
//! at once, for a process that only needs the file to keep existing where it is while it reads
//! or writes what is INSIDE it (a board's own concurrency past that point is SQLite's
//! business, not this module's) — and [`Mode::Exclusive`] — one holder, and it waits out
//! every shared holder first, for a process about to move, replace or otherwise change what
//! the path itself points at.
//!
//! **The lock is the KERNEL's**, held by an open file description, so it dies with the
//! process however the process ends — a crash, a `kill -9`, a `panic!` — leaving nothing to
//! reap. The `.lock` file's mere existence means nothing (see `a_leftover_lock_file_blocks_nobody`
//! in `tests/machine_race.rs`): only a live [`Guard`] blocks anyone. Waits are bounded; on
//! timeout the caller gets [`Error::Busy`], and on Linux — straight from the kernel's own
//! `/proc/locks`, no external tool — the pids still holding it, current at the moment asked
//! (never stale: a dead process cannot appear here, since this reads live kernel state, not a
//! saved list). Empty on other unix (no `/proc/locks`): a less specific message, never a
//! different lock decision — `holders` is asked only to compose [`Error::Busy`], after
//! [`take`] has already decided to give up.
//!
//! Where there is no `flock` (Windows) [`take`] always succeeds and [`Guard`] holds nothing:
//! two writers at the same instant are last-writer-wins, exactly as they were before this
//! module existed. A build for that target still needs a *correct*, if unenforced, story — see
//! `crate::store::Store::open`'s doc comment for what that means for a board file there.

use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Shared,
    Exclusive,
}

/// Why [`take`] gave up.
#[derive(Debug)]
pub enum Error {
    /// Waited out `wait` without getting the lock. The pids currently holding it, when they
    /// could be determined (Linux, via `/proc/locks`) — empty on other platforms, or when the
    /// holder let go between the last retry and asking.
    Busy(Vec<i32>),
    /// Could not even open or lock the `.lock` file itself (permissions, a bad filesystem…).
    Io(std::io::Error),
}

/// `target`'s lock file: a hidden sibling, `.` + its own file name + `.lock` — never the
/// board/settings file itself, so tb never has to teach SQLite (or JSON) about a file whose
/// bytes are meaningless and whose only job is to be `flock`ed. Works for any `target`,
/// existing or not: the lock file is what is coordinated, not the thing it stands for.
pub fn sibling(target: &Path) -> PathBuf {
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("tb.lock");
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    dir.join(format!(".{name}.lock"))
}

#[cfg(unix)]
#[derive(Debug)]
pub struct Guard {
    file: std::fs::File,
}

#[cfg(unix)]
impl Drop for Guard {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // closing the fd would release it anyway (the last close of the last fd on this open
        // file description does); this says so out loud, and matters if `Guard` ever grows a
        // second field that outlives this drop
        unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// The queue beside a lock file: `.board.db.lock` -> `.board.db.lock.q/`. Only a process that
/// has to WAIT for the lock touches it, so it exists only where there was contention; like the
/// lock file, nothing in it means anything unless a live process holds it.
fn queue_dir(lock_file: &Path) -> PathBuf {
    let name = lock_file.file_name().and_then(|n| n.to_str()).unwrap_or("tb.lock");
    let dir = lock_file.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    dir.join(format!("{name}.q"))
}

#[cfg(unix)]
fn open_lock_file(path: &Path, create: bool) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true).write(true).create(create).truncate(false);
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// One non-blocking try: `Ok(true)` got it, `Ok(false)` somebody else holds it.
#[cfg(unix)]
fn try_flock(file: &std::fs::File, flag: libc::c_int) -> Result<bool, Error> {
    use std::os::unix::io::AsRawFd;
    if unsafe { libc::flock(file.as_raw_fd(), flag | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let e = std::io::Error::last_os_error();
    if e.kind() == std::io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(Error::Io(e))
    }
}

/// The live tickets in `queue` that come before `before` (all of them for `None`), oldest
/// first. A ticket is a file named by its number, held EXCLUSIVE by its waiter for as long as
/// it waits: one that can be locked here belongs to a waiter that is gone — it gave up and
/// crashed before removing it, or was killed — and is removed on the way, so a dead waiter
/// never holds up the queue.
#[cfg(unix)]
fn live_tickets(queue: &Path, before: Option<u64>) -> Vec<u64> {
    let Ok(rd) = std::fs::read_dir(queue) else { return Vec::new() };
    let mut nums: Vec<u64> = rd
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u64>().ok())
        .filter(|n| before.is_none_or(|b| *n < b))
        .collect();
    nums.sort_unstable();
    nums.retain(|n| {
        let path = queue.join(n.to_string());
        let Ok(f) = open_lock_file(&path, false) else { return false };
        match try_flock(&f, libc::LOCK_EX) {
            // nobody holds it: a waiter that is gone. Remove it while holding it, so a waiter
            // that is merely slow to look can never lose its place this way.
            Ok(true) => {
                let _ = std::fs::remove_file(&path);
                false
            }
            Ok(false) => true,
            Err(_) => false,
        }
    });
    nums
}

/// A waiter's place in the queue: its ticket file, held EXCLUSIVE until it gets the lock or
/// gives up — either way the ticket is removed, and should the process die first, the kernel
/// lets go of it and the next look (`live_tickets`) removes it.
#[cfg(unix)]
struct Ticket {
    path: PathBuf,
    number: u64,
    _file: std::fs::File,
}

#[cfg(unix)]
impl Drop for Ticket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Join the queue: the next number, taken under the queue's own short lock (`next`, which
/// also holds the counter). The ticket is created and locked under a private name first and
/// only then renamed to its number, so no one ever sees a numbered ticket that is not held.
/// `None` when the queue cannot be used (a read-only directory): that only costs the order.
#[cfg(unix)]
fn join_queue(queue: &Path) -> Option<Ticket> {
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::unix::io::AsRawFd;
    std::fs::create_dir_all(queue).ok()?;
    let private = queue.join(format!(".{}-{:?}", std::process::id(), std::thread::current().id()));
    let file = open_lock_file(&private, true).ok()?;
    if !matches!(try_flock(&file, libc::LOCK_EX), Ok(true)) {
        let _ = std::fs::remove_file(&private);
        return None;
    }
    let mut counter = open_lock_file(&queue.join("next"), true).ok()?;
    // held for a read, an increment and a rename: microseconds, so a blocking lock is fine
    // here, and it is never held while waiting for the real lock
    if unsafe { libc::flock(counter.as_raw_fd(), libc::LOCK_EX) } != 0 {
        let _ = std::fs::remove_file(&private);
        return None;
    }
    let mut text = String::new();
    let _ = counter.read_to_string(&mut text);
    let number: u64 = text.trim().parse().unwrap_or(0);
    let placed = counter
        .seek(SeekFrom::Start(0))
        .and_then(|_| counter.set_len(0))
        .and_then(|_| counter.write_all((number + 1).to_string().as_bytes()))
        .and_then(|_| std::fs::rename(&private, queue.join(number.to_string())));
    unsafe { libc::flock(counter.as_raw_fd(), libc::LOCK_UN) };
    if placed.is_err() {
        let _ = std::fs::remove_file(&private);
        return None;
    }
    Some(Ticket { path: queue.join(number.to_string()), number, _file: file })
}

/// Take the lock on `path`, waiting at most `wait`.
///
/// Every try is non-blocking, on the calling thread: nothing is ever left blocked in the
/// kernel, so a take that gives up leaves nothing behind — no thread, no open file, no queued
/// request that could be granted the lock later.
///
/// **First come, first served.** A process that has to wait takes a numbered ticket
/// (`join_queue`) and tries the lock only once no live ticket is ahead of it; a newcomer that
/// finds anyone queued takes a ticket too instead of grabbing the lock the moment it is free.
/// So a process that lets go and asks again at once (a loop of writes) goes to the back of the
/// queue, and no waiter can lose the race for the lock again and again. A waiter that gives up
/// removes its ticket; one that dies loses its hold on it with the process, and the next waiter
/// to look skips and removes it. The uncontended take is one try, as it always was: no queue,
/// no file created.
#[cfg(unix)]
pub fn take(path: &Path, mode: Mode, wait: Duration) -> Result<Guard, Error> {
    use std::time::Instant;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(dir);
        }
    }
    let file = open_lock_file(path, true).map_err(Error::Io)?;
    let flag = match mode {
        Mode::Shared => libc::LOCK_SH,
        Mode::Exclusive => libc::LOCK_EX,
    };
    let queue = queue_dir(path);
    // the ordinary case: nobody queued, one try
    if live_tickets(&queue, None).is_empty() && try_flock(&file, flag)? {
        return Ok(Guard { file });
    }
    if wait.is_zero() {
        return Err(Error::Busy(holders(path)));
    }
    let ticket = join_queue(&queue);
    let start = Instant::now();
    let mut n: u64 = 0;
    loop {
        // my turn once nobody still waiting came before me (without a ticket: every poll)
        let my_turn = ticket.as_ref().is_none_or(|t| live_tickets(&queue, Some(t.number)).is_empty());
        if my_turn && try_flock(&file, flag)? {
            // leaving the queue: `ticket` is dropped here, which removes it
            return Ok(Guard { file });
        }
        if start.elapsed() >= wait {
            return Err(Error::Busy(holders(path)));
        }
        n += 1;
        std::thread::sleep(Duration::from_millis(1 + (std::process::id() as u64 + n * 7) % 4));
    }
}

#[cfg(not(unix))]
#[derive(Debug)]
pub struct Guard;

#[cfg(not(unix))]
pub fn take(_path: &Path, _mode: Mode, _wait: Duration) -> Result<Guard, Error> {
    Ok(Guard)
}

/// Best-effort: the pids holding an `flock` on `path` right now, straight from the kernel's
/// own `/proc/locks` — never a saved list, so a process that has since died or let go simply
/// is not in it; nothing here can go stale. Matched by INODE alone, not the full
/// `major:minor:inode` triple `/proc/locks` prints (the device-number encoding differs from
/// `stat`'s and is not worth reproducing for a `.lock` file in tb's own state directory, where
/// an inode collision with an unrelated device is not a realistic event) — silently empty
/// when that is wrong, or on a kernel/container without `/proc/locks`, which only makes the
/// message less specific, never the lock decision, which never reads this.
#[cfg(target_os = "linux")]
fn holders(path: &Path) -> Vec<i32> {
    use std::os::unix::fs::MetadataExt;
    let Ok(ino) = std::fs::metadata(path).map(|m| m.ino()) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string("/proc/locks") else { return Vec::new() };
    let mut pids: Vec<i32> = text
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            // "id: FLOCK ADVISORY WRITE|READ pid major:minor:inode start end" — a line for a
            // lock someone is still WAITING on (never a holder) instead reads "id: -> FLOCK
            // …", so `f[1] == "FLOCK"` already excludes it without any special case.
            if f.len() < 6 || f[1] != "FLOCK" {
                return None;
            }
            let this_ino: u64 = f[5].rsplit(':').next()?.parse::<u64>().ok()?;
            if this_ino != ino {
                return None;
            }
            f[4].parse::<i32>().ok()
        })
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(all(unix, not(target_os = "linux")))]
fn holders(_path: &Path) -> Vec<i32> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_names() {
        assert_eq!(sibling(Path::new("boards/scratch.db")), PathBuf::from("boards/.scratch.db.lock"));
        assert_eq!(sibling(Path::new("scratch.db")), PathBuf::from("./.scratch.db.lock"));
    }

    #[cfg(unix)]
    #[test]
    fn shared_holders_do_not_block_each_other() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("board.db");
        let a = take(&p, Mode::Shared, Duration::from_millis(200)).unwrap();
        let b = take(&p, Mode::Shared, Duration::from_millis(200)).unwrap();
        drop(a);
        drop(b);
    }

    #[cfg(unix)]
    #[test]
    fn exclusive_waits_for_shared_then_succeeds() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("board.db");
        let reader = take(&p, Mode::Shared, Duration::from_millis(200)).unwrap();
        let start = std::time::Instant::now();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done2 = done.clone();
        let p2 = p.clone();
        let mover = std::thread::spawn(move || {
            let g = take(&p2, Mode::Exclusive, Duration::from_secs(2)).unwrap();
            done2.store(true, std::sync::atomic::Ordering::SeqCst);
            drop(g);
        });
        std::thread::sleep(Duration::from_millis(60));
        assert!(!done.load(std::sync::atomic::Ordering::SeqCst), "exclusive proceeded while a shared holder was still open");
        drop(reader);
        mover.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(2), "exclusive waited the whole timeout instead of unblocking on drop");
    }

    #[cfg(unix)]
    #[test]
    fn exclusive_refuses_after_the_wait_and_shared_can_still_wait_out_an_exclusive_holder() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("board.db");
        let holder = take(&p, Mode::Exclusive, Duration::from_millis(200)).unwrap();
        let e = take(&p, Mode::Exclusive, Duration::from_millis(80)).unwrap_err();
        assert!(matches!(e, Error::Busy(_)), "{e:?}");
        // a new reader (Store::open's shape) queues behind the mover instead of racing it
        let p2 = p.clone();
        let waiter = std::thread::spawn(move || take(&p2, Mode::Shared, Duration::from_secs(2)));
        std::thread::sleep(Duration::from_millis(60));
        drop(holder);
        assert!(waiter.join().unwrap().is_ok(), "a shared waiter never got the lock after the exclusive holder let go");
    }
}
