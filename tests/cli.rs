use std::process::{Command, Output};

fn keys(v: &serde_json::Value) -> Vec<String> {
    use std::collections::BTreeSet;
    v.as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

fn sorted(list: &[&str]) -> Vec<String> {
    use std::collections::BTreeSet;
    list.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>().into_iter().collect()
}

struct Board {
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
}

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sub/board.db");
        Board { _dir: dir, db }
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("TB_DB", &self.db)
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .env_remove("HERDR_AGENT_NAME")
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }
}

/// M7: --help fits one screen.
#[test]
fn help_fits_one_screen() {
    let b = Board::new();
    let out = b.ok(&["--help"]);
    let n = out.lines().count();
    assert!(n <= 25, "--help is {n} lines:\n{out}");
    for verb in [
        "add", "list", "show", "next", "take", "note", "check", "move", "done", "block", "drop", "config", "boards",
        "github", "rm", "prio", "edit", "sync", "board", "watch", "agents",
    ] {
        let found = out.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == verb);
        assert!(found, "missing verb {verb}:\n{out}");
    }
    assert!(out.starts_with(&format!("tb {} - Terminal Board", env!("CARGO_PKG_VERSION"))) && !out.contains("ttyboard"), "{out}");
    assert_eq!(out.lines().last().unwrap(), "agents: run 'tb guide' for the full agent manual");
}

/// M4: bare run with stdout not a TTY prints the board once and exits.
#[test]
fn bare_non_tty_prints_board() {
    let b = Board::new();
    b.ok(&["add", "admin: renew domain"]);
    b.ok(&["add", "widgets: gh#7 fix lifter", "--check", "repro", "--check", "patch"]);
    b.ok(&["take", "2"]);
    let out = b.ok(&[]);
    for s in ["TODO (1)", "DOING (1/3)", "REVIEW (0)", "DONE today (0)", "#1 renew domain", "#2 gh#7 fix lifter", "tester"] {
        assert!(out.contains(s), "missing {s:?} in:\n{out}");
    }
}

/// M1: the verbs work end to end, --json on read commands.
#[test]
fn verbs_and_json() {
    let b = Board::new();
    b.ok(&["add", "ops: outline runbook", "-d", "chapters 1-3", "--check", "ch1", "--check", "ch2"]);
    b.ok(&["add", "second"]);
    let next: serde_json::Value = serde_json::from_str(&b.ok(&["next", "--json"])).unwrap();
    assert_eq!(next["ok"], true);
    let next = &next["card"];
    assert_eq!(next["id"], 1);
    assert_eq!(next["column"], "doing");
    assert_eq!(next["owner"], "tester");
    assert_eq!(next["tag"], "ops");
    b.ok(&["note", "1", "started ch1"]);
    b.ok(&["check", "1", "1"]);
    assert!(b.ok(&["check", "1", "--add", "ch3"]).contains("item 3 added"));
    b.ok(&["check", "1", "--rm", "2"]);
    let chk: serde_json::Value = serde_json::from_str(&b.ok(&["show", "1", "--json"])).unwrap();
    let texts: Vec<_> = chk["checklist"].as_array().unwrap().iter().map(|c| (c["idx"].as_i64().unwrap(), c["text"].as_str().unwrap().to_string())).collect();
    assert_eq!(texts, [(1, "ch1".to_string()), (2, "ch3".to_string())]);
    // `n` is the canonical key in both `show --json` and `board --json`; `idx` is its alias
    for c in chk["checklist"].as_array().unwrap() {
        assert_eq!(c["n"], c["idx"]);
    }
    let board: serde_json::Value = serde_json::from_str(&b.ok(&["board", "--json"])).unwrap();
    let card1 = board["columns"]["doing"].as_array().unwrap().iter().find(|c| c["id"] == 1).unwrap();
    let shown: Vec<_> = chk["checklist"].as_array().unwrap().iter().map(|c| (c["n"].clone(), c["text"].clone())).collect();
    let boarded: Vec<_> = card1["checklist"].as_array().unwrap().iter().map(|c| (c["n"].clone(), c["text"].clone())).collect();
    assert_eq!(shown, boarded);
    assert!(!b.run(&["check", "1"]).status.success());
    assert!(b.ok(&["config", "theme", "light"]).contains("light"));
    assert!(!b.run(&["config", "theme", "blue"]).status.success());
    let o = b.run(&["config", "tag", "admin"]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("config wip"), "tag colours are gone");
    let show: serde_json::Value = serde_json::from_str(&b.ok(&["show", "1", "--json"])).unwrap();
    assert_eq!(show["checklist"][0]["done"], true);
    assert!(show["events"].as_array().unwrap().iter().any(|e| e["text"] == "started ch1"));
    assert!(b.ok(&["done", "1"]).contains("review"));
    b.ok(&["move", "2", "review"]);
    b.ok(&["drop", "2"]);
    let list: serde_json::Value = serde_json::from_str(&b.ok(&["list", "--json"])).unwrap();
    let cols: Vec<_> = list.as_array().unwrap().iter().map(|c| c["column"].as_str().unwrap().to_string()).collect();
    assert_eq!(cols, ["review", "todo"]);
    assert!(b.ok(&["list"]).contains("#2 second"));
    b.ok(&["--as", "someone-else", "take", "2"]);
    let c: serde_json::Value = serde_json::from_str(&b.ok(&["show", "2", "--json"])).unwrap();
    assert_eq!(c["owner"], "someone-else");
}

