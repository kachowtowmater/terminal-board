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
//! The liveness probes themselves (and the `TB_REAP_FAKE_*` fixture mode) live in the
//! library, `terminal_board::liveness` — `tb release` asks the SAME world, so the two can
//! never drift apart.

use serde_json::json;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use terminal_board::boards;
use terminal_board::liveness::{eq_ci, World};
use terminal_board::store::Store;

/// Bumped when the liveness rules change: only dry-run lines written under the current rules
/// count toward the 7-day proof `--apply` needs.
const LIVENESS_VERSION: i64 = 2;
const APPLY_MIN_DAYS: i64 = 7;

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
