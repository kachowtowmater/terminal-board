//! Every wait the store's write path makes on purpose — the store and the board lock it
//! opens with (`crate::lock`) — in one place.
//!
//! An uncontended `tb add` must never wait: no lock to queue for, no busy retry, no backoff
//! sleep. That is a property of what the code DOES, not of how fast a machine is, so it is
//! tested by counting waits rather than timing adds (a timed bound failed on a loaded disk
//! with nothing wrong). `TB_TRACE_WAITS=FILE` — a test hook — makes tb append one line per
//! wait to FILE: what it waited for. Unset, nothing is written and nothing changes.
use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// Sleep for `d` because of `why` (a retry, a backoff), noting it in the trace.
pub fn pause(why: &str, d: Duration) {
    note(why);
    std::thread::sleep(d);
}

/// Record a wait in the trace, when one is asked for.
pub fn note(why: &str) {
    if let Some(p) = crate::env("TRACE_WAITS") {
        note_to(Path::new(&p), why);
    }
}

fn note_to(path: &Path, why: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {why}", std::process::id());
    }
}

/// Make `conn` wait up to `d` for another writer, as `busy_timeout` does. Traced, the wait
/// goes through a busy handler that notes every retry (SQLite's own timeout cannot be seen
/// from outside), sleeping with the same back-off SQLite uses and giving up after `d`.
pub fn busy(conn: &rusqlite::Connection, d: Duration) -> rusqlite::Result<()> {
    if crate::env("TRACE_WAITS").is_none() {
        return conn.busy_timeout(d);
    }
    // a busy handler is a plain `fn`: the limit it needs is the store's fixed 10s one
    debug_assert_eq!(d, Duration::from_secs(10));
    conn.busy_handler(Some(traced_busy))
}

/// SQLite's busy back-off (ms per retry, then 100ms each), capped at 10s in all.
fn traced_busy(retry: i32) -> bool {
    const STEPS: [u64; 12] = [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100];
    let n = retry.max(0) as usize;
    let spent: u64 = (0..n).map(|i| STEPS.get(i).copied().unwrap_or(100)).sum();
    if spent >= 10_000 {
        return false;
    }
    pause("sqlite busy", Duration::from_millis(STEPS.get(n).copied().unwrap_or(100)));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_noted_wait_lands_in_the_trace_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("waits");
        note_to(&p, "lock");
        note_to(&p, "sqlite busy");
        let text = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].ends_with(" lock") && lines[1].ends_with(" sqlite busy"), "{text}");
    }

    #[test]
    fn the_traced_busy_handler_gives_up_after_ten_seconds_of_back_off() {
        // retries 0..=11 follow SQLite's steps (1+2+5+…+100 = 328ms), then 100ms each
        let spent_before = |n: usize| -> u64 { (0..n).map(|i| [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100].get(i).copied().unwrap_or(100)).sum() };
        let last_retry = (0..).find(|n| spent_before(*n) >= 10_000).unwrap();
        assert!(spent_before(last_retry - 1) < 10_000);
        assert!(!traced_busy(last_retry as i32), "gives up once 10s are spent");
    }
}
