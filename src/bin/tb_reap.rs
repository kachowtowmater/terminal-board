//! `tb-reap` — card-driven pane lifecycle (orch-restructure W-3, tb orch #28, U5/R4).
//!
//! State machine (see the gate at `~/Vault/openos/plans/orch-restructure/gates/W-3.sh`):
//!   - card DONE / dropped to TODO -> a live caller (herdr/pane owner) closes its pane; this
//!     binary only reports which cards left DOING, it never touches panes/tmux itself.
//!   - card REVIEW -> never touched (the builder may still be asked to fix it).
//!   - a DOING card whose owner is not ALIVE by any liveness source below for
//!     `TB_REAP_DEAD_AFTER` seconds (default 1800 = 30m, measured from the card's time in
//!     DOING — the only clock tb has) -> released to TODO with a note, via a direct library
//!     call (never the CLI's `--force` flag).
//!   - "idle" = a LIVE owner whose herdr agent status is `idle`/`done` (no CPU, no fresh pane
//!     output — herdr's own signal, `Agent::is_idle`) while the card is still in DOING.
//!     Idle agents are REPORTED only, never released — a long *working* session sitting in
//!     DOING for hours is not idle, and must never be flagged just for its age.
//!   - a card carrying a `mode:headless` note is never released for lacking a pane
//!     (headless workers have none by design); its note's `pid=N`, when present, decides.
//!
//! Liveness (v2, after the 2026-09-24 05:00 `--apply` run released four live owners' cards —
//! tb tb #131): an owner is ALIVE when ANY of these holds, and only an owner none of them
//! vouches for is a dead owner:
//!   1. the orchestrator: an `orch` / `orch-*` name, a name in `TB_REAP_PROTECT`, or the
//!      identity the reaper itself runs as (`TB_AS`);
//!   2. a herdr agent with that exact name;
//!   3. a tmux session with that name;
//!   4. a herdr pane (plain panes included) whose LABEL names the owner as a whole token;
//!   5. a running agent process (codex / omp / claude / pi / aider / opencode / gemini) whose
//!      argv or environment names the owner as a whole token (`TB_AS=<owner>`, `--as <owner>`);
//!   6. one of the owner's tb actor sessions on this card is a live herdr agent's session
//!      (an orchestrator or headless builder acting under the name);
//!   7. `mode:headless`: with `pid=N` in the note, alive while pid N runs; with no pid,
//!      conservatively alive.
//!
//! Modes: `--dry-run` (report only, never writes; also appends a dated line carrying
//! `"liveness":2` to `<state>/dry-run.log`, the 7-day proof) and `--apply`. Neither flag
//! defaults to `--dry-run`. `--apply` is REFUSED (exit 1, `{"refused":true,...}`) unless
//! `TB_REAP_MODE=live` AND the dry-run log holds liveness-v2 entries spanning >= 7 days, the
//! newest < 24 h old, AND 0 false positives: no card the dry run flagged was later written to
//! by its owner, and `<state>/false-positives.log` (a human-kept ledger) has no entries.
//! `<state>` = `TB_REAP_STATE_DIR`, else `~/.local/state/tb-reap`. A kill-switch file
//! (`TB_REAP_KILLSWITCH`, else `~/.config/tb/reap.disabled`) short-circuits everything to
//! `{"disabled":true}` and exit 0.
//!
//! Fixtures: setting any `TB_REAP_FAKE_{AGENTS,TMUX,PANES,SESSIONS,PROCS}` switches every
//! probe to fixture mode (unset ones are empty) so a test never asks the real world. AGENTS,
//! TMUX, SESSIONS are comma lists; PANES (labels) and PROCS (`<pid> <command line>`) are
//! `;`-separated.

use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use terminal_board::boards;
use terminal_board::herdr::{self, Agent, AgentsState};
use terminal_board::store::{actors, Card, Store};

