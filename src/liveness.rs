//! Liveness — the probe set that decides whether a card's owner is still there. Shared by
//! `tb-reap` (release a DOING card stuck with a dead owner) and `tb release` (a lead or
//! orchestrator frees the same card by hand, refused while the owner is alive), so both
//! answer "is this holder alive?" with the SAME evidence, never two diverging copies.
//!
//! An owner is ALIVE when ANY of these vouches for it, and only an owner none of them
//! vouches for is dead:
//!   1. the orchestrator: an `orch` / `orch-*` name, a name in `TB_REAP_PROTECT`, or the
//!      identity the caller itself runs as (`TB_AS`);
//!   2. a herdr agent with that exact name;
//!   3. a tmux session with that name;
//!   4. a herdr pane (plain panes included) whose LABEL names the owner as a whole token;
//!   5. a running agent process (codex / omp / claude / pi / aider / opencode / gemini) whose
//!      argv or environment names the owner as a whole token (`TB_AS=<owner>`, `--as <owner>`);
//!   6. one of the owner's tb actor sessions on this card is a live herdr agent's session
//!      (an orchestrator or headless builder acting under the name);
//!   7. `mode:headless`: with `pid=N` in the note, alive while pid N runs; with no pid,
//!      conservatively alive — for `tb-reap`. An explicit `tb release` treats a no-pid
//!      headless holder as DEAD (nothing but the note records it; the releaser has checked).
//!      Both keep a no-pid holder alive by every other probe.
//!
//! Fixtures: setting any `TB_REAP_FAKE_{AGENTS,TMUX,PANES,SESSIONS,PROCS}` switches every
//! probe to fixture mode (unset ones are empty) so a test never asks the real world. AGENTS,
//! TMUX, SESSIONS are comma lists; PANES (labels) and PROCS (`<pid> <command line>`) are
//! `;`-separated. Any one of them set (even to "") puts EVERY probe in fixture mode: a test
//! must never half-ask the real world.
//!
//! The five probe sources are asked ONCE per run (`World::load`) and the answer reused for
//! every card: a release pass over a board must not re-ask herdr, tmux and the process table
//! per card.

use crate::herdr::{self, Agent, AgentsState};
use crate::store::{actors, Card, Store};
use std::process::Command;

/// Agent binaries whose running process vouches for a name (probe 5). Not every process —
/// a `vim codex-u5.txt` is not codex-u5 working.
const AGENT_BINS: &[&str] = &["codex", "omp", "claude", "pi", "aider", "opencode", "gemini"];

const FAKE_VARS: &[&str] =
    &["TB_REAP_FAKE_AGENTS", "TB_REAP_FAKE_TMUX", "TB_REAP_FAKE_PANES", "TB_REAP_FAKE_SESSIONS", "TB_REAP_FAKE_PROCS"];

/// Which caller is asking [`World::alive_by`]: the automatic `tb-reap` scan, or an explicit
/// `tb release`. They share every probe; only a no-pid `mode:headless` note differs between
/// them (see [`World::alive_by`]).
pub enum Mode {
    Reap,
    Release,
}

/// Is any `TB_REAP_FAKE_*` set (even to "")? `std::env::var` directly (not `crate::env`,
/// which treats "" as unset) so a test can assert "nothing is alive".
pub fn fixture_mode() -> bool {
    FAKE_VARS.iter().any(|v| std::env::var(v).is_ok())
}

