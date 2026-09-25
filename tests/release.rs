#![cfg(unix)]
//! `tb release ID "why"` (store/release.rs): a DOING card whose holder is DEAD goes back to
//! TODO, unowned, with a `released` event — for a lead, an orchestrator or a person; refused
//! while the holder is alive (`holder_alive`), for other agents (`not_releaser`), without a
//! reason, and for a card not in DOING (`not_in_doing`). Liveness is tb-reap's probe set
//! (`terminal_board::liveness`), always in fixture mode here (`TB_REAP_FAKE_*`), so nothing
//! asks the real herdr, tmux or process table.
use std::path::PathBuf;
use std::process::{Command, Output};

/// An agent harness on record, with `TB_ROLE` set per test.
fn agent(role: &str) -> Vec<(&'static str, String)> {
    vec![("TB_HARNESS", "claude-code".into()), ("TB_MODEL", "m".into()), ("TB_SESSION", format!("s-{role}")), ("TB_ROLE", role.into())]
}

struct Board {
    dir: tempfile::TempDir,
}

impl Board {
    fn new() -> Board {
        let b = Board { dir: tempfile::tempdir().unwrap() };
        let o = b.run(&[], &[], "charles", &["config", "wip", "9"]);
        assert!(o.status.success());
        b
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("b.db")
    }

    /// `env` is the identity; `fake` overrides the (all-empty) liveness fixture.
    fn run(&self, env: &[(&str, String)], fake: &[(&str, &str)], who: &str, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).arg("--as").arg(who).env_clear();
        c.env("TB_DB", self.db())
            .env("TB_NO_HERDR", "1")
            .env("TB_GH", "/nonexistent/gh")
            .env("USER", "login-user")
            .env("TZ", "UTC")
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.dir.path());
        for v in ["TB_REAP_FAKE_AGENTS", "TB_REAP_FAKE_TMUX", "TB_REAP_FAKE_PANES", "TB_REAP_FAKE_SESSIONS", "TB_REAP_FAKE_PROCS"] {
            c.env(v, "");
        }
        c.envs(fake.iter().copied());
        c.envs(env.iter().map(|(k, v)| (*k, v.as_str())));
        c.output().unwrap()
    }

    /// A DOING card held by the coder agent `owner`.
    fn held_by(&self, owner: &str) -> String {
        let o = self.run(&agent("coder"), &[], owner, &["add", "work", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        let id = v["card"]["id"].as_i64().unwrap().to_string();
        assert!(self.run(&agent("coder"), &[], owner, &["take", &id]).status.success());
        id
    }

    /// (column, owner, every event text joined).
    fn state(&self, id: &str) -> (String, String, String) {
        let o = self.run(&[], &[], "charles", &["show", id, "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        let c = if v.get("card").is_some() { &v["card"] } else { &v };
        let events = c["events"].as_array().map(|a| a.iter().map(|e| format!("{} {}", e["kind"].as_str().unwrap_or(""), e["text"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        (c["column"].as_str().unwrap_or("?").into(), c["owner"].as_str().unwrap_or("-").into(), events)
    }

    /// The refusal's `--json` code; asserts it WAS refused.
    fn refused(&self, env: &[(&str, String)], fake: &[(&str, &str)], who: &str, args: &[&str]) -> (String, String) {
        let mut a = args.to_vec();
        a.push("--json");
        let o = self.run(env, fake, who, &a);
        assert!(!o.status.success(), "tb {args:?} by {who} was allowed: {}", String::from_utf8_lossy(&o.stdout));
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
        (v["code"].as_str().unwrap_or("").to_string(), format!("{} — {}", v["error"].as_str().unwrap_or(""), v["hint"].as_str().unwrap_or("")))
    }
}

#[test]
fn a_lead_releases_a_dead_holders_card_to_todo_unowned_with_the_reason_logged() {
    let b = Board::new();
    let id = b.held_by("ghost");
    let o = b.run(&agent("lead"), &[], "lead-x", &["release", &id, "ghost's pane is gone", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
    let (col, owner, events) = b.state(&id);
    assert_eq!((col.as_str(), owner.as_str()), ("todo", "-"));
    assert!(events.contains("released released ghost"), "names the old holder: {events}");
    assert!(events.contains("ghost's pane is gone"), "logs the reason: {events}");
    assert!(!events.contains("force"), "no --force path: {events}");
}

#[test]
fn an_orchestrator_and_a_person_may_release() {
    let b = Board::new();
    let (one, two) = (b.held_by("g1"), b.held_by("g2"));
    assert!(b.run(&agent("orchestrator"), &[], "orch-x", &["release", &one, "cleanup"]).status.success());
    assert!(b.run(&[], &[], "charles", &["release", &two, "cleanup"]).status.success());
    assert_eq!(b.state(&one).0, "todo");
    assert_eq!(b.state(&two).0, "todo");
}

#[test]
fn a_live_holder_is_refused_holder_alive_naming_the_probe() {
    let b = Board::new();
    let id = b.held_by("g2");
    for (var, val, how) in [
        ("TB_REAP_FAKE_AGENTS", "g2", "herdr-agent"),
        ("TB_REAP_FAKE_TMUX", "g2", "tmux"),
        ("TB_REAP_FAKE_PANES", "worker · g2 · #1", "pane-label"),
        ("TB_REAP_FAKE_PROCS", "4242 omp --as g2", "process 4242"),
    ] {
        let (code, text) = b.refused(&agent("lead"), &[(var, val)], "lead-x", &["release", &id, "try"]);
        assert_eq!(code, "holder_alive", "{var}: {text}");
        assert!(text.contains(how), "{var}: says how it is alive: {text}");
        assert_eq!(b.state(&id).0, "doing");
    }
}

#[test]
fn other_agents_are_refused_not_releaser() {
    let b = Board::new();
    let id = b.held_by("g3");
    for role in ["coder", "verifier"] {
        let (code, _) = b.refused(&agent(role), &[], "b-x", &["release", &id, "mine now"]);
        assert_eq!(code, "not_releaser", "{role}");
    }
    // an agent with no role at all
    let env = vec![("TB_HARNESS", "claude-code".to_string())];
    assert_eq!(b.refused(&env, &[], "b-y", &["release", &id, "mine now"]).0, "not_releaser");
    let (col, owner, _) = b.state(&id);
    assert_eq!((col.as_str(), owner.as_str()), ("doing", "g3"));
}

#[test]
fn no_reason_or_a_card_not_in_doing_is_refused() {
    let b = Board::new();
    let id = b.held_by("g4");
    // no reason at all is a usage error; a blank one is reason_required
    assert!(!b.run(&agent("lead"), &[], "lead-x", &["release", &id]).status.success());
    assert_eq!(b.refused(&agent("lead"), &[], "lead-x", &["release", &id, "  "]).0, "reason_required");
    assert_eq!(b.state(&id).0, "doing");
    let o = b.run(&[], &[], "charles", &["add", "idle", "--json"]);
    let todo = serde_json::from_slice::<serde_json::Value>(&o.stdout).unwrap()["card"]["id"].as_i64().unwrap().to_string();
    assert_eq!(b.refused(&agent("lead"), &[], "lead-x", &["release", &todo, "not doing"]).0, "not_in_doing");
    assert_eq!(b.refused(&agent("lead"), &[], "lead-x", &["release", "999", "gone"]).0, "no_card");
}
