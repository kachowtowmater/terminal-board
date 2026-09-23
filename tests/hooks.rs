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

// ---------------------------------------------------------------- nested calls: never forgeable, never silent

/// Everything a hook's own `tb` call needs to find this test's board: the same `HOME` (and so
/// the same default board) the outer `tb` was given, which the hook inherits from it.
fn nested_take(id: i64, actor: &str) -> String {
    format!("#!/bin/sh\n\"{}\" take {id} --as {actor} >/dev/null 2>&1 || exit 3\nexit 0\n", env!("CARGO_BIN_EXE_tb"))
}

#[test]
fn a_hook_cannot_be_skipped_by_setting_an_environment_variable() {
    let h = Home::new();
    let id = h.add("x: card");
    let script = h.script("gate.sh", REFUSE);
    h.ok(&["config", "hook", "gate"]);
    h.trust("gate", &script);
    let hex = "a".repeat(64);
    let forged: [&[(&str, &str)]; 5] = [
        &[("TB_IN_HOOK", "1")],
        &[("TTYBOARD_IN_HOOK", "1")],
        &[("TB_IN_HOOK", "1"), ("TB_HOOK_TOKEN", &hex), ("TB_HOOK_NAME", "gate"), ("TB_HOOK_EVENT", "pre-change")],
        &[("TB_HOOK_TOKEN", "../config.json")],
        &[("TB_HOOK_DEPTH", "1")],
    ];
    for vars in forged {
        let mut c = h.cmd(&["take", &id.to_string(), "--json"], "mallory");
        for (k, v) in vars {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        assert_eq!(o.status.code(), Some(1), "{vars:?} got past the hook: {}", text(&o.stdout));
        let v: Value = serde_json::from_str(&text(&o.stdout)).unwrap();
        assert_eq!(v["code"], "hook_refused", "{vars:?}: {v}");
        assert_eq!(h.column(id), "todo", "{vars:?} moved the card");
    }
}

#[test]
fn a_change_made_from_inside_the_hook_is_recorded_on_the_card_and_the_board() {
    let h = Home::new();
    let id = h.add("x: outer");
    let other = h.add("x: taken by the hook itself");
    let script = h.script("inner.sh", &nested_take(other, "bob"));
    h.ok(&["config", "hook", "inner"]);
    h.trust("inner", &script);
    h.ok(&["take", &id.to_string()]);
    assert_eq!(h.column(id), "doing");
    let v = h.json(&["show", &other.to_string()]);
    assert_eq!((v["column"].as_str(), v["owner"].as_str()), (Some("doing"), Some("bob")), "the hook's own call went through: {v}");
    let nested: Vec<&Value> = v["events"].as_array().unwrap().iter().filter(|e| e["kind"] == "hook-nested").collect();
    assert_eq!(nested.len(), 1, "a change that skipped the hook says so on the card: {v}");
    assert!(nested[0]["text"].as_str().unwrap().contains("inner"), "{v}");
    let log = h.ok(&["log"]);
    assert!(log.contains("hook-nested") && log.contains(&format!("#{other}")), "and in the board log: {log}");
}

#[test]
fn a_card_that_changed_while_the_hook_ran_is_refused_with_its_own_code() {
    let h = Home::new();
    let id = h.add("x: contested");
    // the hook itself takes the very card it is being asked about, for someone else
    let script = h.script("race.sh", &nested_take(id, "bob"));
    h.ok(&["config", "hook", "race"]);
    h.trust("race", &script);
    let v = h.json_err(&["take", &id.to_string()]);
    assert_eq!(v["code"], "hook_race", "{v}");
    assert!(v["hint"].as_str().unwrap_or_default().contains("retry"), "{v}");
    let card = h.json(&["show", &id.to_string()]);
    assert_eq!(card["owner"], "bob", "the outer change was not applied on top: {card}");
}

// ---------------------------------------------------------------- tb trust NAME --timeout

#[test]
fn trust_timeout_alone_changes_a_known_hooks_timeout_and_keeps_its_trust() {
    let h = Home::new();
    let script = h.script("ok.sh", ALLOW);
    h.trust("ok", &script);
    assert_eq!(h.json(&["trust", "ok"])["timeout_secs"], 10);
    h.json(&["trust", "ok", "--timeout", "30"]);
    let v = h.json(&["trust", "ok"]);
    assert_eq!(v["timeout_secs"], 30, "{v}");
    assert_eq!(v["state"], "trusted", "a new time limit is not a new command: {v}");
    let v = h.json_err(&["trust", "ok", "--timeout", "0"]);
    assert_eq!(v["code"], "invalid_value", "{v}");
    assert_eq!(h.json(&["trust", "ok"])["timeout_secs"], 30, "a refused value changes nothing");
    let v = h.json_err(&["trust", "nope", "--timeout", "5"]);
    assert_eq!(v["code"], "no_hook", "{v}");
    let v = h.json(&["trust"]);
    assert_eq!(v["hooks"].as_array().unwrap().len(), 1, "an unknown name is not recorded by a timeout: {v}");
}

// ---------------------------------------------------------------- the file checked is the file run

#[test]
fn a_hook_file_swapped_after_it_was_checked_never_runs() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let h = Home::new();
    let id = h.add("x: card");
    let evil_ran = h.path().join("evil.ran");
    let good_ran = h.path().join("good.ran");
    let good = format!("#!/bin/sh\ntouch {}\nexit 0\n", good_ran.display());
    let evil = format!("#!/bin/sh\ntouch {}\nexit 0\n", evil_ran.display());
    let hook = h.script("gate.sh", &good);
    h.ok(&["config", "hook", "gate"]);
    h.trust("gate", &hook);
    // a file that was never trusted is renamed over the trusted one and back, as fast as
    // possible, while the board is worked: whatever tb hashed must be exactly what it runs
    let stop = Arc::new(AtomicBool::new(false));
    let swapper = {
        let (stop, dir, hook) = (stop.clone(), h.path().to_path_buf(), hook.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                for (body, tmp) in [(&evil, "evil.tmp"), (&good, "good.tmp")] {
                    let t = dir.join(tmp);
                    std::fs::write(&t, body).unwrap();
                    std::fs::set_permissions(&t, std::fs::Permissions::from_mode(0o755)).unwrap();
                    std::fs::rename(&t, &hook).unwrap();
                }
            }
        })
    };
    for i in 0..300 {
        let _ = h.run(&[if i % 2 == 0 { "take" } else { "drop" }, &id.to_string()]);
    }
    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();
    assert!(!evil_ran.exists(), "a file whose digest was never trusted ran");
    assert!(good_ran.exists(), "the trusted file never ran at all — the test did not exercise the hook");
}
