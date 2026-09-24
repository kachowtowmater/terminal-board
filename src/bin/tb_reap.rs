//! `tb-reap` — card-driven pane lifecycle (orch-restructure W-3, tb orch #28, U5/R4).
//!
//! State machine (see the gate at `~/Vault/openos/plans/orch-restructure/gates/W-3.sh`):
//!   - card DONE / dropped to TODO -> a live caller (herdr/pane owner) closes its pane; this
//!     binary only reports which cards left DOING, it never touches panes/tmux itself.
//!   - card REVIEW -> never touched (the builder may still be asked to fix it).
//!   - a DOING card whose owner has **no herdr agent AND no tmux session** for
//!     `TB_REAP_DEAD_AFTER` seconds (default 1800 = 30m, measured from the card's time in
//!     DOING — the only clock tb has) -> released to TODO with a note, via a direct library
//!     call (never the CLI's `--force` flag).
//!   - "idle" = a LIVE owner whose herdr agent status is `idle`/`done` (no CPU, no fresh pane
//!     output — herdr's own signal, `Agent::is_idle`) while the card is still in DOING.
//!     Idle agents are REPORTED only, never released — a long *working* session sitting in
//!     DOING for hours is not idle, and must never be flagged just for its age.
//!   - a card carrying a `mode:headless` note is NEVER dead-owner-released for lacking a pane:
//!     headless (Agent-tool) workers have no herdr agent and no tmux session by design.
//!
//! Modes: `--dry-run` (report only, never writes; also appends a dated line to
//! `~/.local/state/tb-reap/dry-run.log`, the 7-day proof the gate reads) and `--apply`
//! (actually releases dead-owner cards). Neither flag given defaults to the safe `--dry-run`
//! behaviour. A kill-switch file (`TB_REAP_KILLSWITCH`, else `~/.config/tb/reap.disabled`)
//! short-circuits everything to `{"disabled":true}` and exit 0.

use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use terminal_board::boards;
use terminal_board::herdr::{self, Agent, AgentsState};
use terminal_board::store::{Card, Store};

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

fn dry_run_log_path() -> PathBuf {
    home().join(".local/state/tb-reap/dry-run.log")
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn dead_after() -> i64 {
    std::env::var("TB_REAP_DEAD_AFTER").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(1800)
}

/// `TB_REAP_FAKE_AGENTS` / `TB_REAP_FAKE_TMUX`, comma-separated agent/session names. The var
/// being SET (even to "") switches the check into fixture mode: `Some(vec![])` means "no live
/// agents/sessions at all", vs `None` (unset) meaning "ask the real world". `std::env::var` is
/// used directly (not `terminal_board::env`, which treats "" as unset) so a test can assert
/// "nothing is alive".
fn fake_list(var: &str) -> Option<Vec<String>> {
    std::env::var(var).ok().map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Real or fixture herdr agents, asked once per run.
enum Agents {
    Fake(Vec<String>),
    Real(Vec<Agent>),
    Unavailable,
}

fn load_agents() -> Agents {
    if let Some(names) = fake_list("TB_REAP_FAKE_AGENTS") {
        return Agents::Fake(names);
    }
    match herdr::probe() {
        AgentsState::Agents(a) => Agents::Real(a),
        _ => Agents::Unavailable,
    }
}

/// The live agent behind this card's owner, when one exists. Fixture mode has no per-agent
/// record to hand back (just a name list), so it reports liveness only, no status.
fn live_agent<'a>(agents: &'a Agents, card: &Card) -> Option<Option<&'a Agent>> {
    let owner = card.owner.as_deref()?;
    match agents {
        Agents::Fake(names) => names.iter().any(|n| eq_ci(n, owner)).then_some(None),
        Agents::Real(a) => herdr::exact_owner(a, card).map(Some),
        Agents::Unavailable => None,
    }
}

