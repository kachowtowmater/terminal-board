//! The pre/post-change hook (A1): `tb config hook|hook-after`, `tb trust`, `--break-glass`.
//!
//! CLI binary + raw `rusqlite` only, on purpose — no `terminal_board::hooks`/`store` calls.
//! Compiled and run against a build that has none of this (plain `origin/main`), every test
//! here still COMPILES (nothing references an item that does not exist there) and FAILS BY
//! ASSERTION: `tb trust`/`tb config hook` are unknown commands, so `.ok(...)` panics on a
//! non-zero exit and `.refused(...)`'s text does not contain what a real hook would have said.
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
    fn cmd(&self, args: &[&str], actor: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).current_dir(self.path()).env("HOME", self.path()).env("TB_AS", actor).env("TB_NO_HERDR", "1").env("TZ", "UTC");
        for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD", "TB_CONFIG", "TB_READONLY", "TTYBOARD_READONLY", "HERDR_AGENT_NAME", "XDG_STATE_HOME", "TB_GH"] {
            c.env_remove(k);
        }
        c.stdin(Stdio::null());
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args, "alice").output().unwrap()
    }
    fn run_as(&self, args: &[&str], actor: &str) -> Output {
        self.cmd(args, actor).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stdout)
    }
    fn ok_as(&self, args: &[&str], actor: &str) -> String {
        let o = self.run_as(args, actor);
        assert!(o.status.success(), "{args:?} as {actor} failed: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stdout)
    }
    fn refused(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(1), "{args:?} should be refused: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stderr)
    }
    /// `args` with `--json` appended — never for a `trust NAME -- CMD…` call, where anything
    /// after `--` is captured as the command's own argv (put `--json` before `--` and use
    /// [`Home::json_raw`] there instead).
    fn json(&self, args: &[&str]) -> Value {
        let mut a = args.to_vec();
        a.push("--json");
        serde_json::from_str(&self.ok(&a)).unwrap()
    }
    /// `args` exactly as given — for a `trust NAME --json -- CMD…` call, where `--json` has to
    /// be placed by the caller (see [`Home::json`]).
    fn json_raw(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
    fn json_err(&self, args: &[&str]) -> Value {
        let mut a = args.to_vec();
        a.push("--json");
        let o = self.run(&a);
        assert_eq!(o.status.code(), Some(1), "{a:?} should be refused: {}", text(&o.stdout));
        serde_json::from_str(&text(&o.stdout)).unwrap()
    }
    fn add(&self, title: &str) -> i64 {
        self.json(&["add", title])["card"]["id"].as_i64().unwrap()
    }
    fn column(&self, id: i64) -> String {
        self.json(&["show", &id.to_string()])["column"].as_str().unwrap().to_string()
    }
    /// A script this machine can be asked to trust, `0755`.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let p = self.path().join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }
    /// Records and confirms a hook in one step: `tb trust NAME -- SCRIPT`, then reads the
    /// printed digest back and `tb trust NAME --sha256 HEX` — the two-step flow a person or
    /// agent goes through by hand, just automated. `--json` goes BEFORE `--`: `json()`'s
    /// auto-append would otherwise land after it and become part of the recorded argv.
    fn trust(&self, name: &str, script: &Path) {
        let v = self.json_raw(&["trust", name, "--json", "--", script.to_str().unwrap()]);
        assert_eq!(v["trusted"], false, "recording alone must not trust it: {v}");
        let digest = v["sha256"].as_str().unwrap().to_string();
        let v = self.json(&["trust", name, "--sha256", &digest]);
        assert_eq!(v["trusted"], true, "{v}");
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

const ALLOW: &str = "#!/bin/sh\nexit 0\n";
const REFUSE: &str = "#!/bin/sh\necho 'not on my watch'\nexit 1\n";

// ---------------------------------------------------------------- no hook: unchanged

#[test]
fn a_board_with_no_hook_set_is_unaffected() {
    let h = Home::new();
    let id = h.add("x: card one");
    h.ok(&["take", &id.to_string()]);
    h.ok(&["done", &id.to_string()]);
    h.ok_as(&["done", &id.to_string()], "bob"); // review -> done: a different agent, never the author
    let cfg = h.ok(&["config"]);
    assert!(!cfg.contains("hook"), "a board that asks for no hook lists nothing about one: {cfg}");
}

// ---------------------------------------------------------------- fail-closed

#[test]
fn a_hook_this_machine_does_not_know_refuses_the_change() {
    let h = Home::new();
    let id = h.add("x: card");
    h.ok(&["config", "hook", "approve"]);
    let v = h.json_err(&["take", &id.to_string()]);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"], "hook refused");
    assert_eq!(v["code"], "hook_refused");
    assert!(v["hint"].as_str().unwrap().contains("does not know the hook 'approve'"), "{v}");
    // fail-closed means nothing changed: the card is still in todo, unowned
    let card = h.json(&["show", &id.to_string()]);
    assert_eq!(card["column"], "todo");
    assert_eq!(card["owner"], Value::Null);
}

#[test]
fn recorded_but_not_yet_trusted_still_refuses() {
    let h = Home::new();
    let id = h.add("x: card");
    let script = h.script("approve.sh", ALLOW);
    h.ok(&["config", "hook", "approve"]);
    h.ok(&["trust", "approve", "--", script.to_str().unwrap()]);
    let e = h.refused(&["take", &id.to_string()]);
    assert!(e.contains("not trusted"), "{e}");
    assert_eq!(h.column(id), "todo");
}

#[test]
fn a_trusted_allowing_hook_lets_the_change_through() {
    let h = Home::new();
    let id = h.add("x: card");
    let script = h.script("approve.sh", ALLOW);
    h.ok(&["config", "hook", "approve"]);
    h.trust("approve", &script);
    h.ok(&["take", &id.to_string()]);
    assert_eq!(h.column(id), "doing");
}

#[test]
fn a_trusted_refusing_hook_blocks_the_change_and_shows_its_own_words() {
    let h = Home::new();
    let id = h.add("x: card");
    let script = h.script("gate.sh", REFUSE);
    h.ok(&["config", "hook", "gate"]);
    h.trust("gate", &script);
    let v = h.json_err(&["take", &id.to_string()]);
    assert_eq!(v["hint"], "not on my watch", "the hint is the hook's own last line: {v}");
    assert_eq!(h.column(id), "todo");
}

#[test]
fn force_does_not_skip_the_hook() {
    let h = Home::new();
    let id = h.add("x: card");
    h.ok_as(&["take", &id.to_string()], "bob"); // bob holds it in DOING
    let script = h.script("gate.sh", REFUSE);
    h.ok(&["config", "hook", "gate"]);
    h.trust("gate", &script);
    // --force is meant to get alice past the HOLDER guard (bob owns this DOING card); the hook
    // is asked about the move regardless of --force (the proposal it sees says "forced": true)
    // and its no still refuses the whole change — --force only ever waives tb's OWN guards.
    let v = h.json_err(&["move", &id.to_string(), "review", "--force"]);
    assert_eq!(v["code"], "hook_refused", "{v}");
    assert_eq!(h.column(id), "doing", "still with bob — --force never bypasses a hook's refusal");
}

// ---------------------------------------------------------------- the payload

#[test]
fn the_hook_receives_the_documented_json_on_stdin() {
    let h = Home::new();
    let id = h.add("x: dump me");
    let dump = h.path().join("payload.json");
    let script = h.script("dump.sh", &format!("#!/bin/sh\ncat > {}\nexit 0\n", dump.display()));
    h.ok(&["config", "hook", "dump"]);
    h.trust("dump", &script);
    h.ok(&["take", &id.to_string()]);
    let raw = std::fs::read_to_string(&dump).unwrap();
    assert_eq!(raw.lines().count(), 1, "one JSON line: {raw}");
    let v: Value = serde_json::from_str(raw.trim()).unwrap();
    assert_eq!(v["v"], 1);
    assert_eq!(v["event"], "pre-change");
    assert_eq!(v["card"]["id"], id);
    assert_eq!(v["from"], "todo");
    assert_eq!(v["to"], "doing");
    assert_eq!(v["actor"], "alice");
    assert_eq!(v["forced"], false);
    assert!(v["ts"].as_i64().unwrap() > 0);
}

// ---------------------------------------------------------------- timeout

#[test]
fn a_hook_that_does_not_answer_in_time_is_stopped_and_refuses() {
    let h = Home::new();
    let id = h.add("x: card");
    let script = h.script("slow.sh", "#!/bin/sh\nsleep 5\nexit 0\n");
    h.ok(&["config", "hook", "slow"]);
    let recorded = h.json_raw(&["trust", "slow", "--timeout", "1", "--json", "--", script.to_str().unwrap()]);
    let digest = recorded["sha256"].as_str().unwrap();
    h.ok(&["trust", "slow", "--sha256", digest]);
    let started = std::time::Instant::now();
    let v = h.json_err(&["take", &id.to_string()]);
    assert!(started.elapsed().as_secs() < 4, "stopped well before the 5s sleep: {:?}", started.elapsed());
    assert!(v["hint"].as_str().unwrap().contains("did not answer in 1s"), "{v}");
    assert_eq!(h.column(id), "todo");
}

// ---------------------------------------------------------------- break-glass

#[test]
fn break_glass_skips_a_refusing_hook_and_is_logged() {
    let h = Home::new();
    let id = h.add("x: card");
    let script = h.script("gate.sh", REFUSE);
    h.ok(&["config", "hook", "gate"]);
    h.trust("gate", &script);
    h.ok(&["take", &id.to_string(), "--break-glass", "gate is broken, ops said go"]);
    assert_eq!(h.column(id), "doing");
    let card = h.ok(&["show", &id.to_string()]);
    assert!(card.contains("break-glass") && card.contains("gate is broken"), "{card}");
    let log = h.ok(&["log"]);
    assert!(log.contains("break-glass"), "the board log carries it too, not only the card: {log}");
}

#[test]
fn break_glass_with_nothing_to_break_is_refused() {
    let h = Home::new();
    let id = h.add("x: card");
    let e = h.refused(&["take", &id.to_string(), "--break-glass", "why"]);
    assert!(e.contains("this board asks for none"), "{e}");
    assert_eq!(h.column(id), "todo", "refused before anything ran");
}

// ---------------------------------------------------------------- trust tampering

#[test]
fn a_changed_command_is_refused_until_retrusted() {
    let h = Home::new();
    let id = h.add("x: card");
    let script = h.script("flip.sh", ALLOW);
    h.ok(&["config", "hook", "flip"]);
    h.trust("flip", &script);
    h.ok(&["take", &id.to_string()]);
    // the file changes under the same name and mode — a re-run must not trust it blindly
    std::fs::write(&script, REFUSE).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let id2 = h.add("x: card two");
    let e = h.refused(&["take", &id2.to_string()]);
    assert!(e.contains("has changed since it was trusted"), "{e}");
    let v = h.json(&["trust", "flip"]);
    assert_eq!(v["state"], "CHANGED", "{v}");
}

// ---------------------------------------------------------------- post-change

#[test]
fn a_post_change_hook_runs_but_never_blocks_the_change() {
    let h = Home::new();
    let id = h.add("x: card");
    let marker = h.path().join("after.ran");
    let script = h.script("after.sh", &format!("#!/bin/sh\ntouch {}\nexit 1\n", marker.display()));
    h.ok(&["config", "hook-after", "after"]);
    h.trust("after", &script);
    // the change succeeds even though the post-change hook always fails
    h.ok(&["take", &id.to_string()]);
    assert_eq!(h.column(id), "doing");
    assert!(marker.exists(), "the post-change hook still ran");
    let card = h.ok(&["show", &id.to_string()]);
    assert!(card.contains("after failed"), "its failure is reported, not swallowed: {card}");
}

// ---------------------------------------------------------------- re-entrancy

#[test]
fn a_hook_is_told_it_is_a_hook_so_it_never_recurses() {
    let h = Home::new();
    let id = h.add("x: card");
    let dump = h.path().join("env.txt");
    let script = h.script(
        "envdump.sh",
        &format!("#!/bin/sh\nprintf '%s %s %s' \"$TB_IN_HOOK\" \"$TB_HOOK_EVENT\" \"$TB_HOOK_NAME\" > {}\nexit 0\n", dump.display()),
    );
    h.ok(&["config", "hook", "envdump"]);
    h.trust("envdump", &script);
    h.ok(&["take", &id.to_string()]);
    assert_eq!(std::fs::read_to_string(&dump).unwrap(), "1 pre-change envdump");
}

// ---------------------------------------------------------------- tb sync is exempt

/// A fake `gh` (`TB_GH`) that reports issue #10 as CLOSED (absent from `issue list`'s open
/// page): the evidence `tb sync` needs to move an UNOWNED todo card straight to done, so this
/// never has to route through `tb take` (which the always-refusing hook below would refuse).
fn gh_issue_10_closed(dir: &Path) -> PathBuf {
    let p = dir.join("gh-fake");
    std::fs::write(
        &p,
        r#"#!/bin/sh
case "$1 $2" in
  "repo view") echo '{"nameWithOwner":"o/r"}';;
  "pr list") echo '[]';;
  "issue list") echo '[]';;
  "run list") echo '[]';;
  "api repos/o/r/issues/10") echo '{"state":"closed"}';;
  api*) echo 1;;
  *) exit 2;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[test]
fn tb_sync_is_exempt_from_the_hook_by_origin_not_by_actor_name() {
    let h = Home::new();
    let script = h.script("gate.sh", REFUSE);
    h.ok(&["config", "hook", "gate"]);
    h.trust("gate", &script);
    let gh = gh_issue_10_closed(h.path());
    let gh_run = |args: &[&str]| -> Output {
        let mut c = h.cmd(args, "alice");
        c.env("TB_GH", &gh);
        c.output().unwrap()
    };
    let o = gh_run(&["config", "github", "o/r"]);
    assert!(o.status.success(), "config github: {}", text(&o.stderr));
    let id = h.add("x: gh#10 closed already");
    // confirm the hook really does bind a person: a plain move is refused
    let e = h.refused(&["move", &id.to_string(), "done"]);
    assert!(e.contains("hook refused") || e.contains("gate"), "{e}");
    assert_eq!(h.column(id), "todo");
    // `tb sync`, same board, same hook trusted and still refusing: the closed issue's card
    // still reaches done — an internal ORIGIN, not the actor `sync` runs the CLI as
    let o = gh_run(&["sync", "--json"]);
    assert!(o.status.success(), "sync: {}", text(&o.stderr));
    assert_eq!(h.column(id), "done");
}
