#![cfg(unix)]
//! `tb rm` on a card that is not DONE (card #225): a person's call, or a REGISTERED
//! verifier's — never a builder's, a lead's/orchestrator's, or a role-forging shell's, with
//! or without `--force`. Everything here drives the real `tb` binary with a controlled
//! environment (whatever harness runs the suite must not leak in) and only through the CLI,
//! so a tree without the rule fails by assertion, not by compile error.
//!
//! Keep-cases already pinned elsewhere are repeated here END-TO-END so the rule is proven
//! as a whole on one board: a person rm's anything, a registered verifier rm's open work, an
//! agent rm's a DONE card — and a refused rm is visible in `tb log` afterwards (a refused
//! transaction rolls back, so the refusal's log line is written separately).
mod common;

use std::path::PathBuf;
use std::process::{Command, Output};

const UUID: &str = "0b9f6a52-7c1d-4e0a-9f3b-2a6c1d8e4f70";
/// The registry's dir env: the sessions this board treats as registered verifiers.
const VERIFIER: &[(&str, &str)] = &[
    ("CLAUDECODE", "1"),
    ("CLAUDE_CODE_SESSION_ID", UUID),
    ("TB_ROLE", "verifier"),
    ("TB_MODEL", "model-x"),
    ("TB_HOST", "lab"),
];
/// A builder: a harness on record, no role.
const AGENT: &[(&str, &str)] = &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", UUID)];
/// A lead/orchestrator: a role, but not a verifier's.
const LEAD: &[(&str, &str)] = &[
    ("CLAUDECODE", "1"),
    ("CLAUDE_CODE_SESSION_ID", UUID),
    ("TB_ROLE", "orchestrator"),
];
/// A person: a plain terminal, nothing but the name.
const PERSON: &[(&str, &str)] = &[];

struct Board {
    dir: tempfile::TempDir,
    registry: common::VerifierRegistry,
}

impl Board {
    fn new() -> Board {
        let b = Board {
            dir: tempfile::tempdir().unwrap(),
            registry: common::VerifierRegistry::new(),
        };
        b.ok(PERSON, "lead", &["config", "wip", "9"]);
        b
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("b.db")
    }

    fn run_raw(&self, env: &[(&str, &str)], who: &str, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).env_clear();
        c.env("TB_DB", self.db())
            .env("TB_AS", who)
            .env("TB_VERIFIERS_DIR", self.registry.dir.path())
            .env("TB_NO_HERDR", "1")
            .env("TB_GH", "/nonexistent/gh")
            .env("USER", "login-user")
            .env("TZ", "UTC")
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.dir.path());
        c.envs(env.iter().copied());
        c.output().unwrap()
    }

    fn ok(&self, env: &[(&str, &str)], who: &str, args: &[&str]) -> String {
        let o = self.run_raw(env, who, args);
        assert!(
            o.status.success(),
            "{who}: tb {args:?} failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap()
    }

    /// The `--json` refusal: (error text, code). Asserts it WAS refused.
    fn refused(&self, env: &[(&str, &str)], who: &str, args: &[&str]) -> (String, String) {
        let mut a = args.to_vec();
        a.push("--json");
        let o = self.run_raw(env, who, &a);
        let v: serde_json::Value =
            serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
        assert!(!o.status.success(), "{who}: tb {args:?} was allowed: {v}");
        let text = format!(
            "{} — {}",
            v["error"].as_str().unwrap_or(""),
            v["hint"].as_str().unwrap_or("")
        );
        (text, v["code"].as_str().unwrap_or("").to_string())
    }

    /// The id of the one card whose title matches, or the id itself when given.
    fn add(&self, title: &str) -> String {
        let v: serde_json::Value =
            serde_json::from_str(&self.ok(PERSON, "lead", &["add", title, "--json"])).unwrap();
        v["card"]["id"].as_i64().unwrap().to_string()
    }

    fn column(&self, id: &str) -> String {
        let v: serde_json::Value =
            serde_json::from_str(&self.ok(PERSON, "lead", &["show", id, "--json"])).unwrap();
        v["card"]["column"]
            .as_str()
            .or(v["column"].as_str())
            .unwrap()
            .to_string()
    }

    fn on_board(&self, id: &str) -> bool {
        let o = self.run_raw(PERSON, "lead", &["show", id, "--json"]);
        o.status.success()
    }

    /// The board's log as one string (plain and JSON, both: a consumer may read either).
    fn log(&self) -> String {
        format!(
            "{} {}",
            self.ok(PERSON, "lead", &["log"]),
            self.ok(PERSON, "lead", &["log", "--json"])
        )
    }
}