fn fake_split(var: &str, sep: char) -> Vec<String> {
    std::env::var(var)
        .map(|v| v.split(sep).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

/// Case-insensitive, trimmed, both-non-empty equality — how every liveness name is compared.
pub fn eq_ci(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// Whole-token match: `text` split on anything that cannot be part of an agent name
/// (letters, digits, `-`, `_`, `.`) holds `name`. `codex-u5` is in `worker · codex-u5 · #5`
/// but not in `codex-u55`.
fn names_token(text: &str, name: &str) -> bool {
    text.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.')).any(|t| eq_ci(t, name))
}

/// Real or fixture herdr agents, asked once per run.
pub enum Agents {
    Fake(Vec<String>),
    Real(Vec<Agent>),
    Unavailable,
}

/// Everything liveness is judged against, probed once per run.
pub struct World {
    agents: Agents,
    tmux: Vec<String>,
    pane_labels: Vec<String>,
    /// Session tokens of live herdr agents (same reduction tb applies when it records one).
    sessions: Vec<String>,
    /// `(pid, command line + environment where the OS shows it)`.
    procs: Vec<(i64, String)>,
    protect: Vec<String>,
    me: Option<String>,
}

fn herdr_json(args: &[&str]) -> Option<serde_json::Value> {
    herdr::run(args).and_then(|s| serde_json::from_str(&s).ok())
}

fn load_procs() -> Vec<(i64, String)> {
    // macOS: -E appends each process's environment (own user only) — where TB_AS lives.
    // Linux procps has no -E; fall back to argv alone.
    let tries: [&[&str]; 2] = [&["-E", "-ww", "-A", "-o", "pid=,command="], &["-ww", "-A", "-o", "pid=,command="]];
    for args in tries {
        if let Ok(o) = Command::new("ps").args(args).output() {
            if o.status.success() {
                return parse_procs(&String::from_utf8_lossy(&o.stdout), '\n');
            }
        }
    }
    Vec::new()
}

fn parse_procs(text: &str, sep: char) -> Vec<(i64, String)> {
    text.split(sep)
        .filter_map(|l| {
            let l = l.trim();
            let (pid, rest) = l.split_once(char::is_whitespace)?;
            Some((pid.parse().ok()?, rest.trim().to_string()))
        })
        .collect()
}

impl World {
    pub fn load() -> World {
        let protect = std::env::var("TB_REAP_PROTECT")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        let me = std::env::var("TB_AS").ok().filter(|s| !s.trim().is_empty());
        if fixture_mode() {
            let procs = std::env::var("TB_REAP_FAKE_PROCS").map(|v| parse_procs(&v, ';')).unwrap_or_default();
            return World {
                agents: Agents::Fake(fake_split("TB_REAP_FAKE_AGENTS", ',')),
                tmux: fake_split("TB_REAP_FAKE_TMUX", ','),
                pane_labels: fake_split("TB_REAP_FAKE_PANES", ';'),
                sessions: fake_split("TB_REAP_FAKE_SESSIONS", ','),
                procs,
                protect,
                me,
            };
        }
        let agents = match herdr::probe() {
            AgentsState::Agents(a) => Agents::Real(a),
            _ => Agents::Unavailable,
        };
        let (mut pane_labels, mut sessions) = (Vec::new(), Vec::new());
        if herdr::herdr_enabled() {
            if let Some(v) = herdr_json(&["pane", "list"]) {
                for p in v.pointer("/result/panes").and_then(|a| a.as_array()).into_iter().flatten() {
                    if let Some(l) = p.get("label").and_then(|l| l.as_str()) {
                        pane_labels.push(l.to_string());
                    }
                }
            }
            if let Some(v) = herdr_json(&["agent", "list"]) {
                for a in v.pointer("/result/agents").and_then(|a| a.as_array()).into_iter().flatten() {
                    if let Some(s) = a.pointer("/agent_session/value").and_then(|s| s.as_str()) {
                        sessions.extend(actors::session_token(s));
                    }
                }
            }
        }
        let tmux = match Command::new("tmux").args(["list-sessions", "-F", "#{session_name}"]).output() {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect(),
            _ => Vec::new(),
        };
        World { agents, tmux, pane_labels, sessions, procs: load_procs(), protect, me }
    }

    pub fn pid_alive(&self, pid: i64) -> bool {
        self.procs.iter().any(|(p, _)| *p == pid)
    }

    /// The live herdr agent behind this card's owner (for the idle report), when one exists.
    /// `Some(None)`: fixture mode says the name IS a live agent (no status to report);
    /// `None`: not even fixture mode says so.
    pub fn agent<'a>(&'a self, card: &Card) -> Option<Option<&'a Agent>> {
        let owner = card.owner.as_deref()?;
        match &self.agents {
            Agents::Fake(names) => names.iter().any(|n| eq_ci(n, owner)).then_some(None),
            Agents::Real(a) => herdr::exact_owner(a, card).map(Some),
            Agents::Unavailable => None,
        }
    }

    /// Why `owner` of card `id` counts as alive, or None when nothing vouches for it. The
    /// wording is the evidence a refusal or a release note quotes: keep it naming the SOURCE
    /// (herdr-agent, tmux, pane label, process N, actor-session, headless pid N,
    /// orchestrator, reaper-caller), not just "alive".
    pub fn alive_by(&self, store: &Store, card: &Card) -> Option<String> {
        self.alive_impl(store, card, Mode::Reap)
    }

    /// The one liveness question an explicit `tb release` asks. Same probes, same wording —
    /// but a no-pid `mode:headless` note does NOT vouch for the holder: nothing but that
    /// note records the worker, and the releaser (a lead/orchestrator/person acting with a
    /// reason) has already checked the holder is gone. tb-reap keeps its conservative
    /// exemption ([`World::alive_by`]).
    pub fn alive_for_release(&self, store: &Store, card: &Card) -> Option<String> {
        self.alive_impl(store, card, Mode::Release)
    }

    /// Shared body of [`World::alive_by`] and [`World::alive_for_release`]; `mode` decides
    /// only the no-pid-headless verdict.
    fn alive_impl(&self, store: &Store, card: &Card, mode: Mode) -> Option<String> {
        let owner = card.owner.as_deref()?;
        let o = owner.trim();
        if eq_ci(o, "orch") || o.to_ascii_lowercase().starts_with("orch-") || self.protect.iter().any(|p| eq_ci(p, o)) {
            return Some("orchestrator".into());
        }
        if self.me.as_deref().is_some_and(|m| eq_ci(m, o)) {
            return Some("reaper-caller".into());
        }
        if self.agent(card).is_some() {
            return Some("herdr-agent".into());
        }
        if self.tmux.iter().any(|n| eq_ci(n, o)) {
            return Some("tmux".into());
        }
        if self.pane_labels.iter().any(|l| names_token(l, o)) {
            return Some("pane-label".into());
        }
        if let Some((pid, _)) = self.procs.iter().find(|(_, cmd)| {
            let toks: Vec<&str> = cmd.split(|c: char| c.is_whitespace() || c == '=').collect();
            let agentish = toks.iter().any(|t| {
                let base = t.rsplit('/').next().unwrap_or(t);
                AGENT_BINS.iter().any(|b| base.eq_ignore_ascii_case(b))
            });
            agentish && toks.iter().any(|t| eq_ci(t, o))
        }) {
            return Some(format!("process {pid}"));
        }
        let detail = store.show(card.id).ok()?;
        if detail
            .actors
            .iter()
            .any(|a| eq_ci(&a.actor, o) && a.session.as_deref().is_some_and(|s| self.sessions.iter().any(|l| l == s)))
        {
            return Some("actor-session".into());
        }
        // mode:headless — the newest such note decides; its pid, when it names one. With a
        // pid, both callers ask the process table: alive while it runs, dead once it is
        // gone. With NO pid, tb-reap stays conservatively alive (its documented exemption —
        // a reaped headless worker can't fight back), but an explicit `tb release` by a
        // lead/orchestrator/person who has already checked the holder is gone treats it as
        // dead: nothing records that worker but this note.
        if let Some(note) = detail.events.iter().rev().find(|e| e.kind == "note" && e.text.contains("mode:headless")) {
            return match headless_pid(&note.text) {
                Some(pid) if self.pid_alive(pid) => Some(format!("headless pid {pid}")),
                Some(_) => None,
                None => match mode {
                    Mode::Reap => Some("headless (no pid recorded)".into()),
                    Mode::Release => None,
                },
            };
        }
        None
    }
}

/// `pid=123`, `pid:123` or `pid 123` in a `mode:headless` note.
fn headless_pid(text: &str) -> Option<i64> {
    let lower = text.to_ascii_lowercase();
    let i = lower.find("pid")?;
    let rest = lower[i + 3..].trim_start_matches(|c: char| c == '=' || c == ':' || c.is_whitespace());
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}