/// Bumped when the liveness rules change: only dry-run lines written under the current rules
/// count toward the 7-day proof `--apply` needs.
const LIVENESS_VERSION: i64 = 2;
const APPLY_MIN_DAYS: i64 = 7;
const AGENT_BINS: &[&str] = &["codex", "omp", "claude", "pi", "aider", "opencode", "gemini"];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    DryRun,
    Apply,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn killswitch_path() -> PathBuf {
    match std::env::var("TB_REAP_KILLSWITCH") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => home().join(".config/tb/reap.disabled"),
    }
}

fn state_dir() -> PathBuf {
    match std::env::var("TB_REAP_STATE_DIR") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => home().join(".local/state/tb-reap"),
    }
}

fn dry_run_log_path() -> PathBuf {
    state_dir().join("dry-run.log")
}

fn false_positive_ledger_path() -> PathBuf {
    state_dir().join("false-positives.log")
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn dead_after() -> i64 {
    std::env::var("TB_REAP_DEAD_AFTER").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(1800)
}

const FAKE_VARS: &[&str] =
    &["TB_REAP_FAKE_AGENTS", "TB_REAP_FAKE_TMUX", "TB_REAP_FAKE_PANES", "TB_REAP_FAKE_SESSIONS", "TB_REAP_FAKE_PROCS"];

/// Any `TB_REAP_FAKE_*` set (even to "") puts EVERY probe in fixture mode: a test must never
/// half-ask the real world. `std::env::var` directly (not `terminal_board::env`, which treats
/// "" as unset) so a test can assert "nothing is alive".
fn fixture_mode() -> bool {
    FAKE_VARS.iter().any(|v| std::env::var(v).is_ok())
}

fn fake_split(var: &str, sep: char) -> Vec<String> {
    std::env::var(var)
        .map(|v| v.split(sep).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

fn eq_ci(a: &str, b: &str) -> bool {
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
enum Agents {
    Fake(Vec<String>),
    Real(Vec<Agent>),
    Unavailable,
}

/// Everything liveness is judged against, probed once per run.
struct World {
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
    fn load() -> World {
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

    fn pid_alive(&self, pid: i64) -> bool {
        self.procs.iter().any(|(p, _)| *p == pid)
    }

    /// The live herdr agent behind this card's owner (for the idle report), when one exists.
    fn agent<'a>(&'a self, card: &Card) -> Option<Option<&'a Agent>> {
        let owner = card.owner.as_deref()?;
        match &self.agents {
            Agents::Fake(names) => names.iter().any(|n| eq_ci(n, owner)).then_some(None),
            Agents::Real(a) => herdr::exact_owner(a, card).map(Some),
            Agents::Unavailable => None,
        }
    }

    /// Why `owner` of card `id` counts as alive, or None when nothing vouches for it.
    fn alive_by(&self, store: &Store, card: &Card) -> Option<String> {
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
        // mode:headless — the newest such note decides; its pid, when it names one.
        if let Some(note) = detail.events.iter().rev().find(|e| e.kind == "note" && e.text.contains("mode:headless")) {
            return match headless_pid(&note.text) {
                Some(pid) if self.pid_alive(pid) => Some(format!("headless pid {pid}")),
                Some(_) => None,
                None => Some("headless (no pid recorded)".into()),
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

struct BoardResult {
    board: String,
    dead_owner_cards: Vec<(i64, Option<String>)>,
    idle_cards: Vec<i64>,
    alive: Vec<(i64, String, String)>,
    released: Vec<i64>,
}

fn boards_to_scan() -> Vec<String> {
    if terminal_board::env("DB").is_some() {
        // TB_DB pins one file — there is exactly one board to look at, whatever it's named.
        return vec![boards::default_name()];
    }
    if let Some(b) = terminal_board::env("BOARD") {
        return vec![b];
    }
    let all = boards::list();
    if all.is_empty() { vec![boards::default_name()] } else { all }
}

fn open_board(name: &str) -> Option<Store> {
    Some(Store::open_if_exists(&boards::path_for(name)).ok()??.named(name))
}

fn scan_board(name: &str, mode: Mode, threshold: i64, world: &World) -> Option<BoardResult> {
    let store = open_board(name)?;
    let cards = store.list().ok()?;
    let mut r = BoardResult {
        board: name.to_string(),
        dead_owner_cards: Vec::new(),
        idle_cards: Vec::new(),
        alive: Vec::new(),
        released: Vec::new(),
    };
    let t = now();
    for c in cards.iter().filter(|c| c.column == "doing") {
        match world.alive_by(&store, c) {
            None => {
                if t - c.column_since >= threshold {
                    r.dead_owner_cards.push((c.id, c.owner.clone()));
                }
            }
            Some(by) => {
                // Idle only when herdr's own status says so — never inferred from card age.
                if matches!(world.agent(c), Some(Some(a)) if a.is_idle()) {
                    r.idle_cards.push(c.id);
                }
                r.alive.push((c.id, c.owner.clone().unwrap_or_default(), by));
            }
        }
    }
    if mode == Mode::Apply {
        let mut store = store;
        for (id, owner) in &r.dead_owner_cards {
            let owner = owner.as_deref().unwrap_or("unknown");
            let reason = format!(
                "tb-reap: released — owner '{owner}' not alive by any liveness source (herdr agent, tmux, pane label, agent process, actor session, headless pid, orchestrator) for >= {threshold}s"
            );
            // force=true here is a direct library call, never the CLI's `--force` flag: the
            // reaper is the one caller allowed to release a dead owner's card without asking.
            // `move_opts`'s `reason` is reserved for a review->doing send-back, so the release
            // note goes through `note`, after the move succeeds.
            if store.move_opts(*id, "todo", "tb-reap", true, None).is_ok() {
                let _ = store.note(*id, &reason, "tb-reap");
                r.released.push(*id);
            }
        }
    }
    Some(r)
}

fn append_dry_run_log(summary: &serde_json::Value) {
    let path = dry_run_log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let line = format!("{ts} {summary}\n");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Why `--apply` may not run now (empty = allowed), and the false positives found.
fn apply_refusals() -> (Vec<String>, Vec<serde_json::Value>) {
    let mut reasons = Vec::new();
    let mut fps = Vec::new();
    let mode = std::env::var("TB_REAP_MODE").unwrap_or_default();
    if mode != "live" {
        reasons.push(format!("TB_REAP_MODE is '{mode}', not 'live'"));
    }
    // liveness-v2 dry-run lines: (unix ts, json)
    let text = std::fs::read_to_string(dry_run_log_path()).unwrap_or_default();
    let lines: Vec<(i64, serde_json::Value)> = text
        .lines()
        .filter_map(|l| {
            let (ts, js) = l.split_once(' ')?;
            let ts = chrono::DateTime::parse_from_rfc3339(ts).ok()?.timestamp();
            let v: serde_json::Value = serde_json::from_str(js).ok()?;
            (v["liveness"].as_i64().unwrap_or(0) >= LIVENESS_VERSION).then_some((ts, v))
        })
        .collect();
    let t = now();
    match (lines.iter().map(|l| l.0).min(), lines.iter().map(|l| l.0).max()) {
        (Some(first), Some(last)) => {
            let days = (t - first) / 86400;
            if days < APPLY_MIN_DAYS {
                reasons.push(format!("dry-run log (liveness v{LIVENESS_VERSION}) spans {days}d, needs >= {APPLY_MIN_DAYS}d"));
            }
            if t - last > 86400 {
                reasons.push("dry-run log is stale: newest entry is over 24h old".into());
            }
        }
        _ => reasons.push(format!(
            "no liveness-v{LIVENESS_VERSION} dry-run log at {} — run --dry-run for {APPLY_MIN_DAYS} days first",
            dry_run_log_path().display()
        )),
    }
    // False positive = a card the dry run flagged whose owner wrote to it afterwards.
    let mut first_flag: std::collections::BTreeMap<(String, i64, String), i64> = Default::default();
    for (ts, v) in &lines {
        for d in v["dead_owner_detail"].as_array().into_iter().flatten() {
            let (Some(b), Some(id), Some(o)) = (d["board"].as_str(), d["id"].as_i64(), d["owner"].as_str()) else { continue };
            first_flag.entry((b.to_string(), id, o.to_string())).and_modify(|e| *e = (*e).min(*ts)).or_insert(*ts);
        }
    }
    for ((b, id, o), ts) in &first_flag {
        let Some(store) = open_board(b) else { continue };
        let Ok(detail) = store.show(*id) else { continue };
        if let Some(e) = detail.events.iter().find(|e| e.ts > *ts && eq_ci(&e.actor, o)) {
            fps.push(json!({"board": b, "id": id, "owner": o, "flagged_at": ts, "owner_event": e.kind, "owner_event_at": e.ts}));
        }
    }
    let ledger = std::fs::read_to_string(false_positive_ledger_path()).unwrap_or_default();
    for l in ledger.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        fps.push(json!({"ledger": l}));
    }
    if !fps.is_empty() {
        reasons.push(format!("{} false positive(s) in the dry-run period — live mode needs 0", fps.len()));
    }
    (reasons, fps)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let apply = args.iter().any(|a| a == "--apply");
    let mode = if apply { Mode::Apply } else { Mode::DryRun };

    let ks = killswitch_path();
    if ks.exists() {
        println!("{}", json!({"disabled": true, "reason": "kill-switch present", "path": ks.display().to_string()}));
        return;
    }

    if mode == Mode::Apply {
        let (reasons, fps) = apply_refusals();
        if !reasons.is_empty() {
            println!("{}", json!({"refused": true, "mode": "apply", "reasons": reasons, "false_positives": fps}));
            std::process::exit(1);
        }
    }

    let threshold = dead_after();
    let world = World::load();
    let boards_list = boards_to_scan();
    let results: Vec<BoardResult> = boards_list.iter().filter_map(|b| scan_board(b, mode, threshold, &world)).collect();

    let dead_owner_cards: i64 = results.iter().map(|r| r.dead_owner_cards.len() as i64).sum();
    let orphans_idle_gt30m: i64 = results.iter().map(|r| r.idle_cards.len() as i64).sum();
    let released: Vec<serde_json::Value> = results
        .iter()
        .flat_map(|r| r.released.iter().map(move |id| json!({"board": r.board, "id": id})))
        .collect();
    let dead_owner_detail: Vec<serde_json::Value> = results
        .iter()
        .flat_map(|r| r.dead_owner_cards.iter().map(move |(id, owner)| json!({"board": r.board, "id": id, "owner": owner})))
        .collect();
    let idle_detail: Vec<serde_json::Value> =
        results.iter().flat_map(|r| r.idle_cards.iter().map(move |id| json!({"board": r.board, "id": id}))).collect();
    let alive_detail: Vec<serde_json::Value> = results
        .iter()
        .flat_map(|r| r.alive.iter().map(move |(id, o, by)| json!({"board": r.board, "id": id, "owner": o, "by": by})))
        .collect();
    let boards_scanned: Vec<&str> = results.iter().map(|r| r.board.as_str()).collect();

    let out = json!({
        "disabled": false,
        "mode": if apply { "apply" } else { "dry-run" },
        "liveness": LIVENESS_VERSION,
        "boards_scanned": boards_scanned,
        "dead_owner_cards": dead_owner_cards,
        "orphans_idle_gt30m": orphans_idle_gt30m,
        "dead_owner_detail": dead_owner_detail,
        "idle_detail": idle_detail,
        "alive_detail": alive_detail,
        "released": released,
    });
    if mode == Mode::DryRun {
        append_dry_run_log(&out);
    }
    println!("{out}");
}
