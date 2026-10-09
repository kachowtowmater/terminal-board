//! The ordered pre-change hook LIST (tb#270): `tb config hook a,b` runs a then b, the first
//! refusal wins, each name keeps its own `tb trust` pin, one name behaves exactly as before.
//!
//! CLI binary + raw files only, like tests/hooks.rs — no `terminal_board::` items — so this
//! compiles against a tree without the feature and fails by assertion.
#![cfg(unix)]
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home { dir: tempfile::tempdir().unwrap() }
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.current_dir(self.path()).env("HOME", self.path()).env("TB_AS", "alice").env("TB_NO_HERDR", "1").env("TZ", "UTC");
        for k in [
            "TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD", "TB_CONFIG", "TB_READONLY", "TTYBOARD_READONLY", "HERDR_AGENT_NAME",
            "XDG_STATE_HOME", "XDG_CONFIG_HOME", "TB_GH", "TB_HOOK_TOKEN", "TB_HOOK_DEPTH", "TB_IN_HOOK", "TB_ROLE", "TB_SESSION", "TB_CARD",
        ] {
            c.env_remove(k);
        }
        c.stdin(Stdio::null());
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stdout)
    }
    fn refused(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(1), "{args:?} should be refused: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stderr)
    }
    fn json(&self, args: &[&str]) -> Value {
        let mut a = args.to_vec();
        a.push("--json");
        serde_json::from_str(&self.ok(&a)).unwrap()
    }
    fn json_err(&self, args: &[&str]) -> Value {
        let mut a = args.to_vec();
        a.push("--json");
        let o = self.run(&a);
        assert_eq!(o.status.code(), Some(1), "{a:?} should be refused: {}{}", text(&o.stdout), text(&o.stderr));
        serde_json::from_str(&text(&o.stdout)).unwrap()
    }
    fn add(&self, title: &str) -> i64 {
        self.json(&["add", title])["card"]["id"].as_i64().unwrap()
    }
    fn column(&self, id: i64) -> String {
        self.json(&["show", &id.to_string()])["column"].as_str().unwrap().to_string()
    }
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let p = self.path().join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }
    /// `tb trust NAME -- SCRIPT` then `tb trust NAME --sha256 HEX` (the two-step flow).
    fn trust(&self, name: &str, script: &Path) {
        let v = self.json(&["trust", name, "--json", "--", script.to_str().unwrap()]);
        let digest = v["sha256"].as_str().unwrap().to_string();
        let v = self.json(&["trust", name, "--sha256", &digest]);
        assert_eq!(v["trusted"], true, "{v}");
    }
    /// A hook that appends its own name to `order.log` and exits `code`.
    fn logging_hook(&self, name: &str, code: i32) -> PathBuf {
        let dir = self.path().display().to_string();
        self.script(&format!("{name}.sh"), &format!("#!/bin/sh\necho {name} >> {dir}/order.log\nexit {code}\n"))
    }
    fn order(&self) -> Vec<String> {
        std::fs::read_to_string(self.path().join("order.log")).unwrap_or_default().lines().map(str::to_string).collect()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

fn names_of(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => vec![],
    }
}

// ---------------------------------------------------------------- the list is stored in order

#[test]
fn a_list_is_stored_in_the_order_given_and_shown_back() {
    let h = Home::new();
    h.add("x: card");
    h.ok(&["config", "hook", "alpha,beta,gamma"]);
    let v = h.json(&["config", "hook"]);
    assert_eq!(names_of(&v["config"]["value"]), vec!["alpha", "beta", "gamma"], "{v}");
    let plain = h.ok(&["config", "hook"]);
    assert!(plain.find("alpha").unwrap_or(99) < plain.find("beta").unwrap_or(100), "{plain:?}");
    let table = h.ok(&["config"]);
    let line = table.lines().find(|l| l.starts_with("hook")).unwrap_or("");
    assert!(line.contains("alpha") && line.contains("beta") && line.contains("gamma"), "{table}");
}