/// C1: a builder agent's rm on open work is refused in every column — TODO, REVIEW, DOING,
/// its own DOING card — with or without `--force`, with a hint that names the way out.
#[test]
fn a_builder_cannot_rm_open_work() {
    let b = Board::new();
    let t = b.add("t: build it");
    let d = b.add("d: mine in doing");
    b.ok(PERSON, "lead", &["take", &d]);
    b.ok(PERSON, "lead", &["done", &d]);
    let d = b.add("d2: agent doing");
    b.ok(AGENT, "b-x", &["take", &d]);
    let r = b.add("r2: for review");
    b.ok(AGENT, "b-x", &["take", &r]);
    b.ok(AGENT, "b-x", &["done", &r]);
    for args in [vec!["rm", t.as_str()], vec!["rm", t.as_str(), "--force"]] {
        let (e, code) = b.refused(AGENT, "b-x", &args);
        assert_eq!(code, "rm_verifier_only", "{args:?}: {e}");
        assert!(e.contains("review"), "the refusal names the way out: {e}");
        assert!(e.contains("CLOSE"), "the refusal names the CLOSE note: {e}");
        assert!(b.on_board(&t), "the card survived {args:?}");
    }
    let (e, code) = b.refused(AGENT, "b-x", &["rm", &r, "--force"]);
    assert_eq!(code, "rm_verifier_only", "{e}");
    let (e, code) = b.refused(AGENT, "b-x", &["rm", &d, "--force"]);
    assert_eq!(code, "rm_verifier_only", "{e}");
}

/// C2: a lead/orchestrator is an agent, not a verifier — same refusal.
#[test]
fn a_lead_cannot_rm_open_work() {
    let b = Board::new();
    let t = b.add("t: filed for me");
    let (e, code) = b.refused(LEAD, "lead-x", &["rm", &t, "--force"]);
    assert_eq!(code, "rm_verifier_only", "{e}");
    assert!(b.on_board(&t));
}

/// C3: a role alone is not a grant (#169): a `TB_ROLE=verifier` with no registry entry is
/// refused, because `tb-agent-start` did not start this session as a verifier.
#[test]
fn a_forged_verifier_cannot_rm_open_work() {
    let b = Board::new();
    let t = b.add("t: the forge");
    let (e, code) = b.refused(VERIFIER, "rv-x", &["rm", &t]);
    assert_eq!(code, "rm_verifier_only", "{e}");
    assert!(b.on_board(&t));
}

/// C4: a REGISTERED verifier keeps rm — the same session in the registry, the same name.
#[test]
fn a_registered_verifier_keeps_rm() {
    let b = Board::new();
    b.registry.register(UUID, "rv-x", "claude-code");
    let t = b.add("t: registered cleanup");
    b.ok(VERIFIER, "rv-x", &["rm", &t]);
    assert!(!b.on_board(&t));
}

/// C5: a person keeps rm everywhere — the plain-terminal identity, no harness.
#[test]
fn a_person_keeps_rm() {
    let b = Board::new();
    let t = b.add("t: person cleanup");
    b.ok(PERSON, "lead", &["rm", &t, "--force"]);
    assert!(!b.on_board(&t));
}

/// C6: an agent may still rm a DONE card — the trace of closed work stays in the ledger.
/// Getting a card TO done needs a verifier: a person takes it, a registered verifier
/// (`rv-x`, in the registry) closes it.
#[test]
fn an_agent_may_still_rm_a_done_card() {
    let b = Board::new();
    b.registry.register(UUID, "rv-x", "claude-code");
    let d = b.add("d: finished");
    b.ok(PERSON, "lead", &["take", &d]);
    b.ok(PERSON, "lead", &["done", &d]);
    b.ok(VERIFIER, "rv-x", &["done", &d]);
    assert_eq!(b.column(&d), "done");
    b.ok(AGENT, "b-x", &["rm", &d]);
    assert!(!b.on_board(&d));
}

/// C7: a refused rm is visible afterwards — `tb log` (plain and `--json`) carries the
/// refusal, though the refused rm wrote nothing else (the transaction rolled back).
#[test]
fn a_refused_rm_is_logged() {
    let b = Board::new();
    let t = b.add("t: logged refusal");
    b.refused(AGENT, "b-x", &["rm", &t, "--force"]);
    let after = b.log();
    assert!(after.contains("refused #"), "{after}");
    assert!(
        after.contains(&format!("#{t}")),
        "the refusal names the card: {after}"
    );
    assert!(
        after.contains("rm_verifier_only"),
        "the refusal carries its code: {after}"
    );
    let refusals = after.matches("refused #").count();
    assert_eq!(
        refusals, 2,
        "one refusal, visible in plain and JSON log: {after}"
    );
    // nothing else moved: the card is still there, still in todo
    assert!(
        b.on_board(&t) && b.column(&t) == "todo",
        "the refused rm changed nothing: {}",
        b.column(&t)
    );
}

/// The refusal's hint is executable advice: run `tb note ID "CLOSE: …"` then
/// `tb move ID review` as the refused agent — the card lands in REVIEW, un-deleted.
#[test]
fn the_hint_closes_the_card_out() {
    let b = Board::new();
    let t = b.add("t: follow the hint");
    b.refused(AGENT, "b-x", &["rm", &t]);
    b.ok(AGENT, "b-x", &["note", &t, "CLOSE: superseded"]);
    b.ok(AGENT, "b-x", &["move", &t, "review"]);
    assert_eq!(
        b.column(&t),
        "review",
        "the hint closed the card out instead of a delete"
    );
}

