//! `tb-reap` through its binary only: one test per false positive of the 2026-09-24 05:00
//! `--apply` run (factory #3, factory #5, orch #8, orch #23), a genuinely dead owner that must
//! still be flagged (so the widened liveness is not vacuous), and the code-level `--apply`
//! guard (TB_REAP_MODE=live AND a >= 7-day dry-run log with 0 false positives).
//!
//! Every run is in fixture mode (`TB_REAP_FAKE_*` set), so nothing here asks the real herdr,
//! tmux or process table.
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fx {
    _dir: tempfile::TempDir,
    db: PathBuf,
    state: PathBuf,
    home: PathBuf,
}

fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("r.db");
    let state = dir.path().join("state");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    Fx { db, state, home, _dir: dir }
}

fn tb(f: &Fx, who: &str, session: Option<&str>, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
    c.args(args)
        .env("TB_DB", &f.db)
        .env("HOME", &f.home)
        .env("TB_GH", "/nonexistent/gh")
        .env("TB_AS", who)
        .env("TB_NO_HERDR", "1")
        .env_remove("TB_BOARD")
        .env_remove("HERDR_AGENT_NAME")
        .env_remove("HERDR_PANE_ID");
    match session {
        Some(s) => c.env("TB_SESSION", s),
        None => c.env_remove("TB_SESSION"),
    };
    c.output().unwrap()
}

