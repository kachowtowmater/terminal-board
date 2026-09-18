//! v2: inside a herdr pane the actor is the pane's herdr agent name, not the login name.
#![cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[
  {"name":"bot-1","agent":"claude","agent_status":"working","pane_id":"w:p2"},
  {"agent":"claude","agent_status":"idle","pane_id":"w:p3","terminal_title_stripped":"lead"}
]}}"#;

struct Env {
    dir: tempfile::TempDir,
    herdr: PathBuf,
    n: std::cell::Cell<u32>,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("agents.json"), AGENTS).unwrap();
        let herdr = dir.path().join("herdr");
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{0}/calls'\n[ \"$1 $2\" = \"agent list\" ] && exec cat '{0}/agents.json'\nexit 1\n",
            dir.path().display()
        );
        std::fs::write(&herdr, script).unwrap();
        std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
        Env { dir, herdr, n: 0.into() }
    }

    /// Adds a card with `env` set and returns the actor tb logged it under.
    fn actor(&self, env: &[(&str, &str)], args: &[&str]) -> String {
        let db = self.db();
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(["add", "a card"]).args(args);
        for k in ["TB_AS", "TTYBOARD_AS", "TB_NO_HERDR", "HERDR_AGENT_NAME", "HERDR_PANE_ID", "HERDR_ENV", "HERDR_BIN_PATH"] {
            c.env_remove(k);
        }
        c.env("TB_DB", &db).env("TB_GH", "/nonexistent/gh").env("USER", "login-user").env("PATH", "/usr/bin:/bin");
        c.envs(env.iter().copied());
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let n = self.n.get() + 1;
        self.n.set(n);
        let show = Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(["show", &n.to_string(), "--json"])
            .env("TB_DB", &db)
            .env("TB_NO_HERDR", "1")
            .output()
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
        v["events"][0]["actor"].as_str().unwrap().to_string()
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("b.db")
    }

    fn herdr_called(&self) -> bool {
        Path::new(&self.dir.path().join("calls")).exists()
    }

    fn in_pane<'a>(&'a self, pane: &'a str) -> Vec<(&'a str, &'a str)> {
        vec![("HERDR_ENV", "1"), ("HERDR_PANE_ID", pane), ("HERDR_BIN_PATH", self.herdr.to_str().unwrap())]
    }
}

#[test]
fn outside_herdr_the_login_name_is_used_and_herdr_is_not_asked() {
    let e = Env::new();
    assert_eq!(e.actor(&[("HERDR_BIN_PATH", e.herdr.to_str().unwrap())], &[]), "login-user");
    assert!(!e.herdr_called());
}

#[test]
fn a_named_herdr_pane_gives_its_agent_name() {
    let e = Env::new();
    assert_eq!(e.actor(&e.in_pane("w:p2"), &[]), "bot-1");
    assert!(e.herdr_called());
}

#[test]
fn explicit_names_still_win_over_the_pane() {
    let e = Env::new();
    let mut env = e.in_pane("w:p2");
    assert_eq!(e.actor(&env, &["--as", "chosen"]), "chosen");
    env.push(("HERDR_AGENT_NAME", "from-env"));
    assert_eq!(e.actor(&env, &[]), "from-env");
    env.push(("TB_AS", "tb-as"));
    assert_eq!(e.actor(&env, &[]), "tb-as");
}

#[test]
fn unnamed_or_unknown_pane_and_disabled_herdr_fall_back_to_login() {
    let e = Env::new();
    assert_eq!(e.actor(&e.in_pane("w:p3"), &[]), "login-user", "unnamed agent: no guessing from titles");
    assert_eq!(e.actor(&e.in_pane("w:p9"), &[]), "login-user");
    let mut env = e.in_pane("w:p2");
    env.push(("TB_NO_HERDR", "1"));
    assert_eq!(e.actor(&env, &[]), "login-user");
    // a herdr that fails or is missing is never fatal
    let env = vec![("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w:p2"), ("HERDR_BIN_PATH", "/nonexistent/herdr")];
    assert_eq!(e.actor(&env, &[]), "login-user");
}
