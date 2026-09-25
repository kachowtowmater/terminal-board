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

/// One ancestor hop on macOS: `proc_pidinfo` (`PROC_PIDTBSDINFO`) gives both the ppid and
/// the binary's short name (`pbi_comm`, what `ps -o comm` shows without the path; `pbi_name`
/// when `comm` is blank). The process may be gone by the time we read it: `None`, and the
/// chain ends where it ended.
#[cfg(target_os = "macos")]
fn step(pid: u32) -> Option<(u32, String)> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a writable, correctly sized `proc_bsdinfo`; the kernel fills at most
    // `size` bytes and returns how many it wrote.
    let got = unsafe {
        libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size)
    };
    if got != size {
        return None;
    }
    let name = |raw: &[libc::c_char]| -> String {
        let bytes: Vec<u8> = raw.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
        String::from_utf8_lossy(&bytes).trim().to_string()
    };
    let comm = Some(name(&info.pbi_comm)).filter(|n| !n.is_empty()).unwrap_or_else(|| name(&info.pbi_name));
    Some((info.pbi_ppid, if comm.is_empty() { "?".to_string() } else { comm }))
}

/// Other unix systems (the BSDs, …): no walk yet — an empty ancestry, recorded as such.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn step(_pid: u32) -> Option<(u32, String)> {
    None
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
