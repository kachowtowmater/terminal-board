//! Shared test fixture: a small, neutral board (10 cards across the columns).
#![allow(dead_code)]

use std::path::PathBuf;
use terminal_board::store::{Result, Seed, Store};

const M: i64 = 60;
const H: i64 = 3600;
const D: i64 = 86400;

pub fn seed(store: &Store, you: &str) -> Result<Vec<i64>> {
    let tomorrow = (chrono::Local::now() + chrono::Duration::days(1)).format("%b %-d").to_string();
    let seeds = [
        Seed {
            title: "widgets: gh#308 csv export",
            desc: "CSV export drops the header row. Done = header present, test added.",
            column: "todo",
            owner: None,
            age_secs: 3 * D,
            due: None,
            checks: &[],
            notes: &[],
        },
        Seed {
            title: "widgets: gh#309 retry backoff",
            desc: "Retry failed uploads with exponential backoff.",
            column: "todo",
            owner: None,
            age_secs: 2 * D,
            due: None,
            checks: &[("pick the limits", false), ("wire into uploader", false)],
            notes: &[],
        },
        Seed {
            title: "ops: rotate API tokens",
            desc: "Rotate the deploy and CI tokens before they expire.",
            column: "todo",
            owner: None,
            age_secs: D + 2 * H,
            due: Some(tomorrow.as_str()),
            checks: &[],
            notes: &[],
        },
        Seed {
            title: "admin: renew domain",
            desc: "Renew before the end of the month; check auto-renew.",
            column: "todo",
            owner: None,
            age_secs: D,
            due: None,
            checks: &[],
            notes: &[],
        },
        Seed {
            title: "widgets: gh#327 login form rejects valid emails",
            desc: "Emails with a plus sign are rejected. Done = plus-addresses accepted,\ntest added, PR green.",
            column: "doing",
            owner: Some("bot-2"),
            age_secs: 40 * M,
            due: None,
            checks: &[
                ("reproduce with a+b@example.com", true),
                ("patch validator", true),
                ("add test", false),
                ("open PR", false),
            ],
            notes: &[("bot-2", "repro confirmed"), ("bot-2", "patch applied, tests running")],
        },
        Seed {
            title: "admin: vendor quote",
            desc: "Get the final quote from the hosting vendor.",
            column: "doing",
            owner: Some(you),
            age_secs: 2 * D,
            due: None,
            checks: &[],
            notes: &[(you, "left a voicemail")],
        },
        Seed {
            title: "widgets: gh#314 search index lags",
            desc: "New items take minutes to show in search. PR #334 is up; review re-running.",
            column: "review",
            owner: Some("rev"),
            age_secs: 12 * M,
            due: None,
            checks: &[("fix", true), ("test", true), ("CI green", false)],
            notes: &[("rev", "CI running")],
        },
        Seed {
            title: "widgets: gh#325 typo in footer",
            desc: "",
            column: "done",
            owner: Some("bot"),
            age_secs: 3 * H,
            due: None,
            checks: &[],
            notes: &[("bot", "PR #333 merged")],
        },
        Seed {
            title: "widgets: gh#328 dark theme",
            desc: "",
            column: "done",
            owner: Some("bot-3"),
            age_secs: 5 * H,
            due: None,
            checks: &[],
            notes: &[("bot-3", "PR #332 merged")],
        },
        Seed {
            title: "fix README images",
            desc: "",
            column: "done",
            owner: Some(you),
            age_secs: 6 * H,
            due: None,
            checks: &[],
            notes: &[],
        },
    ];
    let ids = seeds.iter().map(|s| store.seed(s)).collect::<Result<Vec<_>>>()?;
    store.order_by_time()?;
    // retry backoff waits on the search fix in review
    store.block(ids[1], Some(&format!("#{}", ids[6])), "seed")?;
    Ok(ids)
}

/// Pin the test clock: `TB_NOW` = local noon today, set once per process. Every fixture
/// derived from `store::now()` is then stable whatever the time of day: just after local
/// midnight, `now - 3h` fixtures used to fall on yesterday and flip the `+N today` counts.
/// Call it first in every test of a binary that uses it, so no test sees the clock move.
pub fn pin_clock() {
    use std::sync::Once;
    static PIN: Once = Once::new();
    PIN.call_once(|| {
        let noon = chrono::Local::now()
            .date_naive()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .map(|t| t.timestamp())
            .unwrap_or_else(terminal_board::store::now);
        std::env::set_var("TB_NOW", noon.to_string());
    });
}

