//! The headless-liveness split between the two `alive_by` callers: an explicit
//! `tb release ID "why"` treats a `mode:headless` holder whose note records NO pid as DEAD
//! (nothing but the note picks out that worker; the releaser has checked), while tb-reap's
//! automatic scan keeps treating it as ALIVE (its documented conservative exemption). A
//! headless holder WITH a running pid stays alive in both. Liveness always in fixture mode
//! (`TB_REAP_FAKE_*`): nothing here asks the real herdr, tmux or process table.
use std::path::PathBuf;
use std::process::{Command, Output};

/// An agent harness on record, with `TB_ROLE` set per test.
fn agent(role: &str) -> Vec<(&'static str, String)> {
    vec![("TB_HARNESS", "claude-code".into()), ("TB_MODEL", "m".into()), ("TB_SESSION", format!("s-{role}")), ("TB_ROLE", role.into())]
}

/// The launcher's headless log: a spawn header and, once the run ends, the exit trailer —
/// matching what tb-agent-start's HEADLESS_WRAP appends to the file the note's `log:` token
/// names.
fn headless_log_start(holder: &str, pid: i64) -> String {
    format!("[tb-agent-start] 2026-09-26 21:08:21 start {holder} pid={pid}: omp --approval-mode yolo --max-time 35m\n")
}

