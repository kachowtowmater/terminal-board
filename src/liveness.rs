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
//!      argv or environment names the owner as a whole token (`TB_AS=<owner>`, `--as <owner>`).
//!      A tmux process is NEVER this probe's proof — the SERVER spawned by the first headless
//!      `tmux new-session -s tbh-<owner> -e TB_AS=<owner> …` (and any tmux client) keeps that
//!      first session's `-e TB_AS` and agent binary in its cmdline long after that session is
//!      gone and other agents run on the same server — so a process whose argv0 basename is
//!      `tmux` is skipped, however agent-shaped its arguments look;
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
//! `;`-separated. A PROCS line whose argv0 is `tmux` is skipped by the process probe (see
//! probe 5), so a test can pin a tmux server line and assert it vouches for nobody. Any one
//! of them set (even to "") puts EVERY probe in fixture mode: a test must never half-ask the
//! real world. Fixture PROCS are NEVER filtered by ancestry — the exclusion is about the
//! caller's own process tree, which a fixture table does not contain.
//!
//! The five probe sources are asked ONCE per run (`World::load`) and the answer reused for
//! every card: a release pass over a board must not re-ask herdr, tmux and the process table
//! per card.

use crate::herdr::{self, Agent, AgentsState};
use crate::store::{actors, Card, Store};
use std::collections::HashMap;
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
/// which treats "" as unset) so a test can assert "nothing is alive". In fixture mode the
/// REAL process table is never read: a test owns every probe, and the box's live processes
/// (test runners, ssh, other agents' sessions) must never vouch for a name a fixture meant
/// to be dead (#235).
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
    // Linux procps has no -E; fall back to argv alone. The ppid column rides along on both:
    // the probe never counts the caller's own process tree (exclude_ancestry), and a row's
    // ppid is what the walk needs, so one ps answers the whole probe.
    let tries: [&[&str]; 2] = [&["-E", "-ww", "-A", "-o", "pid=,ppid=,command="], &["-ww", "-A", "-o", "pid=,ppid=,command="]];
    for args in tries {
        if let Ok(o) = Command::new("ps").args(args).output() {
            if o.status.success() {
                let (mut procs, ppids) = parse_procs_with_ppids(&String::from_utf8_lossy(&o.stdout), '\n');
                exclude_ancestry(&mut procs, &ppids);
                return procs;
            }
        }
    }
    Vec::new()
}

