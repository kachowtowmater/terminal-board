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
    pub gh_ref: Option<i64>,
    pub blocked: Option<String>,
    pub created_at: i64,
    pub column_since: i64,
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
}

/// A card with its checklist and last events.
pub fn card(store: &Store, c: &Card) -> Result<CardJ> {
    let d = store.show(c.id)?;
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
        gh_ref: c.gh_ref,
        blocked: c.blocked.clone(),
        created_at: c.created_at,
        column_since: c.column_since,
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
    let col = |name: &str| -> Result<Vec<CardJ>> { snap.in_column(name).into_iter().map(|c| card(store, c)).collect() };
    let (json, error) = store.github_cache()?;
    let repo = store.github_repo()?;
    let snapshot = json
        .filter(|_| repo.is_some())
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or(serde_json::Value::Null);
    debug_assert_eq!(COLUMNS.len(), 4);
    Ok(BoardJ {
        v: SCHEMA_VERSION,
        board: store.name.clone(),
        wip: snap.wip,
        theme: snap.theme.clone(),
        layout: snap.layout.clone(),
        github: GithubJ { repo, snapshot, error },
        columns: ColumnsJ { todo: col("todo")?, doing: col("doing")?, review: col("review")?, done: col("done")? },
    })
}

/// herdr agents merged with the board: the card each one holds (doing first).
pub fn agents(list: &[Agent], cards: &[Card]) -> Vec<AgentJ> {
    list.iter()
        .map(|a| {
            let mine: Vec<&Card> = cards
                .iter()
                .filter(|c| c.column != "done" && herdr::find_owner(list, c).is_some_and(|o| o.pane_id == a.pane_id))
                .collect();
            let held = mine.iter().find(|c| c.column == "doing").or(mine.first());
            AgentJ {
                name: a.name.clone(),
                harness: a.harness.clone(),
                status: a.status.clone(),
                pane_id: a.pane_id.clone(),
                job: a.job.clone(),
                card_id: held.map(|c| c.id),
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
