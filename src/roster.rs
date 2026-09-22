//! Who is on THIS board.
//!
//! The board says who: the owner of every card outside DONE, the reviewer of every REVIEW
//! card, and any actor with a card event in the last hour. herdr only adds a live harness
//! and status, and only from a pane whose agent name IS that actor's name (ASCII case aside,
//! nothing looser — no pane label, first word or terminal title): a guessed match would
//! report someone else's status as theirs. Every other herdr agent is counted as elsewhere
//! ("not on this board") and never described — tb does not read other boards.
//!
//! The AGENTS panel, the header count and `tb agents` all read this one list.
//!
//! A later change can add "harness · model" per actor from a table of its own: `Row` is
//! where that goes (next to `live`), and both renderers print the harness from one place
//! (`Row::harness`).

use crate::herdr::Agent;
use crate::store::{Card, Event, Snapshot, COLUMNS};

/// An actor holding no card still counts as here this long after its last card event.
pub const RECENT_SECS: i64 = 3600;
/// The GitHub sync writes events under this name; it is not someone working on the board.
const AUTOMATION: &str = "github";

/// What a row's card is to its actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardRole {
    /// The actor holds the card (`cards.owner`).
    Owner,
    /// The actor claimed the REVIEW card (`cards.reviewer`).
    Reviewer,
}

impl CardRole {
    pub fn as_str(self) -> &'static str {
        match self {
            CardRole::Owner => "owner",
            CardRole::Reviewer => "reviewer",
        }
    }
}

/// One actor of this board.
#[derive(Debug, Clone)]
pub struct Row<'a> {
    /// The name as the board wrote it.
    pub name: String,
    /// What it is on now: its DOING card, else the card it reviews, else another card it owns.
    pub card: Option<&'a Card>,
    pub role: Option<CardRole>,
    /// The herdr agent with exactly this name (None: no live harness or status to show).
    pub live: Option<&'a Agent>,
    /// The actor's latest card event (what a row without a card has to say).
    pub last: Option<&'a Event>,
}

impl Row<'_> {
    pub fn harness(&self) -> &str {
        self.live.map_or("-", |a| a.harness.as_str())
    }

    pub fn status(&self) -> &str {
        self.live.map_or("-", |a| a.status.as_str())
    }

    /// An idle agent that still holds a DOING card (the panel's `!` warning).
    pub fn idle_holder(&self) -> bool {
        self.live.is_some_and(Agent::is_idle) && self.role == Some(CardRole::Owner) && self.card.is_some_and(|c| c.column == "doing")
    }
}

/// The board's actors first, then the herdr agents that are none of them.
#[derive(Debug, Clone, Default)]
pub struct Roster<'a> {
    pub here: Vec<Row<'a>>,
    pub elsewhere: Vec<&'a Agent>,
}

impl Roster<'_> {
    pub fn total(&self) -> usize {
        self.here.len() + self.elsewhere.len()
    }

    /// Lines the AGENTS panel needs: one per actor here, one `+N elsewhere` line.
    pub fn panel_rows(&self) -> usize {
        self.here.len() + usize::from(!self.elsewhere.is_empty())
    }
}

/// `3 here` / `2 elsewhere`, the pair every AGENTS title and bar shows.
pub fn counts(r: &Roster) -> (String, String) {
    (format!("{} here", r.here.len()), format!("{} elsewhere", r.elsewhere.len()))
}

fn key(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

/// working < blocked < idle/done < anything else: one actor may run several panes, and the
/// busiest one is the truthful answer to "is it working".
fn status_rank(status: &str) -> u8 {
    match status {
        "working" => 0,
        "blocked" => 1,
        "idle" | "done" => 2,
        _ => 3,
    }
}

/// The herdr agent named exactly `k` (already lowercased). Unnamed panes never match.
fn live_for<'a>(agents: &'a [Agent], k: &str) -> Option<&'a Agent> {
    agents.iter().filter(|a| a.agent_name.as_deref().is_some_and(|n| key(n) == k)).min_by_key(|a| status_rank(&a.status))
}