#[test]
fn every_member_is_validated_and_a_refused_list_changes_nothing() {
    let h = Home::new();
    h.add("x: card");
    h.ok(&["config", "hook", "alpha"]);
    let e = h.refused(&["config", "hook", "alpha,BETA!"]);
    assert!(e.contains("BETA!"), "the refusal names the bad member: {e}");
    let v = h.json(&["config", "hook"]);
    assert_eq!(names_of(&v["config"]["value"]), vec!["alpha"], "{v}");
}

#[test]
fn off_clears_the_whole_list() {
    let h = Home::new();
    h.add("x: card");
    h.ok(&["config", "hook", "alpha,beta"]);
    h.ok(&["config", "hook", "--off"]);
    assert_eq!(h.ok(&["config", "hook"]).trim(), "off");
    let id = h.add("x: second");
    h.ok(&["take", &id.to_string()]);
    assert_eq!(h.column(id), "doing");
}

// ---------------------------------------------------------------- run order

#[test]
fn the_list_runs_in_order_each_handed_the_same_proposal() {
    let h = Home::new();
    let id = h.add("x: card");
    let a = h.logging_hook("alpha", 0);
    let b = h.logging_hook("beta", 0);
    h.trust("alpha", &a);
    h.trust("beta", &b);
    h.ok(&["config", "hook", "alpha,beta"]);
    h.ok(&["take", &id.to_string()]);
    assert_eq!(h.column(id), "doing");
    assert_eq!(h.order(), vec!["alpha", "beta"]);
    let card = h.ok(&["show", &id.to_string()]);
    assert!(card.contains("alpha allowed") && card.contains("beta allowed"), "each run is recorded: {card}");
}

#[test]
fn reversing_the_config_reverses_the_run() {
    let h = Home::new();
    let id = h.add("x: card");
    let a = h.logging_hook("alpha", 0);
    let b = h.logging_hook("beta", 0);
    h.trust("alpha", &a);
    h.trust("beta", &b);
    h.ok(&["config", "hook", "beta,alpha"]);
    h.ok(&["take", &id.to_string()]);
    assert_eq!(h.order(), vec!["beta", "alpha"]);
}

// ---------------------------------------------------------------- first refusal wins

#[test]
fn the_first_refusal_wins_and_nothing_is_written() {
    let h = Home::new();
    let id = h.add("x: card");
    let a = h.logging_hook("alpha", 1);
    let b = h.logging_hook("beta", 0);
    h.trust("alpha", &a);
    h.trust("beta", &b);
    h.ok(&["config", "hook", "alpha,beta"]);
    let v = h.json_err(&["take", &id.to_string()]);
    assert_eq!(v["code"], "hook_refused", "{v}");
    assert_eq!(h.order(), vec!["alpha"], "beta never ran");
    assert_eq!(h.column(id), "todo", "nothing was written");
}

#[test]
fn a_later_refusal_still_refuses_and_runs_nothing_further() {
    let h = Home::new();
    let id = h.add("x: card");
    let a = h.logging_hook("alpha", 0);
    let b = h.logging_hook("beta", 1);
    let c = h.logging_hook("gamma", 0);
    h.trust("alpha", &a);
    h.trust("beta", &b);
    h.trust("gamma", &c);
    h.ok(&["config", "hook", "alpha,beta,gamma"]);
    let v = h.json_err(&["take", &id.to_string()]);
    assert_eq!(v["code"], "hook_refused", "{v}");
    assert_eq!(h.order(), vec!["alpha", "beta"], "gamma never ran");
    assert_eq!(h.column(id), "todo");
    let card = h.ok(&["show", &id.to_string()]);
    assert!(!card.contains("alpha allowed"), "a refused change writes no hook event at all: {card}");
}

#[test]
fn force_does_not_skip_any_member() {
    let h = Home::new();
    let id = h.add("x: card");
    let a = h.logging_hook("alpha", 0);
    let b = h.logging_hook("beta", 1);
    h.trust("alpha", &a);
    h.trust("beta", &b);
    h.ok(&["config", "hook", "alpha,beta"]);
    let v = h.json_err(&["move", &id.to_string(), "doing", "--force"]);
    assert_eq!(v["code"], "hook_refused", "{v}");
    assert_eq!(h.column(id), "todo");
}

// ---------------------------------------------------------------- each member pinned separately