fn headless_log_exit(holder: &str) -> String {
    format!("[tb-agent-start] 2026-09-26 22:03:21 exit=0 {holder}\n")
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

    /// A `mode:headless` placement note from the holder.
    fn note(&self, id: &str, holder: &str, text: &str) {
        assert!(self.run(&agent("coder"), &[], holder, &["note", id, text]).status.success());
    }

    /// (column, owner, every event text joined).
    fn state(&self, id: &str) -> (String, String, String) {
        let o = self.run(&[], &[], "charles", &["show", id, "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        let c = if v.get("card").is_some() { &v["card"] } else { &v };
        let events = c["events"].as_array().map(|a| a.iter().map(|e| format!("{} {}", e["kind"].as_str().unwrap_or(""), e["text"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        (c["column"].as_str().unwrap_or("?").into(), c["owner"].as_str().unwrap_or("-").into(), events)
    }

    /// tb-reap `--dry-run --json` over this board: does it flag `id` as a dead owner?
    fn reap_dead(&self, id: &str) -> bool {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb-reap"));
        c.args(["--dry-run", "--json"]).env_clear();
        c.env("TB_DB", self.db())
            .env("HOME", self.dir.path().join("home"))
            .env("TB_REAP_STATE_DIR", self.dir.path().join("state"))
            .env("TB_REAP_KILLSWITCH", self.dir.path().join("home/no-such-switch"))
            .env("TB_REAP_DEAD_AFTER", "0")
            .env("USER", "login-user")
            .env("TZ", "UTC")
            .env("PATH", "/usr/bin:/bin")
            .env_remove("TB_BOARD")
            .env_remove("TB_AS")
            .env_remove("TB_REAP_MODE")
            .env_remove("TB_REAP_PROTECT")
            .env_remove("HERDR_AGENT_NAME");
        for v in ["TB_REAP_FAKE_AGENTS", "TB_REAP_FAKE_TMUX", "TB_REAP_FAKE_PANES", "TB_REAP_FAKE_SESSIONS", "TB_REAP_FAKE_PROCS"] {
            c.env(v, "");
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "tb-reap failed: {}", String::from_utf8_lossy(&o.stderr));
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        v["dead_owner_detail"].as_array().is_some_and(|a| {
            a.iter().any(|d| d["id"].as_i64() == id.parse::<i64>().ok())
        })
    }

    /// Assert a release SUCCEEDED and the card went to todo, unowned, holder named.
    fn assert_released(&self, id: &str, holder: &str) {
        let (col, owner, events) = self.state(id);
        assert_eq!((col.as_str(), owner.as_str()), ("todo", "-"));
        assert!(events.contains(&format!("released {holder}")), "names the old holder: {events}");
    }

    /// Assert the card stayed in DOING with its holder.
    fn assert_doing(&self, id: &str, holder: &str) {
        let (col, owner, _) = self.state(id);
        assert_eq!((col.as_str(), owner.as_str()), ("doing", holder));
    }

    /// A release attempt by lead `who` under the liveness fixture `fake`; returns the
    /// refusal's json code + text, asserting it WAS refused. `fake` MUST keep every
    /// `TB_REAP_FAKE_*` set (fixture mode) whenever the test means a probe to fire — an
    /// empty list runs the real probes, where no pid is running.
    fn refused_release(&self, who: &str, id: &str, reason: &str, fake: &[(&str, &str)]) -> (String, String) {
        let o = self.run(&agent("lead"), fake, who, &["release", id, reason, "--json"]);
        assert!(!o.status.success(), "tb release {id} by {who} was allowed: {}", String::from_utf8_lossy(&o.stdout));
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
        (v["code"].as_str().unwrap_or("").to_string(), format!("{} — {}", v["error"].as_str().unwrap_or(""), v["hint"].as_str().unwrap_or("")))
    }

    /// A release by lead `who` under the liveness fixture `fake`; asserts it SUCCEEDED.
    /// Same fixture rule as [`Board::refused_release`]: pass the `TB_REAP_FAKE_*` values
    /// the case needs (fixture mode is already on via `run`'s all-empty base).
    fn release_ok(&self, who: &str, id: &str, reason: &str, fake: &[(&str, &str)]) {
        let o = self.run(&agent("lead"), fake, who, &["release", id, reason, "--json"]);
        assert!(o.status.success(), "tb release {id} by {who} was refused: {}", String::from_utf8_lossy(&o.stderr));
    }
}

#[test]
fn release_a_no_pid_headless_holder_counts_it_dead_and_releases() {
    let b = Board::new();
    let id = b.held_by("hw");
    b.note(&id, "hw", "mode:headless placement (no pid known)");
    b.release_ok("lead-x", &id, "headless worker gone, no pid", &[]);
    b.assert_released(&id, "hw");
}

#[test]
fn release_a_headless_holder_whose_recorded_pid_runs_is_refused_holder_alive() {
    let b = Board::new();
    let id = b.held_by("hp");
    b.note(&id, "hp", "mode:headless pid=4242");
    let (code, text) =
        b.refused_release("lead-x", &id, "try", &[("TB_REAP_FAKE_PROCS", "4242 omp --worker")]);
    assert_eq!(code, "holder_alive", "{text}");
    assert!(text.contains("headless pid 4242"), "{text}");
    b.assert_doing(&id, "hp");
}

#[test]
fn reap_still_treats_a_no_pid_headless_holder_as_alive() {
    let b = Board::new();
    let id = b.held_by("hw");
    b.note(&id, "hw", "mode:headless placement (no pid known)");
    assert!(!b.reap_dead(&id), "tb-reap keeps the no-pid exemption: {id}");
}

/// The tmux SERVER line a headless launch leaves behind: started for the FIRST agent, its
/// cmdline keeps that agent's `-e TB_AS` and agent binary forever, and `bash -c` wraps the
/// agent command the server will run for any later session.
fn tmux_server(first: &str) -> String {
    format!(
        "3441 tmux new-session -d -s tbh-{first} -c /w -e TB_MODEL=glm-5.3-flash -e TB_ROLE=coder \
         -e TB_AS={first} -e TB_SESSION=s-{first} -e TB_CARD=tb#1 \
         bash -c wrap _ /tmp/{first}.log {first} omp --approval-mode yolo --max-time 35m"
    )
}

/// A tmux server (or client) process naming an agent must NOT vouch for it — it carries the
/// FIRST session's TB_AS forever and hosts other agents' sessions on the same server.
#[test]
fn a_tmux_server_process_does_not_vouch_for_the_agent_it_was_started_for() {
    let b = Board::new();
    let id = b.held_by("b-157");
    b.note(&id, "b-157", "mode:headless pid:910157 log:/tmp/b-157.log session:tbh-b-157");
    b.assert_doing(&id, "b-157");
    b.release_ok(
        "lead-x",
        &id,
        "b-157 finished; its tmux session is gone",
        &[("TB_REAP_FAKE_PROCS", &tmux_server("b-157"))],
    );
    b.assert_released(&id, "b-157");
}

/// A live agent process (omp with `TB_AS=<owner>`) still vouches, even with a tmux server
/// for another agent on the same fixture table.
#[test]
fn a_live_agent_process_still_vouches_next_to_a_tmux_server() {
    let b = Board::new();
    let id = b.held_by("b-100");
    b.note(&id, "b-100", "mode:headless pid:4100 log:/tmp/b-100.log session:tbh-b-100");
    let (code, text) = b.refused_release(
        "lead-x",
        &id,
        "should be refused",
        &[
            ("TB_REAP_FAKE_PROCS", &format!(
                "{};4100 bash -c wrap _ /tmp/b-100.log b-100 omp --approval-mode yolo TB_AS=b-100;\
                 4101 omp --approval-mode yolo TB_AS=b-100 TB_ROLE=coder",
                tmux_server("b-157")
            )),
            ("TB_REAP_FAKE_TMUX", "tbh-b-100"),
        ],
    );
    assert_eq!(code, "holder_alive", "{text}");
    assert!(
        text.contains("headless pid 4100") || text.contains("process 4100"),
        "{text}"
    );
    b.assert_doing(&id, "b-100");
}

/// Probe 5 marks a process as tmux ONLY by its own executable — argv0 basename = tmux. A
/// tmux word anywhere else (env TERM_PROGRAM=tmux, TERM=tmux-256color, a prompt that
/// mentions tmux) is not the process's executable and must change nothing. rv-lead-tb's
/// probe cases, one test each (rv/probe.sh: omp_plain, omp_in_tmux, claude_in_tmux,
/// omp_brief_says_tmux — every release refused, rc=1 col=doing). Each note carries NO pid
/// (Release mode treats a no-pid headless note as DEAD), so the FAKE_PROCS line is the
/// ONLY possible vouch — the assertion tests the process probe, not the note.
#[test]
fn omp_plain_still_vouches() {
    let b = Board::new();
    let id = b.held_by("b-500");
    b.note(&id, "b-500", "mode:headless log:/tmp/b-500.log");
    let (code, _) = b.refused_release(
        "lead-x",
        &id,
        "live omp with no tmux anywhere",
        &[("TB_REAP_FAKE_PROCS", "4501 omp -p --approval-mode yolo TB_AS=b-500 TERM=xterm-256color")],
    );
    assert_eq!(code, "holder_alive");
    b.assert_doing(&id, "b-500");
}

#[test]
fn omp_started_inside_tmux_still_vouches() {
    let b = Board::new();
    let id = b.held_by("b-501");
    b.note(&id, "b-501", "mode:headless log:/tmp/b-501.log");
    let (code, _) = b.refused_release(
        "lead-x",
        &id,
        "env TERM_PROGRAM=tmux is not an argv0",
        &[("TB_REAP_FAKE_PROCS", "4502 omp -p --approval-mode yolo TB_AS=b-501 TERM_PROGRAM=tmux TERM=tmux-256color")],
    );
    assert_eq!(code, "holder_alive");
    b.assert_doing(&id, "b-501");
}

#[test]
fn claude_started_inside_tmux_still_vouches() {
    let b = Board::new();
    let id = b.held_by("b-502");
    b.note(&id, "b-502", "mode:headless log:/tmp/b-502.log");
    let (code, _) = b.refused_release(
        "lead-x",
        &id,
        "claude under TERM_PROGRAM=tmux still vouches",
        &[("TB_REAP_FAKE_PROCS", "4503 /opt/homebrew/bin/claude --agent verify-lead TB_AS=b-502 TERM_PROGRAM=tmux")],
    );
    assert_eq!(code, "holder_alive");
    b.assert_doing(&id, "b-502");
}

#[test]
fn omp_prompt_mentioning_tmux_still_vouches() {
    let b = Board::new();
    let id = b.held_by("b-503");
    b.note(&id, "b-503", "mode:headless log:/tmp/b-503.log");
    let (code, _) = b.refused_release(
        "lead-x",
        &id,
        "prompt text naming tmux is not an argv0",
        &[("TB_REAP_FAKE_PROCS", "4504 omp -p You are b-503. Do not open tmux panes. TB_AS=b-503")],
    );
    assert_eq!(code, "holder_alive");
    b.assert_doing(&id, "b-503");
}

/// The omp SHARED worker broker line a worker-profile launch leaves behind: `omp
/// __omp_worker_daemon_broker` with ppid 1, inherited by every omp worker-profile session,
/// so its cmdline keeps the FIRST worker's `TB_AS` forever. argv[1] exactly
/// `__omp_worker_daemon_broker` — it vouches for nobody (same shape as the tmux-server
/// skip, default#998).
#[test]
fn the_shared_omp_worker_broker_does_not_vouch_for_the_first_worker_it_inherited() {
    let b = Board::new();
    let id = b.held_by("b-43r");
    b.note(&id, "b-43r", "mode:headless log:/tmp/b-43r.log");
    b.assert_doing(&id, "b-43r");
    b.release_ok(
        "lead-x",
        &id,
        "b-43r finished; only the shared broker still names it",
        &[(
            "TB_REAP_FAKE_PROCS",
            "18841 omp __omp_worker_daemon_broker \
             TB_AS=b-43r TB_SESSION=omp-b-43r-3d433aba TB_CARD=fleet#43",
        )],
    );
    b.assert_released(&id, "b-43r");
}

/// A real omp worker process naming the holder still vouches, even beside the shared
/// broker: the broker skip must not sweep a live worker away with it.
#[test]
fn a_live_omp_worker_still_vouches_beside_the_shared_broker() {
    let b = Board::new();
    let id = b.held_by("b-43r");
    b.note(&id, "b-43r", "mode:headless log:/tmp/b-43r.log");
    let (code, text) = b.refused_release(
        "lead-x",
        &id,
        "should be refused",
        &[(
            "TB_REAP_FAKE_PROCS",
            "18841 omp __omp_worker_daemon_broker \
             TB_AS=b-43r;22001 omp --approval-mode yolo --max-time 35m TB_AS=b-43r",
        )],
    );
    assert_eq!(code, "holder_alive", "{text}");
    assert!(text.contains("process 22001"), "{text}");
    b.assert_doing(&id, "b-43r");
}

/// Probe 5 skips ONLY argv[1] exactly `__omp_worker_daemon_broker`. A real worker whose
/// cmdline holds that string in a LATER arg (`omp -p grep __omp_worker_daemon_broker …`)
/// is the agent itself, not the broker, and still vouches (Dan: match exact, not contains).
#[test]
fn a_real_worker_with_the_broker_string_in_a_later_arg_still_vouches() {
    let b = Board::new();
    let id = b.held_by("b-43r");
    b.note(&id, "b-43r", "mode:headless log:/tmp/b-43r.log");
    let (code, _) = b.refused_release(
        "lead-x",
        &id,
        "worker grepping the broker string is not the broker",
        &[(
            "TB_REAP_FAKE_PROCS",
            "22002 omp -p grep __omp_worker_daemon_broker src TB_AS=b-43r",
        )],
    );
    assert_eq!(code, "holder_alive");
    b.assert_doing(&id, "b-43r");
}

/// The #243 repro: the run ended (`exit=0` trailer in the note's log) and the pid was
/// reused by an unrelated process — the pid must vouch for nobody and the release frees
/// the card.
#[test]
fn a_headless_pid_with_an_exit_trailer_and_a_reused_pid_stops_vouching() {
    let b = Board::new();
    let id = b.held_by("b-100");
    let log = b.dir.path().join("b-100.log");
    std::fs::write(
        &log,
        format!("{}{}", headless_log_start("b-100", 73509), headless_log_exit("b-100")),
    )
    .unwrap();
    b.note(
        &id,
        "b-100",
        &format!("mode:headless pid:73509 log:{} session:tbh-b-100", log.display()),
    );
    b.release_ok(
        "lead-x",
        &id,
        "b-100 finished; its pid is now an audio sandbox helper",
        &[(
            "TB_REAP_FAKE_PROCS",
            "73509 /System/Library/Frameworks/AudioToolbox.framework/XPCServices/com.apple.audio.SandboxHelper.xpc/Contents/MacOS/com.apple.audio.SandboxHelper",
        )],
    );
    b.assert_released(&id, "b-100");
}

/// No log file at all — but the pid now runs something that names neither an agent binary
/// nor the holder: a reused pid is not the agent and stops vouching.
#[test]
fn a_reused_pid_whose_process_names_no_agent_or_holder_stops_vouching() {
    let b = Board::new();
    let id = b.held_by("b-100");
    b.note(&id, "b-100", "mode:headless pid:73509 log:/nonexistent/b-100.log session:tbh-b-100");
    b.release_ok(
        "lead-x",
        &id,
        "pid 73509 is not an agent any more",
        &[("TB_REAP_FAKE_PROCS", "73509 com.apple.audio.SandboxHelper")],
    );
    b.assert_released(&id, "b-100");
}

/// A LIVE headless agent: the log has no exit trailer and the pid still runs the wrapper
/// (holder's name + omp in its command line) — the release stays refused.
#[test]
fn a_live_headless_wrapper_with_the_holder_and_no_trailer_still_vouches() {
    let b = Board::new();
    let id = b.held_by("b-100");
    let log = b.dir.path().join("b-100-live.log");
    std::fs::write(&log, headless_log_start("b-100", 73509)).unwrap();
    b.note(
        &id,
        "b-100",
        &format!("mode:headless pid:73509 log:{} session:tbh-b-100", log.display()),
    );
    let (code, text) = b.refused_release(
        "lead-x",
        &id,
        "should be refused",
        &[(
            "TB_REAP_FAKE_PROCS",
            "73509 bash -c wrap _ /tmp/b-100.log b-100 omp --approval-mode yolo --max-time 35m",
        )],
    );
    assert_eq!(code, "holder_alive", "{text}");
    assert!(text.contains("headless pid 73509"), "{text}");
    b.assert_doing(&id, "b-100");
}

/// The default#993 repro: a lead process whose TB_ROLE token names the holder
/// `orchestrator` while running as lead-legal-mcp — the role token is not the process's
/// identity, so the process probe must not vouch and the release frees the card.
#[test]
fn a_process_naming_the_holder_only_as_its_role_token_does_not_vouch() {
    let b = Board::new();
    let id = b.held_by("orchestrator");
    b.release_ok(
        "lead-x",
        &id,
        "the role token is not the identity (default#993)",
        &[("TB_REAP_FAKE_PROCS", "76259 claude --agent lead TB_AS=lead-legal-mcp TB_ROLE=orchestrator")],
    );
    b.assert_released(&id, "orchestrator");
}

/// A holder's name only as prompt text inside an agent process running as someone else is
/// not its identity either.
#[test]
fn a_holder_named_only_in_prompt_text_of_another_agents_process_does_not_vouch() {
    let b = Board::new();
    let id = b.held_by("orchestrator");
    b.release_ok(
        "lead-x",
        &id,
        "prompt text is not the identity",
        &[("TB_REAP_FAKE_PROCS", "76259 claude -p summarize the orchestrator notes TB_AS=lead-x")],
    );
    b.assert_released(&id, "orchestrator");
}