/// C3b: an env-scrubbed rm — no harness in the identity (a `person` to `is_agent`), but the
/// kernel parent chain names an agent binary (`claude`, the #169 `agent_as_person` lane the
/// close and every person-only change already run): the rm of open work is refused the same
/// way, while a REGISTERED verifier's rm on that same chain still deletes.
#[test]
fn an_env_scrubbed_person_rm_follows_the_parent_chain() {
    let b = Board::new();
    b.registry.register(UUID, "rv-x", "claude-code");
    let t = b.add("t: scrub");
    let Some(scrub) = under_claude(&b, "charles", &["rm", &t, "--force"], &[]) else {
        eprintln!("skipped: no bash to copy as claude");
        return;
    };
    assert_eq!(scrub["code"], "agent_as_person", "the scrubbed rm: {scrub}");
    assert!(
        scrub["error"].as_str().is_some_and(|e| e.contains("rm #")),
        "the refusal names the rm: {scrub}"
    );
    assert!(b.on_board(&t), "the card survived the scrubbed rm");
    assert_eq!(
        b.column(&t),
        "todo",
        "the refused rm changed nothing: {}",
        b.column(&t)
    );
    // a refused transaction rolls back, so the refusal's log line is written separately
    assert!(
        b.log().contains("refused #"),
        "the scrubbed refusal is logged: {}",
        b.log()
    );
    // a registered verifier's rm on the SAME scrubbed chain still deletes: its identity is
    // an agent's (a harness on record), so the parent-chain lane is not its lane, and the
    // registered() lookup rides on the session the registry entry was minted for
    let Some(keep) = under_claude(&b, "rv-x", &["rm", &t], VERIFIER) else {
        panic!("no bash to copy as claude")
    };
    assert_eq!(
        keep["code"],
        serde_json::Value::Null,
        "the registered verifier rm went through: {keep}"
    );
    assert!(!b.on_board(&t), "the verifier rm deleted the card");
}

/// Runs `tb <args> --json --as <who>` as a 'person' (no harness in the env) from a shell
/// whose kernel name is `claude` — a copy of bash — so the agent binary is a real ancestor,
/// the way an env-scrubbed rm from a claude parent reaches tb. The copy-once setup and the
/// ETXTBSY retry follow `under_omp` in tests/verifier_rule.rs. `extra_env` carries what an
/// env-scrubbed call would still have had to name its own session (`TB_SESSION`).
fn under_claude(
    b: &Board,
    who: &str,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> Option<serde_json::Value> {
    let claude = claude_executable()?;
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let mut line = vec![quote(env!("CARGO_BIN_EXE_tb"))];
    line.extend(args.iter().map(|a| quote(a)));
    line.extend([
        "--json".to_string(),
        "--as".to_string(),
        quote(who),
        "; true".to_string(),
    ]);
    let mut c = Command::new(claude);
    c.args(["-c", &line.join(" ")])
        .env_clear()
        .env("TB_DB", b.db())
        .env("TB_VERIFIERS_DIR", b.registry.dir.path())
        .env("TB_NO_HERDR", "1")
        .env("TB_GH", "/nonexistent/gh")
        .env("USER", "login-user")
        .env("TZ", "UTC")
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", b.dir.path());
    c.envs(extra_env.iter().copied());
    for (i, wait) in [0u64, 10, 25, 50, 100, 200, 200, 200, 200, 200]
        .into_iter()
        .enumerate()
    {
        if wait > 0 {
            terminal_board::waits::pause(
                "under_claude exec busy (ETXTBSY) retry backoff",
                std::time::Duration::from_millis(wait),
            );
        }
        match c.output() {
            Err(e)
                if e.raw_os_error() == Some(26)
                    || e.kind() == std::io::ErrorKind::ExecutableFileBusy =>
            {
                eprintln!("under_claude: exec busy (ETXTBSY), retry {}/10", i + 1);
            }
            other => {
                let o =
                    other.unwrap_or_else(|e| panic!("exec claude (a copy of bash) failed: {e}"));
                return Some(serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null));
            }
        }
    }
    panic!("exec claude (a copy of bash) still ETXTBSY after 10 retries");
}

/// bash copied ONCE per test process, renamed `claude`, leaked so it outlives every thread.
fn claude_executable() -> Option<&'static PathBuf> {
    static CLAUDE: std::sync::LazyLock<Option<PathBuf>> = std::sync::LazyLock::new(|| {
        use std::os::unix::fs::PermissionsExt;
        let bash = ["/bin/bash", "/usr/bin/bash"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.exists())?;
        let dir = tempfile::tempdir().ok()?; // leaked via into_path: must outlive every thread
        let tmp = dir.path().join("claude.tmp");
        let n = std::fs::copy(&bash, &tmp).ok()?; // clonefile: sets 0755 itself, no leak
        if !(n & 0o111 == 0o111) {
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let claude = dir.path().join("claude");
        std::fs::rename(&tmp, &claude).ok()?;
        let kept = dir.keep(); // the dir must outlive every test thread; leaked on purpose
        Some(kept.join("claude"))
    });
    CLAUDE.as_ref()
}
