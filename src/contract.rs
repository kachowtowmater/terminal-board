//! The JSON contract for apps and agents (`board --json`, `watch --json`, write `--json`,
//! `agents --json`). Field names here are pinned by golden tests; see docs/JSON.md.

use crate::herdr::{self, Agent};
use crate::store::{Card, Result, Store, COLUMNS};
use serde::Serialize;

/// Schema version of every JSON object below.
pub const SCHEMA_VERSION: u32 = 1;
/// How many recent events a card carries.
pub const CARD_EVENTS: usize = 10;

#[derive(Debug, Clone, Serialize)]
pub struct CheckJ {
    pub n: i64,
    /// Deprecated alias of `n` (what `tb show --json` used to call it); same value.
    pub idx: i64,
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventJ {
    pub ts: i64,
    pub actor: String,
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CardJ {
    pub id: i64,
    pub title: String,
    pub tag: Option<String>,
    pub description: String,
    pub column: String,
    pub position: i64,
    pub owner: Option<String>,
    /// Who claimed it with `tb next --review`; null when unclaimed.
    pub reviewer: Option<String>,
    pub due: Option<String>,
    /// Whole calendar days from the board's today to `due` (0 = today, negative = past); null
    /// without a `YYYY-MM-DD` due date and on a `done` card. See `store::due`.
    pub days_left: Option<i64>,
    /// `ok` | `soon` (within `due-warn` days) | `overdue`; null exactly when `days_left` is.
    pub due_state: Option<&'static str>,
    /// What a person reads for `column`: the board's label, else the name in capitals.
    /// Display only — `column` is the name every command takes, and it never changes.
    pub column_label: String,
    pub gh_ref: Option<i64>,
    pub blocked: Option<String>,
    pub created_at: i64,
    pub column_since: i64,
    /// Unix seconds of the card's last event (any kind); readers compute staleness themselves.
    pub last_event_at: i64,
    pub checklist: Vec<CheckJ>,
    /// Rework round: 1, plus one per send-back (`returned` event) — counted from events.
    pub round: i64,
    /// The last 10 events, oldest first.
    pub events: Vec<EventJ>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GithubJ {
    pub repo: Option<String>,
    /// The cached `ttyboard github --json` snapshot, or null.
    pub snapshot: serde_json::Value,
    pub error: Option<String>,
    /// Consecutive failed refreshes (the UI goes red only after 3).
    pub fails: i64,
    /// Unix time of the last good snapshot (0 = never fetched).
    pub fetched_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnsJ {
    pub todo: Vec<CardJ>,
    pub doing: Vec<CardJ>,
    pub review: Vec<CardJ>,
    pub done: Vec<CardJ>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoardJ {
    pub v: u32,
    pub board: String,
    pub wip: i64,
    pub theme: String,
    pub layout: String,
    /// Display names of the four columns (`config label`); the internal names in capitals
    /// unless the board set its own. Chrome: `columns` keys and `card.column` never change.
    pub labels: std::collections::BTreeMap<&'static str, String>,
    pub github: GithubJ,
    pub columns: ColumnsJ,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentJ {
    pub name: String,
    pub harness: String,
    pub status: String,
    pub pane_id: String,
    pub job: Option<String>,
    pub card_id: Option<i64>,
    /// The held card's last note text (None when it holds no card or never noted).
    pub last_note: Option<String>,
    /// Unix seconds of the held card's last event (any kind); the screen computes the age.
    pub last_event_at: Option<i64>,
}

/// A card with its checklist and last events.
pub fn card(store: &Store, c: &Card) -> Result<CardJ> {
    card_on(store, c, &store.due_ctx()?)
}

/// `card` with the board's today already worked out (once per board, not once per card).
fn card_on(store: &Store, c: &Card, due: &crate::store::due::DueCtx) -> Result<CardJ> {
    card_shown(store, c, due, &store.display()?)
}

fn card_shown(store: &Store, c: &Card, due: &crate::store::due::DueCtx, look: &crate::store::display::Display) -> Result<CardJ> {
    let d = store.show(c.id)?;
    let due = due.info(c);
    let skip = d.events.len().saturating_sub(CARD_EVENTS);
    Ok(CardJ {
        id: c.id,
        title: c.title.clone(),
        tag: c.tag.clone(),
        description: c.description.clone(),
        column: c.column.clone(),
        position: c.position,
        owner: c.owner.clone(),
        reviewer: c.reviewer.clone(),
        due: c.due.clone(),
        days_left: due.days_left,
        due_state: due.due_state,
        column_label: look.column_label(&c.column),
        gh_ref: c.gh_ref,
        blocked: c.blocked.clone(),
        created_at: c.created_at,
        column_since: c.column_since,
        last_event_at: d.events.last().map(|e| e.ts).unwrap_or(c.created_at),
        checklist: d.checklist.iter().map(|i| CheckJ { n: i.idx, idx: i.idx, text: i.text.clone(), done: i.done }).collect(),
        round: crate::store::round_of(&d.events),
        events: d
            .events
            .iter()
            .skip(skip)
            .map(|e| EventJ { ts: e.ts, actor: e.actor.clone(), kind: e.kind.clone(), text: e.text.clone() })
            .collect(),
    })
}

pub fn card_by_id(store: &Store, id: i64) -> Result<CardJ> {
    card(store, &store.card(id)?)
}

/// The whole board in one object. TODO/DOING/REVIEW in position order; DONE newest first
/// (all done cards; apps filter).
pub fn board(store: &Store) -> Result<BoardJ> {
    let snap = store.snapshot()?;
    let due = store.due_ctx()?;
    let look = &snap.display;
    let col = |name: &str| -> Result<Vec<CardJ>> { snap.in_column(name).into_iter().map(|c| card_shown(store, c, &due, look)).collect() };
    let (json, error, fails) = store.github_cache()?;
    let repo = store.github_repo()?;
    let snapshot = json
        .filter(|_| repo.is_some())
        .and_then(|j| serde_json::from_str::<serde_json::Value>(j.trim()).ok())
        .unwrap_or(serde_json::Value::Null);
    let fetched_at = snapshot.get("fetched_at").and_then(|f| f.as_i64()).unwrap_or(0);
    debug_assert_eq!(COLUMNS.len(), 4);
    Ok(BoardJ {
        v: SCHEMA_VERSION,
        board: store.name.clone(),
        wip: snap.wip,
        theme: snap.theme.clone(),
        layout: snap.layout.clone(),
        labels: COLUMNS.iter().map(|c| (*c, look.column_label(c))).collect(),
        github: GithubJ { repo, snapshot, error, fails, fetched_at },
        columns: ColumnsJ { todo: col("todo")?, doing: col("doing")?, review: col("review")?, done: col("done")? },
    })
}

/// herdr agents merged with the board: the card each one holds (doing first), with that
/// card's last note and the age of its last activity (what the agent is doing).
pub fn agents(list: &[Agent], snap: &crate::store::Snapshot) -> Vec<AgentJ> {
    list.iter()
        .map(|a| {
            let mine: Vec<&Card> = snap
                .cards
                .iter()
                .filter(|c| c.column != "done" && herdr::find_owner(list, c).is_some_and(|o| o.pane_id == a.pane_id))
                .collect();
            let held = mine.iter().find(|c| c.column == "doing").or(mine.first());
            let (last_note, last_event_at) = match held {
                Some(c) => (snap.last_note.get(&c.id).cloned(), snap.last_event_at.get(&c.id).copied()),
                None => (None, None),
            };
            AgentJ {
                name: a.name.clone(),
                harness: a.harness.clone(),
                status: a.status.clone(),
                pane_id: a.pane_id.clone(),
                job: a.job.clone(),
                card_id: held.map(|c| c.id),
                last_note,
                last_event_at,
            }
        })
        .collect()
}

/// `{"ok":false,"error":…,"hint":…}` from an error message shaped "what — what to do".
pub fn error(msg: &str) -> serde_json::Value {
    let (e, hint) = match msg.split_once(" — ") {
        Some((e, h)) => (e.trim().to_string(), h.trim().to_string()),
        None => (msg.trim().to_string(), "see 'tb --help'".to_string()),
    };
    serde_json::json!({"ok": false, "error": e, "hint": hint})
}