#[test]
fn a_changed_member_refuses_naming_it_and_others_keep_their_pins() {
    let h = Home::new();
    let id = h.add("x: card one");
    let a = h.logging_hook("alpha", 0);
    let b = h.logging_hook("beta", 0);
    h.trust("alpha", &a);
    h.trust("beta", &b);
    h.ok(&["config", "hook", "alpha,beta"]);
    h.ok(&["take", &id.to_string()]);
    let pin_alpha = h.json(&["trust", "alpha"])["sha256"].clone();
    // beta's file changes under the same name and mode
    std::fs::write(&b, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o755)).unwrap();
    let id2 = h.add("x: card two");
    let e = h.refused(&["take", &id2.to_string()]);
    assert!(e.contains("'beta' has changed since it was trusted"), "names the changed member: {e}");
    assert_eq!(h.json(&["trust", "alpha"])["sha256"], pin_alpha, "alpha's pin is untouched");
    assert_eq!(h.json(&["trust", "alpha"])["state"], "trusted");
    // re-trusting only beta is enough
    let v = h.json(&["trust", "beta"]);
    let digest = v["sha256"].as_str().unwrap().to_string();
    h.json(&["trust", "beta", "--sha256", &digest]);
    h.ok(&["take", &id2.to_string()]);
    assert_eq!(h.column(id2), "doing");
}

#[test]
fn an_untrusted_member_refuses_naming_it() {
    let h = Home::new();
    let id = h.add("x: card");
    let a = h.logging_hook("alpha", 0);
    let b = h.logging_hook("beta", 0);
    h.trust("alpha", &a);
    h.ok(&["trust", "beta", "--", b.to_str().unwrap()]); // recorded, never confirmed
    h.ok(&["config", "hook", "alpha,beta"]);
    let e = h.refused(&["take", &id.to_string()]);
    assert!(e.contains("'beta' is not trusted"), "{e}");
    assert_eq!(h.column(id), "todo");
}

#[test]
fn an_unknown_member_refuses_naming_it() {
    let h = Home::new();
    let id = h.add("x: card");
    let a = h.logging_hook("alpha", 0);
    h.trust("alpha", &a);
    h.ok(&["config", "hook", "alpha,ghost"]);
    let e = h.refused(&["take", &id.to_string()]);
    assert!(e.contains("does not know the hook 'ghost'"), "{e}");
    assert_eq!(h.column(id), "todo");
}

// ---------------------------------------------------------------- one name: unchanged

#[test]
fn a_single_name_is_stored_and_printed_exactly_as_before() {
    let h = Home::new();
    h.add("x: card");
    let saved = h.json(&["config", "hook", "solo"]);
    assert_eq!(saved["config"]["value"], "solo", "a single name is still the plain string: {saved}");
    assert_eq!(h.ok(&["config", "hook"]).trim(), "solo");
    let all = h.json(&["config"]);
    assert_eq!(all["config"]["hook"], "solo", "{all}");
}

#[test]
fn a_single_trusted_hook_allows_and_refuses_as_before() {
    let h = Home::new();
    let id = h.add("x: card");
    let ok = h.logging_hook("solo", 0);
    h.trust("solo", &ok);
    h.ok(&["config", "hook", "solo"]);
    h.ok(&["take", &id.to_string()]);
    assert_eq!(h.column(id), "doing");
    let no = h.logging_hook("gate", 1);
    h.trust("gate", &no);
    h.ok(&["config", "hook", "gate"]);
    let id2 = h.add("x: card two");
    let v = h.json_err(&["take", &id2.to_string()]);
    assert_eq!(v["code"], "hook_refused", "{v}");
    assert_eq!(h.column(id2), "todo");
}

#[test]
fn break_glass_skips_a_whole_list_and_logs_it() {
    let h = Home::new();
    let id = h.add("x: card");
    let a = h.logging_hook("alpha", 1);
    let b = h.logging_hook("beta", 1);
    h.trust("alpha", &a);
    h.trust("beta", &b);
    h.ok(&["config", "hook", "alpha,beta"]);
    h.ok(&["take", &id.to_string(), "--break-glass", "both down, ops said go"]);
    assert_eq!(h.column(id), "doing");
    let log = h.ok(&["log"]);
    assert!(log.contains("break-glass"), "{log}");
}