/// The verifier registry (card #169): one JSON file per session under
/// `~/.local/state/terminal-board/verifiers/`, written by `tb-agent-start --role verifier`.
///
/// A CLI test that models a verifier writes its OWN registry dir and points the child's
/// `TB_VERIFIERS_DIR` at it (the env the store reads; never set it in production), so the
/// suite never writes the real machine's registry. `dir` must be a fresh, empty temp dir the
/// test owns — `register` records one session exactly as the launcher does.
pub struct VerifierRegistry {
    pub dir: tempfile::TempDir,
}

impl VerifierRegistry {
    pub fn new() -> VerifierRegistry {
        VerifierRegistry { dir: tempfile::tempdir().unwrap() }
    }

    /// Register `session` for `name` on `harness`, the same file shape
    /// `tb-agent-start` writes (`{"session","name","harness"}`).
    pub fn register(&self, session: &str, name: &str, harness: &str) {
        std::fs::create_dir_all(self.dir.path()).unwrap();
        let entry = serde_json::json!({ "session": session, "name": name, "harness": harness });
        std::fs::write(self.dir.path().join(session), serde_json::to_string(&entry).unwrap()).unwrap();
    }

    /// The env pair a child `tb` needs to see this registry (and nothing else's).
    pub fn env(&self) -> [(&'static str, String); 1] {
        [("TB_VERIFIERS_DIR", self.dir.path().to_string_lossy().into_owned())]
    }
}

/// An executable whose argv0 basename is `name` (usually `omp`): a system shell
/// (`/bin/sh`) copied ONCE per test process — the root fix tb#247 moved into
/// release_real_ps and tb#286 hoisted here: parallel test threads fork while `fs::copy`
/// still holds the destination open for writing, a forked child inherits that write fd
/// and the exec fails ETXTBSY (~1 in 40 runs on Linux, tb#247; tb_reap.rs:376 CI hit,
/// run 37287163080). The copy lands by write-temp → close → chmod 0755 → rename, so the
/// exec'd path never has an open writer; the returned path lives in a leaked tempdir so
/// it outlives every test thread. macOS refuses to exec a signed system binary moved to
/// a new path (the Launch Constraint no longer matches, SIGKILL), so the copy is
/// re-signed ad hoc — the same exec gate verifier_rule's omp copy clears; Linux needs
/// neither step.
pub fn agent_shell(name: &str) -> PathBuf {
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    static SHELL: std::sync::LazyLock<std::sync::Mutex<HashMap<String, PathBuf>>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));
    let mut shells = SHELL.lock().unwrap();
    if let Some(p) = shells.get(name) {
        return p.clone();
    }
    let dir = tempfile::tempdir().unwrap(); // leaked below: must outlive every thread
    let tmp = dir.path().join(format!("{name}.tmp"));
    std::fs::copy("/bin/sh", &tmp).unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    let shell = dir.path().join(name);
    std::fs::rename(&tmp, &shell).unwrap();
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&shell).output();
    }
    dir.keep(); // leaks the dir on purpose: outlives every thread
    shells.insert(name.to_string(), shell.clone());
    shell
}

/// Retry the spawn on ETXTBSY (`Text file busy`, os error 26): between `agent_shell`'s
/// rename and the exec, another thread's forked child can still hold a write-mode fd
/// inherited before the copy closed — the busy window is that forked child's lifetime.
/// Backs off through `terminal_board::waits::pause` (clippy.toml bans raw sleeps), ~1.4s
/// total; the last call re-runs the command and fails with the same error if still busy.
pub fn retry_exec_busy<T>(
    why: &str,
    mut run: impl FnMut() -> Result<T, std::io::Error>,
) -> Result<T, std::io::Error> {
    let mut last = None;
    for (i, wait) in [0u64, 10, 25, 50, 100, 200, 200, 200, 200, 200].into_iter().enumerate() {
        if wait > 0 {
            terminal_board::waits::pause(why, std::time::Duration::from_millis(wait));
        }
        match run() {
            Err(e) if e.raw_os_error() == Some(26) || e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                eprintln!("agent_shell: exec busy (ETXTBSY), retry {}/10", i + 1);
                last = Some(e);
            }
            other => return other,
        }
    }
    Err(last.unwrap())
}