/// A DOING card held by `owner` (taken under `session` when given). Returns its id.
fn doing(f: &Fx, owner: &str, session: Option<&str>) -> i64 {
    let o = tb(f, owner, session, &["add", &format!("fx: {owner}"), "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let id = v["card"]["id"].as_i64().unwrap();
    let t = tb(f, owner, session, &["take", &id.to_string()]);
    assert!(t.status.success(), "take: {}", String::from_utf8_lossy(&t.stderr));
    id
}

fn column(f: &Fx, id: i64) -> String {
    let o = tb(f, "rv-test", None, &["show", &id.to_string(), "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    v["column"].as_str().unwrap_or("?").to_string()
}

/// Run tb-reap in full fixture mode. `extra` overrides/extends the fixture env.
fn reap(f: &Fx, args: &[&str], extra: &[(&str, &str)]) -> (Output, serde_json::Value) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_tb-reap"));
    c.args(args)
        .env("TB_DB", &f.db)
        .env("HOME", &f.home)
        .env("TB_REAP_STATE_DIR", &f.state)
        .env("TB_REAP_KILLSWITCH", f.home.join("no-such-switch"))
        .env("TB_REAP_DEAD_AFTER", "0")
        .env("TB_REAP_FAKE_AGENTS", "")
        .env("TB_REAP_FAKE_TMUX", "")
        .env("TB_REAP_FAKE_PANES", "")
        .env("TB_REAP_FAKE_SESSIONS", "")
        .env("TB_REAP_FAKE_PROCS", "")
        .env_remove("TB_REAP_MODE")
        .env_remove("TB_REAP_PROTECT")
        .env_remove("TB_AS")
        .env_remove("TB_BOARD")
        .env_remove("HERDR_AGENT_NAME");
    for (k, v) in extra {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    let v = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
    (o, v)
}

fn dead(v: &serde_json::Value) -> i64 {
    v["dead_owner_cards"].as_i64().unwrap_or(-1)
}

// ---- liveness: one test per false positive ------------------------------------------------

/// Control: an owner with no agent, pane, process, session or orchestrator name IS dead —
/// the widened checks must not make everything look alive.
#[test]
fn a_genuinely_dead_owner_is_still_flagged() {
    let f = fx();
    doing(&f, "ghost", None);
    let (_, v) = reap(&f, &["--dry-run", "--json"], &[]);
    assert_eq!(dead(&v), 1, "{v}");
}

/// factory #5: codex-u5 was a plain `codex exec` pane — no herdr agent, but its label names it.
#[test]
fn fp_factory5_plain_pane_whose_label_names_the_owner_is_alive() {
    let f = fx();
    doing(&f, "codex-u5", None);
    let label = "worker2 · codex (OpenAI) · codex exec · codex-u5 · U5 queue RED suite r2 (#5)";
    let (_, v) = reap(&f, &["--dry-run", "--json"], &[("TB_REAP_FAKE_PANES", label)]);
    assert_eq!(dead(&v), 0, "{v}");
    // a label that merely CONTAINS the name as a substring of another word does not count
    let (_, v) = reap(&f, &["--dry-run", "--json"], &[("TB_REAP_FAKE_PANES", "worker2 · codex-u55 · other")]);
    assert_eq!(dead(&v), 1, "substring is not a match: {v}");
}

/// factory #5: a running codex process that names the owner (env or args) keeps it alive.
#[test]
fn fp_factory5_codex_process_naming_the_owner_is_alive() {
    let f = fx();
    doing(&f, "codex-u5", None);
    let (_, v) = reap(&f, &["--dry-run", "--json"], &[("TB_REAP_FAKE_PROCS", "4242 codex exec --full-auto TB_AS=codex-u5 HOME=/x")]);
    assert_eq!(dead(&v), 0, "env TB_AS: {v}");
    let (_, v) = reap(&f, &["--dry-run", "--json"], &[("TB_REAP_FAKE_PROCS", "4243 /opt/bin/omp --name codex-u5")]);
    assert_eq!(dead(&v), 0, "argv name: {v}");
    let (_, v) = reap(&f, &["--dry-run", "--json"], &[("TB_REAP_FAKE_PROCS", "4244 /usr/bin/vim codex-u5.txt")]);
    assert_eq!(dead(&v), 1, "a non-agent process does not count: {v}");
}

/// factory #3: bld-u3b's tb actor session was the live orchestrator's herdr session.
#[test]
fn fp_factory3_owner_session_that_is_a_live_herdr_agent_is_alive() {
    let f = fx();
    doing(&f, "bld-u3b", Some("358dc645-orch-chick-2"));
    let (_, v) = reap(&f, &["--dry-run", "--json"], &[("TB_REAP_FAKE_SESSIONS", "358dc645-orch-chick-2")]);
    assert_eq!(dead(&v), 0, "{v}");
    let (_, v) = reap(&f, &["--dry-run", "--json"], &[("TB_REAP_FAKE_SESSIONS", "some-other-session")]);
    assert_eq!(dead(&v), 1, "a different live session does not count: {v}");
}

/// orch #8: orch-lss, a live orchestrator not registered in herdr under that name.
#[test]
fn fp_orch8_the_orchestrator_is_never_a_dead_owner() {
    let f = fx();
    doing(&f, "orch-lss", None);
    let (_, v) = reap(&f, &["--dry-run", "--json"], &[]);
    assert_eq!(dead(&v), 0, "orch-* names: {v}");
    let g = fx();
    doing(&g, "conductor", None);
    let (_, v) = reap(&g, &["--dry-run", "--json"], &[("TB_REAP_PROTECT", "lead,conductor")]);
    assert_eq!(dead(&v), 0, "TB_REAP_PROTECT: {v}");
    let (_, v) = reap(&g, &["--dry-run", "--json"], &[("TB_AS", "conductor")]);
    assert_eq!(dead(&v), 0, "the reaper's own caller: {v}");
}

/// orch #23: b-M-5, a headless builder. `mode:headless pid=N` is alive while pid N runs,
/// and releasable once it is gone; `mode:headless` with no pid stays conservatively alive.
#[test]
fn fp_orch23_headless_card_is_judged_by_the_pid_in_its_note() {
    let f = fx();
    let id = doing(&f, "b-M-5", None);
    assert!(tb(&f, "b-M-5", None, &["note", &id.to_string(), "mode:headless pid=4343"]).status.success());
    let (_, v) = reap(&f, &["--dry-run", "--json"], &[("TB_REAP_FAKE_PROCS", "4343 claude -p brief.md")]);
    assert_eq!(dead(&v), 0, "pid alive: {v}");
    let (_, v) = reap(&f, &["--dry-run", "--json"], &[("TB_REAP_FAKE_PROCS", "9999 claude -p other.md")]);
    assert_eq!(dead(&v), 1, "pid gone: {v}");
    let g = fx();
    let id = doing(&g, "b-M-5", None);
    assert!(tb(&g, "b-M-5", None, &["note", &id.to_string(), "mode:headless"]).status.success());
    let (_, v) = reap(&g, &["--dry-run", "--json"], &[]);
    assert_eq!(dead(&v), 0, "no pid recorded: {v}");
}

// ---- the --apply guard ----------------------------------------------------------------------

fn iso(secs_ago: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(secs_ago)).format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// A dry-run log whose first (empty) line is `days` old; `flagged` entries (board, id, owner)
/// sit on a line stamped NOW — after the fixture cards exist, as a real flag would be.
fn write_log(f: &Fx, days: i64, board: &str, flagged: &[(i64, &str)]) {
    std::fs::create_dir_all(&f.state).unwrap();
    let detail: Vec<serde_json::Value> =
        flagged.iter().map(|(id, o)| serde_json::json!({"board": board, "id": id, "owner": o})).collect();
    let empty = serde_json::json!({"liveness": 2, "mode": "dry-run", "dead_owner_cards": 0, "dead_owner_detail": []});
    let flag = serde_json::json!({"liveness": 2, "mode": "dry-run", "dead_owner_cards": detail.len(), "dead_owner_detail": detail});
    let text = format!("{} {empty}\n{} {flag}\n", iso(days * 86400), iso(0));
    std::fs::write(f.state.join("dry-run.log"), text).unwrap();
}

fn board_name(f: &Fx) -> String {
    let (_, v) = reap(f, &["--dry-run", "--json"], &[]);
    v["boards_scanned"][0].as_str().unwrap().to_string()
}

fn refused(o: &Output, v: &serde_json::Value) -> bool {
    !o.status.success() && v["refused"] == true
}

#[test]
fn apply_is_refused_without_tb_reap_mode_live() {
    let f = fx();
    let id = doing(&f, "ghost", None);
    let b = board_name(&f);
    write_log(&f, 8, &b, &[]);
    let (o, v) = reap(&f, &["--apply", "--json"], &[]);
    assert!(refused(&o, &v), "{v}");
    let (o, v) = reap(&f, &["--apply", "--json"], &[("TB_REAP_MODE", "dry-run")]);
    assert!(refused(&o, &v), "{v}");
    assert_eq!(column(&f, id), "doing", "nothing released");
}

#[test]
fn apply_is_refused_when_the_dry_run_log_spans_under_7_days() {
    let f = fx();
    let id = doing(&f, "ghost", None);
    let b = board_name(&f);
    write_log(&f, 3, &b, &[]);
    let (o, v) = reap(&f, &["--apply", "--json"], &[("TB_REAP_MODE", "live")]);
    assert!(refused(&o, &v), "{v}");
    assert_eq!(column(&f, id), "doing");
    // and with no log at all
    let g = fx();
    doing(&g, "ghost", None);
    let (o, v) = reap(&g, &["--apply", "--json"], &[("TB_REAP_MODE", "live")]);
    assert!(refused(&o, &v), "{v}");
}

/// A card the dry run flagged whose owner then wrote to it was a false positive.
#[test]
#[allow(clippy::disallowed_methods, reason = "a test staging a later event clock; not a write-path wait")]
fn apply_is_refused_when_the_log_holds_a_false_positive() {
    let f = fx();
    let id = doing(&f, "ghost", None);
    let b = board_name(&f);
    let fp = doing(&f, "bld-u3b", None);
    write_log(&f, 8, &b, &[(fp, "bld-u3b")]);
    std::thread::sleep(std::time::Duration::from_millis(1100)); // tb event clocks are seconds
    assert!(tb(&f, "bld-u3b", None, &["note", &fp.to_string(), "still working"]).status.success());
    let (o, v) = reap(&f, &["--apply", "--json"], &[("TB_REAP_MODE", "live")]);
    assert!(refused(&o, &v), "{v}");
    assert!(v["false_positives"].as_array().is_some_and(|a| !a.is_empty()), "{v}");
    assert_eq!(column(&f, id), "doing");
}

/// A human-recorded false positive in the ledger also blocks live mode.
#[test]
fn apply_is_refused_when_the_false_positive_ledger_has_an_entry() {
    let f = fx();
    let id = doing(&f, "ghost", None);
    let b = board_name(&f);
    write_log(&f, 8, &b, &[]);
    std::fs::write(f.state.join("false-positives.log"), "# header\n2026-09-24 factory#5 codex-u5\n").unwrap();
    let (o, v) = reap(&f, &["--apply", "--json"], &[("TB_REAP_MODE", "live")]);
    assert!(refused(&o, &v), "{v}");
    assert_eq!(column(&f, id), "doing");
}

#[test]
fn apply_releases_after_a_clean_7_day_dry_run_in_live_mode() {
    let f = fx();
    let id = doing(&f, "ghost", None);
    let b = board_name(&f);
    write_log(&f, 8, &b, &[(id, "ghost")]); // flagged, and the owner never came back: a true positive
    let (o, v) = reap(&f, &["--apply", "--json"], &[("TB_REAP_MODE", "live")]);
    assert!(o.status.success(), "{v} {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(v["mode"], "apply", "{v}");
    assert_eq!(column(&f, id), "todo");
}

#[test]
fn dry_run_log_lines_carry_the_liveness_version() {
    let f = fx();
    doing(&f, "ghost", None);
    reap(&f, &["--dry-run", "--json"], &[]);
    let log = std::fs::read_to_string(f.state.join("dry-run.log")).unwrap();
    let line = log.lines().last().unwrap();
    let (_, json) = line.split_once(' ').unwrap();
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(v["liveness"], 2, "{line}");
}
