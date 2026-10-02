#![cfg(unix)]
//! The board's reader list (`tb config readers NAME,NAME`, store/readers.rs): when a board
//! sets it, every command that opens the board — read or write, a named board or `TB_DB` —
//! is refused to an agent not acting under a listed name. A person passes; only a person
//! sets or clears the list. Everything here drives the real `tb` binary with a controlled
//! environment (so whatever harness runs the suite never leaks in) and only through the CLI,
//! so a tree without the rule fails these by assertion, not by compile error.
mod common;

use std::path::PathBuf;
use std::process::{Command, Output};

/// An agent: a harness on record, no role.
const AGENT: &[(&str, &str)] = &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "0b9f6a52-7c1d-4e0a-9f3b-2a6c1d8e4f70")];
/// A person in a plain terminal: nothing but the name.
const PERSON: &[(&str, &str)] = &[];

struct Board {
    dir: tempfile::TempDir,
}

impl Board {
    fn new() -> Board {
        let b = Board { dir: tempfile::tempdir().unwrap() };
        b.ok(PERSON, "charles", &["config", "wip", "9"]);
        b
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("b.db")
    }

    fn run_raw(&self, env: &[(&str, String)], who: &str, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).env_clear();
        c.env("TB_DB", self.db())
            .env("TB_NO_HERDR", "1")
            .env("TB_GH", "/nonexistent/gh")
            .env("USER", "login-user")
            .env("TZ", "UTC")
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.dir.path());
        // `"-"` = no name at all: no TB_AS, no --as — the identity-less caller. It still
        // carries its harness env so `is_agent` sees it; `who` is only what tb names in
        // refusals (the login name, `resolve_actor`'s last resort).
        if who != "-" {
            c.env("TB_AS", who);
        }
        c.envs(env.iter().map(|(k, v)| (*k, v.as_str())));
        c.output().unwrap()
    }

    fn ok(&self, env: &[(&'static str, &str)], who: &str, args: &[&str]) -> String {
        let o = self.run_raw(&Board::str_env(env), who, args);
        assert!(o.status.success(), "{who}: tb {args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn str_env(env: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
        env.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    /// The `--json` refusal: (error text + hint, code). Asserts it WAS refused.
    fn refused(&self, env: &[(&'static str, &str)], who: &str, args: &[&str]) -> (String, String) {
        self.refused_raw(env, who, args)
    }

    fn refused_raw(&self, env: &[(&'static str, &str)], who: &str, args: &[&str]) -> (String, String) {
        // identical to `refused`; a second name keeps call sites that may accept more than
        // one code (the open's `not_a_reader` can speak before a person-only rule does)
        // readable in the test body
        let mut a = args.to_vec();
        a.push("--json");
        let o = self.run_raw(&Board::str_env(env), who, &a);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
        assert!(!o.status.success(), "{who}: tb {args:?} was allowed: {v}");
        let text = format!("{} — {}", v["error"].as_str().unwrap_or(""), v["hint"].as_str().unwrap_or(""));
        (text, v["code"].as_str().unwrap_or("").to_string())
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_str(&self.ok(PERSON, "charles", args)).unwrap()
    }
}

/// A person sets the list, reads it, and clears it; the plain output and the JSON agree.
#[test]
fn a_person_sets_prints_and_clears_readers() {
    let b = Board::new();
    b.ok(PERSON, "charles", &["config", "readers", "tb-box-enforcer, lead-bar"]);
    let out = b.ok(PERSON, "charles", &["config", "readers"]);
    assert!(out.contains("tb-box-enforcer") && out.contains("lead-bar"), "{out}");
    let v = b.json(&["config", "readers", "--json"]);
    assert_eq!(v["config"]["value"], serde_json::json!(["tb-box-enforcer", "lead-bar"]), "{v}");
    // the settings listing carries it too, so `tb config` says what the board is
    let listing = b.ok(PERSON, "charles", &["config"]);
    assert!(listing.lines().any(|l| l.starts_with("readers") && l.contains("tb-box-enforcer")), "{listing}");
    // clearing it removes the restriction (the next test reads what that means)
    b.ok(PERSON, "charles", &["config", "readers", "--off"]);
    let out = b.ok(PERSON, "charles", &["config", "readers"]);
    assert!(out.contains("readers is off"), "{out}");
    // an empty value is refused, saying what a name list wants
    let (e, code) = b.refused(PERSON, "charles", &["config", "readers", "  "]);
    assert_eq!(code, "arg_required", "{e}");
}

/// An agent (by harness, listed or not) can never set, edit or clear the list — and a
/// 'person' identity inside an agent's process (the `omp`-ancestor shape card #169's
/// `agent_as_person` catches) cannot either.
#[test]
fn only_a_person_changes_readers() {
    let b = Board::new();
    b.ok(PERSON, "charles", &["config", "readers", "tb-box-enforcer"]);
    for (what, env, who) in [
        ("a listed agent", AGENT, "tb-box-enforcer"),
        ("an unlisted agent", AGENT, "intruder"),
        ("an identity-less agent", PERSON, "-"),
    ] {
        for args in [&["config", "readers", "intruder"][..], &["config", "readers", "--off"]] {
            // a board with readers refuses the off-list agent at the open (`not_a_reader`)
            // BEFORE the person-only rule could speak (`person_only`) — the same one gate
            // every command meets; what matters is that the change is refused either way
            let (e, code) = b.refused(env, who, args);
            assert!(
                code == "person_only" || code == "not_a_reader",
                "{what}: {args:?}: expected person_only or not_a_reader: {e}"
            );
            if code == "person_only" {
                assert!(e.contains("only a person may change 'config readers'"), "{what}: {e}");
            }
        }
    }
    // the list is exactly as the person set it
    let v = b.json(&["config", "readers", "--json"]);
    assert_eq!(v["config"]["value"], serde_json::json!(["tb-box-enforcer"]), "{v}");
}

/// On a board with readers, an unlisted agent and an identity-less agent are refused
/// everything — a read and a write — with the board, the caller and the list named, and
/// nothing is written.
#[test]
fn an_off_list_agent_is_refused_reads_and_writes() {
    let b = Board::new();
    b.ok(PERSON, "charles", &["add", "private: one", "--json"]);
    b.ok(PERSON, "charles", &["config", "readers", "tb-box-enforcer,lead-bar"]);

    // a read, by harness and by nothing-but-ancestry-later (this one: harness env)
    for (what, env, who) in [("a harness agent", AGENT, "lead-fleet"), ("no name at all", AGENT, "-")] {
        let empty: &[(&str, &str)] = &[];
        let env = if what == "no name at all" { empty } else { env }; // no TB_AS, no name
        let caller = if who == "-" { "login-user" } else { who }; // the login name is what the refusal can name
        let (e, code) = b.refused(env, who, &["list"]);
        assert_eq!(code, "not_a_reader", "{what}: {e}");
        assert!(e.contains("board 'default'"), "{what}: the refusal names the board: {e}");
        assert!(e.contains(caller), "{what}: the refusal names the caller: {e}");
        assert!(e.contains("tb-box-enforcer") && e.contains("lead-bar"), "{what}: the refusal names the list: {e}");
        assert!(e.contains("only a person sets or clears the list"), "{what}: the refusal says who lifts it: {e}");
        let (e, code) = b.refused(env, who, &["show", "1"]);
        assert_eq!(code, "not_a_reader", "{what}: show too: {e}");
        // a write, refused before anything lands
        let (e, code) = b.refused(env, who, &["add", "sneaky: from the outside"]);
        assert_eq!(code, "not_a_reader", "{what}: add too: {e}");
    }
    assert_eq!(b.json(&["list", "--json"])["columns"]["todo"].as_array().map(|a| a.len()), Some(1), "no sneaky card was made");
}

/// An agent acting under a listed name passes (trimmed, case-insensitive), a person passes
/// whatever its name, and the same command on a board with no readers is refused to nobody.
#[test]
fn a_listed_agent_and_every_person_pass() {
    let b = Board::new();
    b.ok(PERSON, "charles", &["config", "readers", "tb-box-enforcer,lead-bar"]);
    b.ok(PERSON, "charles", &["add", "private: readable by its readers"]);
    // the list matches the way every stored name matches: trimmed, without case
    let out = b.ok(AGENT, "  TB-BOX-ENFORCER  ", &["list"]);
    assert!(out.contains("private"), "{out}");
    b.ok(AGENT, "lead-bar", &["note", "1", "readers can write too — that is what they are"]);
    // a person passes under any name, on a read and a write
    b.ok(PERSON, "charles", &["list"]);
    b.ok(PERSON, "somebody-else", &["add", "a person's card"]);

    // a board with NO readers behaves exactly as before, for every caller
    let c = Board::new();
    c.ok(PERSON, "charles", &["add", "open: one"]);
    c.ok(AGENT, "any-worker", &["list"]);
    c.ok(AGENT, "any-worker", &["note", "1", "open board, open reads"]);
    let _ = c.ok(&[][..], "login-user", &["list"]);
}

/// The escape card #1079 used — an agent that scrubs its environment (`env -i`) — does not
/// pass as a person: the kernel's parent chain still says `omp`, and an agent whose ancestry
/// says agent is refused with the same `not_a_reader`.
#[test]
fn a_scrubbed_env_under_an_agent_process_is_still_refused() {
    let omp = omp_executable();
    let Some(omp) = omp else {
        eprintln!("skipped: no bash to copy as omp");
        return;
    };
    let b = Board::new();
    b.ok(PERSON, "charles", &["add", "private: one card"]);
    b.ok(PERSON, "charles", &["config", "readers", "tb-box-enforcer"]);
    // `; true` keeps bash from exec'ing tb in place (which would drop `omp` from the chain);
    // the env is empty apart from what a bare run needs — no TB_AS, no harness marker.
    let line = format!("'{}' list --json; true", env!("CARGO_BIN_EXE_tb"));
    let o = Command::new(omp)
        .args(["-c", &line])
        .env_clear()
        .env("TB_DB", b.db())
        .env("TB_NO_HERDR", "1")
        .env("TB_GH", "/nonexistent/gh")
        .env("USER", "login-user")
        .env("TZ", "UTC")
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", b.dir.path())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
    assert_eq!(v["code"], "not_a_reader", "env -i under omp: {v}");
    let whole = format!("{} — {}", v["error"].as_str().unwrap_or(""), v["hint"].as_str().unwrap_or(""));
    assert!(whole.contains("login-user"), "the caller is named: {v}");
}

/// A copy of bash named `omp`, one per test process — the exact fixture
/// `tests/verifier_rule.rs` uses for the ancestry question (`omp_executable` there; the copy
/// lands by write-temp → close → chmod → rename, so the exec'd path never has an open
/// writer, and a fork inheriting the write fd is the race the caller retries).
fn omp_executable() -> Option<&'static PathBuf> {
    static OMP: std::sync::LazyLock<Option<PathBuf>> = std::sync::LazyLock::new(|| {
        use std::os::unix::fs::PermissionsExt;
        let bash = ["/bin/bash", "/usr/bin/bash"].into_iter().map(PathBuf::from).find(|p| p.exists())?;
        let dir = tempfile::tempdir().ok()?; // leaked via into_path: must outlive every thread
        let tmp = dir.path().join("omp.tmp");
        let n = std::fs::copy(&bash, &tmp).ok()?; // clonefile: sets 0755 itself, no leak
        if !(n & 0o111 == 0o111) {
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let omp = dir.path().join("omp");
        std::fs::rename(&tmp, &omp).ok()?;
        let kept = dir.keep(); // the dir must outlive every test thread; leaked on purpose
        Some(kept.join("omp"))
    });
    OMP.as_ref()
}
