//! Tests #227 r2: the REAL process probe (no `TB_REAP_FAKE_*` var set) must never count the
//! caller's own process tree — tb's own argv (`tb release N "omp b-N finished"` names the
//! holder) or the agent shell that launched it — as proof the holder is alive, while an
//! unrelated live agent process that names the holder must still vouch.
//!
//! Each test's process table is otherwise empty of vouches, so with the
//! `exclude_ancestry` call removed the only vouch left is the caller's own tree — the
//! release is (wrongly) REFUSED. That refusal is the assertion that bites.
#![cfg(unix)]
use std::path::PathBuf;
use std::process::{Command, Output};

/// ps is the probe's whole real-world source; without it the world is empty and these
/// tests cannot bite (CI's ubuntu and macOS runners both have it).
fn has_ps() -> bool {
    Command::new("ps").output().map(|o| o.status.success()).unwrap_or(false)
}

fn agent(role: &str) -> Vec<(&'static str, String)> {
    vec![("TB_HARNESS", "claude-code".into()), ("TB_MODEL", "m".into()), ("TB_SESSION", format!("s-{role}")), ("TB_ROLE", role.into())]
}

/// A DOING card held by `owner`, without the shared fixture: identity env only, and NO
/// `TB_REAP_FAKE_*` var — liveness asks the REAL process table.
struct RealBoard {
    dir: tempfile::TempDir,
}

impl RealBoard {
    fn new() -> RealBoard {
        let b = RealBoard { dir: tempfile::tempdir().unwrap() };
        let o = b.run(&[], "charles", &["config", "wip", "9"]);
        assert!(o.status.success(), "config wip failed: {}", String::from_utf8_lossy(&o.stdout));
        b
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("b.db")
    }

    fn run(&self, env: &[(&str, String)], who: &str, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).arg("--as").arg(who).env_clear();
        c.env("TB_DB", self.db())
            .env("TB_NO_HERDR", "1")
            .env("TB_GH", "/nonexistent/gh")
            .env("USER", "login-user")
            .env("TZ", "UTC")
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.dir.path());
        c.envs(env.iter().map(|(k, v)| (*k, v.as_str())));
        c.output().unwrap()
    }

    /// A DOING card held by the coder agent `owner`.
    fn held_by(&self, owner: &str) -> String {
        let o = self.run(&agent("coder"), owner, &["add", "work", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        let id = v["card"]["id"].as_i64().unwrap().to_string();
        assert!(self.run(&agent("coder"), owner, &["take", &id]).status.success());
        id
    }

    /// (column, owner).
    fn state(&self, id: &str) -> (String, String) {
        let o = self.run(&[], "charles", &["show", id, "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        let c = if v.get("card").is_some() { &v["card"] } else { &v };
        (c["column"].as_str().unwrap_or("?").into(), c["owner"].as_str().unwrap_or("-").into())
    }

    /// Release `id` as a lead agent, with `--json`, asserting nothing about the outcome.
    fn release_json(&self, id: &str, reason: &str) -> (i32, String) {
        let env = vec![("TB_HARNESS", "claude-code".to_string()), ("TB_ROLE", "lead".to_string())];
        let o = self.run(&env, "lead-x", &["release", id, reason, "--json"]);
        (
            o.status.code().unwrap_or(-1),
            format!("{}|{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)),
        )
    }
}

/// The agent env for role `role`, as one shell line (`K=v K=v`) for use inside `-c`.
fn shell_env(role: &str) -> String {
    agent(role).iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")
}

#[test]
fn real_ps_release_succeeds_when_only_the_caller_argv_names_the_holder() {
    if !has_ps() {
        eprintln!("skipping: no ps in this environment");
        return;
    }
    let b = RealBoard::new();
    let id = b.held_by("b-140");
    // tb's OWN argv names an agent binary and the holder — the exact shape of the repro
    // ('tb release 140 "<reason containing b-140>"' refused with a fresh pid each call).
    let (rc, out) = b.release_json(&id, "omp b-140 finished");
    assert_eq!(rc, 0, "own argv must not vouch for b-140: {out}");
    let (col, owner) = b.state(&id);
    assert_eq!((col.as_str(), owner.as_str()), ("todo", "-"));
}

#[test]
fn real_ps_release_succeeds_when_only_an_omp_shell_ancestor_names_the_holder() {
    if !has_ps() {
        eprintln!("skipping: no ps in this environment");
        return;
    }
    let b = RealBoard::new();
    let id = b.held_by("b-141");
    // The agent shell running tb: argv0 basename omp, argv names the holder (`-c` keeps
    // the shell as tb's live ancestor — no exec-away), so the ppid walk must drop it.
    let omp = b.dir.path().join("omp");
    let tb_bin = b.dir.path().join("tb");
    std::fs::copy(env!("CARGO_BIN_EXE_tb"), &tb_bin).expect("copy tb");
    std::fs::copy("/bin/sh", &omp).expect("copy /bin/sh as omp");
    let sh = Command::new(&omp)
        .arg("-c")
        .arg(format!(
            "export {env}; ./tb release {id} 'holder gone' --as lead-x; echo TBRC=$?",
            env = shell_env("lead")
        ))
        .arg("b-141")
        .current_dir(b.dir.path())
        .env_clear()
        .env("TB_DB", b.db())
        .env("TB_NO_HERDR", "1")
        .env("TB_GH", "/nonexistent/gh")
        .env("USER", "login-user")
        .env("TZ", "UTC")
        .env("HOME", b.dir.path())
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run omp shell");
    let out = String::from_utf8_lossy(&sh.stdout);
    let rc = out.lines().find(|l| l.starts_with("TBRC=")).and_then(|l| l[5..].parse::<i32>().ok());
    assert_eq!(rc, Some(0), "ancestor shell must not vouch for b-141: {out} / {}", String::from_utf8_lossy(&sh.stderr));
    let (col, owner) = b.state(&id);
    assert_eq!((col.as_str(), owner.as_str()), ("todo", "-"));
}

/// Whether a live process naming `holder` shows in the real ps table right now.
fn has_omp_row() -> bool {
    Command::new("ps")
        .args(["-ww", "-A", "-o", "command="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.split_whitespace().any(|t| t == "omp")))
        .unwrap_or(false)
}

#[test]
fn real_ps_an_unrelated_live_agent_process_naming_the_holder_still_vouches() {
    if !has_ps() {
        eprintln!("skipping: no ps on this environment");
        return;
    }
    let b = RealBoard::new();
    let id = b.held_by("b-8");
    let omp = b.dir.path().join("omp");
    std::fs::copy("/bin/sh", &omp).expect("copy /bin/sh as omp");
    let mut live = Command::new(&omp).arg("-c").arg("sleep 6; true").arg("b-8").spawn().expect("spawn live omp");
    // spin until the live process is visible in ps (ps is asked whole-table, so one pass
    // has it as soon as it is scheduled; a plain sleep is refused by clippy.toml here)
    for _ in 0..50 {
        if has_omp_row() {
            break;
        }
        std::hint::spin_loop();
    }
    let (rc, out) = b.release_json(&id, "is it dead");
    assert_ne!(rc, 0, "a live omp naming b-8 must vouch: {out}");
    assert!(out.contains("holder_alive"), "names the code: {out}");
    assert_eq!(b.state(&id).0, "doing", "not released while a live omp names b-8");
    let _ = live.wait();
}
