//! The kernel's own record of who is running this command: the process ancestry.
//!
//! Environment (`TB_*`) is a claim the caller writes; the parent chain is the kernel's, and a
//! script file or a renamed argv cannot rewrite it. Every write of consequence — a move into
//! DONE, `--force`, a change to who verifies — records this chain alongside the self-reported
//! identity (`store::actors`), so the trace answers "what actually ran this command" too.
//!
//! The chain is the parent chain walked upward, at most [`MAX_DEPTH`] hops, each mapped to the
//! binary's file name (`comm`): `bash`, `claude`, `herdr`, `sshd`, `tb` …, oldest ancestor
//! first. Stopping points:
//!
//! - **pid 0 / pid 1** (the kernel, `launchd`/`init`) — read, but not followed.
//! - A pid that would loop (`ppid <= pid` after the kernel pair) or is already in the chain.
//! - A process gone by the time we read it, or a table that cannot be read at all: the walk
//!   keeps what it has and records it — **an ancestry never fails the command**. It is
//!   evidence, not a gate that must run.
//!
//! Not proof against a determined same-uid attacker (docs/AGENTS.md, "the trace is evidence,
//! not a wall"): a name can be copied, and the env/registry forges are separate. What the
//! walk catches is the *kernel-recorded* ancestor — e.g. the gate's `/tmp/omp` trick, a bash
//! binary copied over an agent's name, which `TB_HARNESS` cannot fake and argv tricks cannot
//! hide. A command under a *different real binary* changes the kernel's record.
pub const MAX_DEPTH: usize = 32;

/// This process's ancestry, oldest ancestor first, without itself. Empty when the table
/// cannot be read at all.
pub fn ancestry() -> Vec<String> {
    walk(std::process::id())
}

/// The same walk from an explicit pid (tests). `start` itself is NOT included.
pub fn walk(start: u32) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: Vec<u32> = Vec::new();
    let Some((mut ppid, _)) = step(start) else { return out };
    for _ in 0..MAX_DEPTH {
        let Some((next, comm)) = step(ppid) else { break };
        out.push(comm);
        seen.push(ppid);
        // pid 0 and 1 end the chain: 0 is the kernel, 1 launchd/init — every process's final
        // ancestors, carrying no signal.
        if next <= 1 {
            break;
        }
        if seen.contains(&next) || seen.len() >= MAX_DEPTH {
            break;
        }
        ppid = next;
    }
    out.reverse(); // collected nearest-first; recorded oldest ancestor first
    out
}

/// One ancestor hop. `None` when the kernel has no row for `pid` (it is gone, or the table
/// is unreadable).
#[cfg(target_os = "linux")]
fn step(pid: u32) -> Option<(u32, String)> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_string();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `comm` may itself hold spaces and parentheses, so ppid sits AFTER the LAST `)`.
    let after = stat.rsplit_once(')')?.1;
    let ppid: u32 = after.split_whitespace().nth(1)?.parse().ok()?;
    Some((ppid, if comm.is_empty() { "?".to_string() } else { comm }))
}

/// One ancestor hop on macOS: `proc_pidinfo` (`PROC_PIDTBSDINFO`) for the ppid and
/// `sysctl` `KERN_PROC_PID` for `comm` — what `ps -o ppid,comm` reads. The process may be
/// gone by the time we read it: `None`, and the chain ends where it ended.
#[cfg(all(unix, not(target_os = "linux")))]
fn step(pid: u32) -> Option<(u32, String)> {
    use libc::*;
    unsafe {
        let mut info: proc_bsdinfo = std::mem::zeroed();
        if proc_pidinfo(
            pid as c_int,
            PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<proc_bsdinfo>() as c_int,
        ) != std::mem::size_of::<proc_bsdinfo>() as c_int
        {
            return None;
        }
        let ppid = info.p_ppid as u32;
        let mut kinfo: kinfo_proc = std::mem::zeroed();
        let mut len = std::mem::size_of::<kinfo_proc>();
        let mut mib = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid as c_int];
        if sysctl(&mut mib, 4, &mut kinfo as *mut _ as *mut c_void, &mut len, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
        let comm = std::ffi::CStr::from_bytes_until_nul(&kinfo.kp_proc.p_comm).to_string_lossy().trim().to_string();
        Some((ppid, if comm.is_empty() { "?".to_string() } else { comm }))
    }
}

#[cfg(not(unix))]
fn step(_pid: u32) -> Option<(u32, String)> {
    None // not unix: no ancestry at all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_has_a_parent_and_stays_within_the_cap() {
        let a = walk(std::process::id());
        assert!(!a.is_empty(), "the test runner itself has a parent");
        assert!(a.iter().all(|n| !n.trim().is_empty()), "no blank names: {a:?}");
        assert!(a.len() <= MAX_DEPTH, "the walk is capped: {a:?}");
    }

    #[test]
    fn the_oldest_entry_is_a_real_ancestor_not_the_process_itself() {
        let a = walk(std::process::id());
        assert_ne!(a.first().map(String::as_str), Some("tb"), "the walk never starts at itself: {a:?}");
    }
}