/// M3 via the CLI: actionable refusal, non-zero exit.
#[test]
fn cli_wip_refusal_and_errors() {
    let b = Board::new();
    b.ok(&["config", "wip", "1"]);
    b.ok(&["add", "a"]);
    b.ok(&["add", "b"]);
    b.ok(&["next"]);
    let o = b.run(&["next"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("doing is full (1/1)") && err.contains("tb done"), "{err}");
    let o = b.run(&["show", "77"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("tb list"));
}


#[test]
fn block_marks_card_and_next_skips_it() {
    let b = Board::new();
    b.ok(&["add", "waits"]);
    b.ok(&["add", "free"]);
    b.ok(&["block", "1", "#9"]);
    assert!(b.ok(&[]).contains("x blocked by #9"));
    let n: serde_json::Value = serde_json::from_str(&b.ok(&["next", "--json"])).unwrap();
    assert_eq!(n["card"]["id"], 2, "next skips the blocked card");
    let o = b.run(&["next"]);
    assert!(!o.status.success(), "only a blocked card left");
    b.ok(&["block", "1", "--clear"]);
    let n: serde_json::Value = serde_json::from_str(&b.ok(&["next", "--json"])).unwrap();
    assert_eq!(n["card"]["id"], 1);
    assert!(!b.ok(&["show", "1"]).contains("x blocked"));
    b.ok(&["block", "2", "by #1"]);
    assert!(b.ok(&["show", "2"]).contains("x blocked by #1"), "by prefix normalised");
}

#[test]
fn guide_prints_the_agent_manual() {
    let b = Board::new();
    let out = b.ok(&["guide"]);
    assert!(out.starts_with("# Terminal Board — the agent manual"), "{out}");
    assert!(out.contains("tb next --as NAME") && out.contains("Full manual: `tb guide`"));
    assert!(out.lines().count() <= 250);
}

#[test]
fn config_lists_all_and_sets_panels() {
    let b = Board::new();
    let out = b.ok(&["config"]);
    for want in ["wip           3", "theme         dark", "layout        auto", "github        off", "github-panel  shown", "agents-panel  shown"] {
        assert!(out.contains(want), "{want}:\n{out}");
    }
    b.ok(&["config", "github-panel", "hidden"]);
    b.ok(&["config", "agents-panel", "hidden"]);
    let v: serde_json::Value = serde_json::from_str(&b.ok(&["config", "--json"])).unwrap();
    assert_eq!(v["config"]["github-panel"], "hidden");
    assert_eq!(v["config"]["agents-panel"], "hidden");
    let o = b.run(&["config", "agents-panel", "maybe"]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("shown|hidden"));
    // connecting a repo un-hides the GitHub panel
    b.ok(&["config", "github", "o/r"]);
    assert!(b.ok(&["config"]).contains("github-panel  shown"));
}

#[test]
fn parse_errors_answer_json_under_json_flag() {
    let b = Board::new();
    b.ok(&["add", "docs: target card"]);
    // bad value: a non-numeric id
    let o = b.run(&["show", "abc", "--json"]);
    assert!(!o.status.success());
    assert!(o.status.code().unwrap() != 0);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(keys(&v), sorted(&["ok", "error", "hint"]), "{}", v);
    assert_eq!(v["ok"], false);
    assert!(v["hint"].as_str().unwrap().contains("tb --help"), "{}", v);
    // missing argument
    let o = b.run(&["note", "1", "--json"]);
    assert!(!o.status.success());
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], false, "{}", v);
    assert!(v["hint"].as_str().unwrap().contains("tb --help"), "{}", v);
    // unknown flag (a bare unknown word stays the documented board-open behavior)
    let o = b.run(&["--frobnicate", "--json"]);
    assert!(!o.status.success());
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(v["error"].as_str().unwrap().contains("argument error"), "{}", v);
    // control: a runtime failure under --json keeps its exact shape and rc 1
    let o = b.run(&["done", "99", "--json"]);
    assert!(!o.status.success() && o.status.code().unwrap() == 1);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["hint"], "see 'tb list' for ids", "{}", v);
    // negative control: without --json a parse failure stays plain on stderr, empty stdout
    let o = b.run(&["show", "abc"]);
    assert!(!o.status.success() && o.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&o.stderr).is_empty());
    // --help with --json still prints help text, exit 0
    let o = b.run(&["--help", "--json"]);
    assert!(o.status.success() && String::from_utf8_lossy(&o.stdout).contains("Usage:"));
}