/// Live tmux session names, asked once per run (empty when `tmux` is absent or has no server
/// running — never an error, just "nothing is live").
fn load_tmux(fake: &Option<Vec<String>>) -> Vec<String> {
    if let Some(names) = fake {
        return names.clone();
    }
    let out = Command::new("tmux").arg("list-sessions").arg("-F").arg("#{session_name}").output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

fn tmux_alive(tmux: &[String], card: &Card) -> bool {
    let Some(owner) = card.owner.as_deref() else { return false };
    tmux.iter().any(|n| eq_ci(n, owner))
}

/// A card carries the `mode:headless` marker when one of its notes says so verbatim (the
/// convention every headless brief starts with, e.g. `tb orch note 28 "mode:headless" --as …`).
fn is_headless(store: &Store, id: i64) -> bool {
    match store.show(id) {
        Ok(detail) => detail.events.iter().any(|e| e.kind == "note" && e.text.contains("mode:headless")),
        Err(_) => false,
    }
}

struct BoardResult {
    board: String,
    dead_owner_cards: Vec<(i64, Option<String>)>,
    idle_cards: Vec<i64>,
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

fn scan_board(name: &str, mode: Mode, threshold: i64, agents: &Agents, tmux: &[String]) -> Option<BoardResult> {
    let path = boards::path_for(name);
    let store = Store::open_if_exists(&path).ok()??.named(name);
    let cards = store.list().ok()?;
    let mut dead_owner_cards = Vec::new();
    let mut idle_cards = Vec::new();
    let mut released = Vec::new();
    let t = now();
    for c in cards.iter().filter(|c| c.column == "doing") {
        let age = t - c.column_since;
        let found = live_agent(agents, c);
        let alive = found.is_some() || tmux_alive(tmux, c);
        if !alive {
            if is_headless(&store, c.id) {
                // Headless workers never carry a pane or a tmux session by design — absence
                // of either is not evidence of death.
                continue;
            }
            if age >= threshold {
                dead_owner_cards.push((c.id, c.owner.clone()));
            }
        } else {
            // Alive: idle only when herdr's own status says so (no CPU, no fresh output) —
            // never inferred from how long the card has sat in DOING. A long *working*
            // session is not idle just because it is old.
            let herdr_idle = matches!(found, Some(Some(a)) if a.is_idle());
            if herdr_idle {
                idle_cards.push(c.id);
            }
        }
    }
    if mode == Mode::Apply {
        let mut store = store;
        for (id, owner) in &dead_owner_cards {
            let owner = owner.as_deref().unwrap_or("unknown");
            let reason =
                format!("tb-reap: released — owner '{owner}' had no herdr agent and no tmux session for >= {threshold}s");
            // force=true here is a direct library call, never the CLI's `--force` flag: the
            // reaper is the one caller allowed to release a dead owner's card without asking.
            if store.move_opts(*id, "todo", "tb-reap", true, Some(&reason)).is_ok() {
                released.push(*id);
            }
        }
    }
    Some(BoardResult { board: name.to_string(), dead_owner_cards, idle_cards, released })
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let apply = args.iter().any(|a| a == "--apply");
    let mode = if apply { Mode::Apply } else { Mode::DryRun };

    let ks = killswitch_path();
    if ks.exists() {
        println!("{}", json!({"disabled": true, "reason": "kill-switch present", "path": ks.display().to_string()}));
        return;
    }

    let threshold = dead_after();
    let agents = load_agents();
    let fake_tmux = fake_list("TB_REAP_FAKE_TMUX");
    let tmux = load_tmux(&fake_tmux);
    let boards_list = boards_to_scan();
    let results: Vec<BoardResult> =
        boards_list.iter().filter_map(|b| scan_board(b, mode, threshold, &agents, &tmux)).collect();

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
    let boards_scanned: Vec<&str> = results.iter().map(|r| r.board.as_str()).collect();

    let out = json!({
        "disabled": false,
        "mode": if apply { "apply" } else { "dry-run" },
        "boards_scanned": boards_scanned,
        "dead_owner_cards": dead_owner_cards,
        "orphans_idle_gt30m": orphans_idle_gt30m,
        "dead_owner_detail": dead_owner_detail,
        "idle_detail": idle_detail,
        "released": released,
    });
    if mode == Mode::DryRun {
        append_dry_run_log(&out);
    }
    println!("{out}");
}