pub fn roster<'a>(snap: &'a Snapshot, agents: &'a [Agent]) -> Roster<'a> {
    // (key, rank, row); rank 0 = holds a DOING card, 1 = reviews, 2 = owns another open card,
    // 3 = holds nothing, seen lately
    let mut rows: Vec<(String, u8, Row<'a>)> = Vec::new();
    let mut offer = |name: &str, rank: u8, card: &'a Card, role: CardRole| {
        let k = key(name);
        if k.is_empty() || k == AUTOMATION {
            return;
        }
        let place = |c: &Card| (COLUMNS.iter().position(|col| *col == c.column), c.position, c.id);
        match rows.iter_mut().find(|r| r.0 == k) {
            Some(r) => {
                let better = r.2.card.is_none_or(|cur| (rank, place(card)) < (r.1, place(cur)));
                if better {
                    (r.1, r.2.card, r.2.role) = (rank, Some(card), Some(role));
                }
            }
            None => rows.push((k, rank, Row { name: name.trim().to_string(), card: Some(card), role: Some(role), live: None, last: None })),
        }
    };
    for c in &snap.cards {
        if let Some(o) = c.owner.as_deref().filter(|_| c.column != "done") {
            offer(o, if c.column == "doing" { 0 } else { 2 }, c, CardRole::Owner);
        }
        if let Some(r) = c.reviewer.as_deref().filter(|_| c.column == "review") {
            offer(r, 1, c, CardRole::Reviewer);
        }
    }
    for (k, e) in &snap.actor_last {
        if k.is_empty() || k == AUTOMATION {
            continue;
        }
        match rows.iter_mut().find(|r| &r.0 == k) {
            Some(r) => r.2.last = Some(e),
            None if snap.now - e.ts < RECENT_SECS => {
                rows.push((k.clone(), 3, Row { name: e.actor.trim().to_string(), card: None, role: None, live: None, last: Some(e) }))
            }
            None => {}
        }
    }
    // card holders in board order, then the ones holding nothing, latest first
    rows.sort_by_key(|(k, rank, r)| {
        let at = r.card.map(|c| (COLUMNS.iter().position(|col| *col == c.column), c.position, c.id));
        let seen = if r.card.is_none() { r.last.map_or(0, |e| -e.ts) } else { 0 };
        (*rank, at, seen, k.clone())
    });
    let mut here = Vec::with_capacity(rows.len());
    let mut keys = Vec::with_capacity(rows.len());
    for (k, _, mut r) in rows {
        r.live = live_for(agents, &k);
        keys.push(k);
        here.push(r);
    }
    let elsewhere = agents.iter().filter(|a| !a.agent_name.as_deref().is_some_and(|n| keys.contains(&key(n)))).collect();
    Roster { here, elsewhere }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::parse_agents;

    fn card(id: i64, column: &str, owner: Option<&str>, reviewer: Option<&str>) -> Card {
        Card {
            id,
            title: format!("card {id}"),
            tag: None,
            description: String::new(),
            column: column.into(),
            owner: owner.map(str::to_string),
            due: None,
            gh_ref: None,
            created_at: 0,
            column_since: 0,
            blocked: None,
            position: id,
            reviewer: reviewer.map(str::to_string),
        }
    }

    fn event(card_id: i64, ts: i64, actor: &str) -> (String, Event) {
        (key(actor), Event { card_id, ts, actor: actor.into(), kind: "note".into(), text: "x".into(), actor_id: None })
    }

    /// The near misses: two actors whose names are only the START of an agent's name, and a
    /// pane whose LABEL (not its agent name) carries the actor's name. None of them may lend
    /// its live status to the board's actor.
    #[test]
    fn a_near_miss_name_never_lends_its_status() {
        let agents = parse_agents(
            r#"{"result":{"agents":[
              {"name":"dev-2","agent":"aider","agent_status":"working","pane_id":"w:p1"},
              {"name":"lead-docs","agent":"claude","agent_status":"working","pane_id":"w:p2"},
              {"agent":"aider","agent_status":"working","pane_id":"w:p3"},
              {"name":"Rev-1","agent":"aider","agent_status":"idle","pane_id":"w:p4"}
            ]}}"#,
            Some(
                r#"{"result":{"panes":[
              {"agent":"aider","label":"builder · model-x · aider · fix login","pane_id":"w:p3","terminal_title_stripped":"builder"}
            ]}}"#,
            ),
        )
        .unwrap();
        let snap = Snapshot {
            cards: vec![
                card(1, "doing", Some("dev"), None),
                card(2, "doing", Some("lead"), None),
                card(3, "doing", Some("builder"), None),
                card(4, "doing", Some("rev-1"), None),
            ],
            now: 10_000,
            ..Default::default()
        };
        let r = roster(&snap, &agents);
        let names: Vec<&str> = r.here.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["dev", "lead", "builder", "rev-1"]);
        // the old matcher would have given `builder` the unnamed pane's status (its label's
        // first word and its terminal title both say `builder`)
        assert!(crate::herdr::find_owner(&agents, &snap.cards[2]).is_some(), "the loose matcher does match it");
        for row in &r.here[..3] {
            assert!(row.live.is_none(), "{} took a near miss's status: {:?}", row.name, row.live);
            assert_eq!((row.harness(), row.status()), ("-", "-"));
        }
        // exact, ASCII case aside: that one is live
        assert_eq!(r.here[3].live.map(|a| a.pane_id.as_str()), Some("w:p4"));
        assert!(r.here[3].idle_holder());
        let elsewhere: Vec<&str> = r.elsewhere.iter().map(|a| a.pane_id.as_str()).collect();
        assert_eq!(elsewhere, ["w:p1", "w:p2", "w:p3"], "near misses are counted elsewhere, not dropped");
        assert_eq!((r.total(), r.panel_rows()), (7, 5));
    }

    #[test]
    fn who_is_here_and_in_which_order() {
        let mut snap = Snapshot {
            cards: vec![
                card(1, "todo", None, None),
                card(2, "review", Some("dev-2"), Some("rev-1")),
                card(3, "doing", Some("dev-1"), None),
                card(4, "review", Some("dev-1"), None),
                card(5, "done", Some("dev-9"), Some("rev-9")),
                card(6, "review", Some("rev-1"), None),
            ],
            now: 100_000,
            ..Default::default()
        };
        snap.actor_last.extend([
            event(1, 99_000, "lead"),
            event(1, 99_500, "Filer"),
            event(5, 100_000 - RECENT_SECS, "dev-9"),
            event(3, 99_900, "dev-1"),
            event(2, 99_990, "github"),
        ]);
        let r = roster(&snap, &[]);
        let got: Vec<(&str, Option<i64>, Option<&str>)> =
            r.here.iter().map(|x| (x.name.as_str(), x.card.map(|c| c.id), x.role.map(CardRole::as_str))).collect();
        assert_eq!(
            got,
            [
                ("dev-1", Some(3), Some("owner")),    // its DOING card, not the one waiting in REVIEW
                ("rev-1", Some(2), Some("reviewer")), // the card it reviews, not the one it owns
                ("dev-2", Some(2), Some("owner")),
                ("Filer", None, None), // holds nothing, seen lately: latest first
                ("lead", None, None),
            ],
            "done cards, the sync and an actor last seen an hour ago are not here"
        );
        assert_eq!(r.here[0].last.map(|e| e.ts), Some(99_900), "a holder keeps its last event too");
        assert!(r.elsewhere.is_empty());
    }

    #[test]
    fn several_panes_of_one_actor_show_the_busiest() {
        let agents = parse_agents(
            r#"{"result":{"agents":[
              {"name":"dev-1","agent":"aider","agent_status":"idle","pane_id":"w:p1"},
              {"name":"dev-1","agent":"aider","agent_status":"working","pane_id":"w:p2"}
            ]}}"#,
            None,
        )
        .unwrap();
        let snap = Snapshot { cards: vec![card(1, "doing", Some("dev-1"), None)], ..Default::default() };
        let r = roster(&snap, &agents);
        assert_eq!(r.here[0].live.map(|a| a.pane_id.as_str()), Some("w:p2"));
        assert!(!r.here[0].idle_holder(), "one working pane: not an idle holder");
        assert!(r.elsewhere.is_empty(), "both panes are this actor's");
    }
}