/// The caller's own pid and every ppid-chain ancestor, walked through the snapshot's ppid
/// column. The probe must never count tb's OWN process — its argv is e.g.
/// `tb release 140 "… b-140 …"`, which names the holder — or the agent shell that launched
/// it, whose argv may name a holder the shell merely typed. The chain stops at the kernel
/// pair, a repeat or an ancestor the snapshot lost — never an error: dropping what it proved
/// is safe, and one missed ancestor only weakens the exclusion, never adds a vouch.
/// Fixture mode never calls this: a fixture table does not contain the caller's tree.
fn exclude_ancestry(procs: &mut Vec<(i64, String)>, ppids: &HashMap<i64, i64>) {
    let own = std::process::id() as i64;
    let mut pids = vec![own];
    let mut cur = own;
    for _ in 0..crate::proc::MAX_DEPTH {
        match ppids.get(&cur).copied() {
            Some(p) if p > 1 && !pids.contains(&p) => {
                pids.push(p);
                cur = p;
            }
            _ => break,
        }
    }
    procs.retain(|(p, _)| !pids.contains(p));
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

/// The real `ps` snapshot, with each row's ppid: `(pid, command line + environment)` rows
/// for the probe, and a `pid -> ppid` map for the own-tree walk. One column further than
/// `parse_procs`; the fixture parser stays the 2-column shape its env var documents.
fn parse_procs_with_ppids(text: &str, sep: char) -> (Vec<(i64, String)>, HashMap<i64, i64>) {
    let mut procs = Vec::new();
    let mut ppids = HashMap::new();
    for l in text.split(sep) {
        let l = l.trim();
        let Some((pid, rest)) = l.split_once(char::is_whitespace) else { continue };
        let Ok(pid) = pid.parse::<i64>() else { continue };
        let rest = rest.trim();
        match rest.split_once(char::is_whitespace) {
            // `pid ppid command…` — the ppid is a bare integer only when a command follows
            Some((ppid, cmd)) if ppid.chars().all(|c| c.is_ascii_digit()) && !ppid.is_empty() => {
                ppids.insert(pid, ppid.parse::<i64>().unwrap_or(0));
                procs.push((pid, cmd.trim().to_string()));
            }
            // no ppid on the row (a ps without the column): keep the row, leave the map empty
            _ => procs.push((pid, rest.to_string())),
        }
    }
    (procs, ppids)
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
            // A tmux process (the SERVER spawned by the first headless `tmux new-session`,
            // and any tmux CLIENT) is never the agent itself: its cmdline keeps the FIRST
            // session's `-e TB_AS=<owner>` and agent binary forever, so it names an owner
            // it merely hosts. ONLY argv0 basename = tmux marks it — a tmux word anywhere
            // else (env TERM_PROGRAM=tmux, TERM=tmux-256color, prompt text) is not the
            // process's executable and changes nothing.
            let argv0_agentish = toks.iter().any(|t| {
                let base = t.rsplit('/').next().unwrap_or(t);
                AGENT_BINS.iter().any(|b| base.eq_ignore_ascii_case(b))
            });
            let is_tmux = toks
                .first()
                .and_then(|t| t.rsplit('/').next())
                .is_some_and(|b| b.eq_ignore_ascii_case("tmux"));
            argv0_agentish && !is_tmux && toks.iter().any(|t| eq_ci(t, o))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// #227: the walk excludes tb's own pid and every snapshot-resolvable ancestor, and
    /// stops at pid 1 / a repeat / a missing row. `std::process::id()` is this process, so
    /// the real row (if the test host's ps has it) is dropped too — in a no-ps environment
    /// the map is empty and only `own` is excluded, which is still correct.
    #[test]
    fn exclude_ancestry_drops_the_own_ppid_chain() {
        let own = std::process::id() as i64;
        let parent = 100 + own % 10; // an arbitrary, definitely-unrelated ppid for the row
        let grand = 200 + own % 10;
        let mut procs = vec![
            (own, "tb release 1 omp b-140 finished".into()),
            (parent, "omp -c 'tb release 3 b-141' b-141".into()),
            (grand, "bash".into()),
            (7, "omp --as b-8".into()), // unrelated live agent: must survive
        ];
        let mut ppids = HashMap::new();
        ppids.insert(own, parent);
        ppids.insert(parent, grand);
        ppids.insert(grand, 1); // init: the chain stops here
        exclude_ancestry(&mut procs, &ppids);
        assert_eq!(procs, vec![(7, "omp --as b-8".to_string())], "own tree gone, unrelated live agent kept");
    }

    #[test]
    fn exclude_ancestry_survives_a_cycle_and_a_missing_row() {
        let own = std::process::id() as i64;
        let mut procs = vec![(own, "tb release 1 x".into()), (9, "omp --as b-8".into())];
        let mut ppids = HashMap::new();
        ppids.insert(own, 5); // 5 is not on the table: the chain ends after this hop
        ppids.insert(5, own); // would loop back to own
        exclude_ancestry(&mut procs, &ppids);
        assert_eq!(procs, vec![(9, "omp --as b-8".to_string())]);
    }

    /// A snapshot row without a ppid (an older ps) still parses; the walk then has no hops
    /// and only `own` is dropped.
    #[test]
    fn parse_procs_with_ppids_tolerates_a_ppidless_row() {
        let (procs, ppids) = parse_procs_with_ppids("  4242 omp --as b-8\n  77 bash\n", '\n');
        assert_eq!(procs.len(), 2, "{procs:?}");
        assert!(ppids.is_empty(), "no ppid column: {ppids:?}");
        // with a ppid column, the map is filled and the command keeps its words
        let (procs, ppids) = parse_procs_with_ppids("  4242  77  omp --as b-8\n", '\n');
        assert_eq!(procs, vec![(4242, "omp --as b-8".to_string())], "{procs:?}");
        assert_eq!(ppids.get(&4242), Some(&77));
    }

    /// A command that itself begins with digits must not read as a ppid (`ps -o pid=,ppid=`
    /// always prints the ppid, but the tolerant branch must not swallow command text).
    #[test]
    fn parse_procs_with_ppids_keeps_a_command_that_looks_numeric() {
        let (procs, ppids) = parse_procs_with_ppids("  4242 77 omp 4242 --as b-8\n", '\n');
        assert_eq!(ppids.get(&4242), Some(&77));
        assert_eq!(procs, vec![(4242, "omp 4242 --as b-8".to_string())], "{procs:?}");
    }
}
