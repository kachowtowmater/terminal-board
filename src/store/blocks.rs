//! Structured blocks: WHO we are waiting on, and WHEN to look again.
//!
//! `tb block ID "text"` is unchanged — free text, as it always was. `--on NAME|#ID` says who
//! or what the card waits for, and `--until DATE` says when to look again. Both are stored
//! next to the block text (`blocked_on`, `blocked_until`), so a board can be asked "what are
//! we waiting on, and what is due for a recheck" instead of reading prose.
//!
//! Everything derived is derived AT READ TIME, from the board's `tz` and `store::now()`
//! (which `TB_NOW` pins) — there is no stored flag and no background clock:
//! - **recheck**: the `--until` date has arrived (today or past);
//! - **the state of the card we wait on**: `open`, `done` or `gone` (deleted or archived).
//!
//! A card blocked `--on #ID` unblocks ITSELF when that card reaches DONE (one statement in
//! the transition, so it is part of the same transaction as the move). Removing a card is
//! not finishing it: a delete or an archive leaves the block standing and reports `gone`,
//! because tb must not decide on its own that someone's "we are waiting on this" is void.
//! Reopening a finished card does not block anything again either — the unblock is in the
//! card's history, and re-blocking behind the user's back would fight them.

use super::due::{self, DueCtx};
use super::{err, Card, Connection, Result, Store};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

/// Longest `--on` text. A name or `#ID`, not a sentence — the sentence is the block text.
pub const ON_MAX: usize = 40;

/// `#7` -> Some(7): the `--on` form that names another card on this board.
pub fn on_card(on: &str) -> Option<i64> {
    on.trim().strip_prefix('#').and_then(|n| n.parse::<i64>().ok()).filter(|n| *n > 0)
}

/// `--on NAME|#ID`, cleaned: `#7` for a card (whatever spacing was typed), else the name as
/// given (sanitised, whitespace collapsed). Refused when empty or too long.
pub fn clean_on(id: i64, on: &str) -> Result<String> {
    let text = crate::text::sanitize(on).split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(n) = on_card(&text) {
        if n == id {
            return err(format!("#{id} cannot wait for itself — name the other card: 'tb block {id} \"…\" --on #7'"));
        }
        return Ok(format!("#{n}"));
    }
    if text.is_empty() {
        return err(format!("say who you are waiting on — 'tb block {id} \"…\" --on \"the other side\"' or '--on #7'"));
    }
    let n = text.chars().count();
    if n > ON_MAX {
        return err(format!(
            "'--on' is {n} characters, the limit is {ON_MAX} — that is who you wait for, the detail goes in the text: 'tb block {id} \"detail\" --on NAME'"
        ));
    }
    Ok(text)
}

/// What a card's block means today. Every field is derived; none is stored.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct BlockInfo {
    /// `--on`: `#7` or a name; null for a block with no `--on`.
    pub blocked_on: Option<String>,
    /// `--until`: a local calendar date `YYYY-MM-DD`; null without one.
    pub blocked_until: Option<String>,
    /// The `--until` date has arrived (today or earlier) in the board's zone: look again.
    pub recheck: bool,
    /// For `--on #ID`: `open` · `done` · `gone` (deleted or archived). Null otherwise.
    pub blocked_on_state: Option<&'static str>,
}

impl BlockInfo {
    /// Is there anything to say? (A card with no block has none of it.)
    pub fn any(&self) -> bool {
        self.blocked_on.is_some() || self.blocked_until.is_some()
    }
}

/// A card-shaped value plus the derived block fields, in the same additive shape the due
/// and label wrappers use (`tb list --json`, `tb show --json`).
#[derive(Debug, Serialize)]
pub struct WithBlock<T: Serialize> {
    #[serde(flatten)]
    pub inner: T,
    #[serde(flatten)]
    pub block: BlockInfo,
}

/// The columns and rows `BlockInfo` needs, read once per board.
#[derive(Debug, Clone, Default)]
pub struct BlockCtx {
    pub due: Option<DueCtx>,
    /// Column of every card id on the board (to say whether the card we wait on is done).
    pub columns: std::collections::HashMap<i64, String>,
}

impl BlockCtx {
    /// `value` with `blocked_on`, `blocked_until`, `recheck` and `blocked_on_state` after
    /// its own fields (for `--json`).
    pub fn with<T: Serialize>(&self, value: T, card: &Card) -> WithBlock<T> {
        WithBlock { inner: value, block: self.of(card) }
    }

    pub fn of(&self, card: &Card) -> BlockInfo {
        if card.blocked.is_none() && card.blocked_on.is_none() && card.blocked_until.is_none() {
            return BlockInfo::default();
        }
        let recheck = match (&card.blocked_until, self.due) {
            (Some(u), Some(ctx)) => due::parse_date(u).is_some_and(|d| d <= ctx.today),
            _ => false,
        };
        let state = card.blocked_on.as_deref().and_then(on_card).map(|n| match self.columns.get(&n).map(String::as_str) {
            Some("done") => "done",
            Some(_) => "open",
            None => "gone",
        });
        BlockInfo {
            blocked_on: card.blocked_on.clone(),
            blocked_until: card.blocked_until.clone(),
            recheck,
            blocked_on_state: state,
        }
    }
}

/// Add `blocked_on` and `blocked_until` — to a board written before this version AND to a
/// fresh one, so every board has the same table whichever way it was made (the pattern
/// `reviewer` already follows; a foreign reader sees one shape, never two).
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    for col in ["blocked_on", "blocked_until"] {
        let have: i64 =
            conn.query_row("SELECT COUNT(*) FROM pragma_table_info('cards') WHERE name=?", [col], |r| r.get(0))?;
        if have == 0 {
            match conn.execute_batch(&format!("ALTER TABLE cards ADD COLUMN {col} TEXT")) {
                Err(e) if e.to_string().contains("duplicate column") => {}
                Err(e) => return Err(e.into()),
                Ok(()) => {}
            }
        }
    }
    Ok(())
}

/// Cards waiting on `#done_id`, unblocked because it reached DONE. Runs inside the
/// transition's transaction, so the move and the unblocks commit together.
pub(super) fn on_done(tx: &Connection, done_id: i64, actor: &str) -> Result<Vec<i64>> {
    let token = format!("#{done_id}");
    let waiting: Vec<i64> = {
        let mut st = tx.prepare("SELECT id FROM cards WHERE blocked_on=? ORDER BY id")?;
        let v = st.query_map([&token], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    for id in &waiting {
        tx.execute("UPDATE cards SET blocked=NULL, blocked_on=NULL, blocked_until=NULL WHERE id=?", [id])?;
        Store::log(tx, *id, actor, "unblocked", &format!("{token} is done"))?;
    }
    Ok(waiting)
}

/// How many DOING cards count against the WIP limit, and the whole DOING count.
///
/// With `wip-counts-blocked no` a blocked card frees a work slot — but only up to `wip` of
/// them board-wide. Past that, blocked cards count again, so blocking everything can never
/// hand out unlimited work: DOING can never exceed twice the limit, whatever anyone blocks.
pub(super) fn doing_counts(tx: &Connection, wip: i64) -> Result<(i64, i64)> {
    let doing: i64 = tx.query_row(r#"SELECT COUNT(*) FROM cards WHERE "column"='doing'"#, [], |r| r.get(0))?;
    if !wip_counts_blocked_off(tx)? {
        return Ok((doing, doing));
    }
    let blocked: i64 = tx.query_row(
        r#"SELECT COUNT(*) FROM cards WHERE "column"='doing' AND (blocked IS NOT NULL OR blocked_on IS NOT NULL)"#,
        [],
        |r| r.get(0),
    )?;
    Ok((doing - blocked.min(wip.max(0)), doing))
}

fn wip_counts_blocked_off(conn: &Connection) -> Result<bool> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM config WHERE key='wip-counts-blocked'", [], |r| r.get(0)).optional()?;
    Ok(v.is_some_and(|v| v.trim().eq_ignore_ascii_case("no")))
}

impl Store {
    /// `tb block ID "text" [--on NAME|#ID] [--until DATE]`, or clear it all with None.
    /// `on` and `until` are already cleaned (`clean_on`, `due::DueDate`).
    pub fn block_opts(&self, id: i64, text: Option<&str>, on: Option<&str>, until: Option<&str>, actor: &str) -> Result<()> {
        self.card(id)?;
        let text = text.map(|r| r.trim().trim_start_matches("by ").trim().to_string());
        if text.as_deref() == Some("") {
            return err(format!("say what blocks it — 'tb block {id} \"#7\"'"));
        }
        // waiting for a card that is already finished would wait for ever: the move that
        // would clear it has happened. Say so instead of storing a block nothing can lift.
        if let Some(n) = on.and_then(on_card) {
            let other = self.card(n).map_err(|_| {
                super::BoardError(format!("no card #{n} to wait for — see 'tb list' for ids, or name who you wait on: 'tb block {id} \"…\" --on NAME'"))
            })?;
            if other.column == "done" {
                return err(format!(
                    "#{n} is already done, so nothing would lift that block — block #{id} on something open, or leave it unblocked"
                ));
            }
        }
        // a write transaction from the start, consistent with every other write path (#85).
        // `&self`, so `unchecked_transaction` + the ROLLBACK/BEGIN IMMEDIATE trick
        // (`transaction_with_behavior` needs `&mut Connection`) — see `store.rs::add_tagged`.
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("ROLLBACK; BEGIN IMMEDIATE")?;
        tx.execute(
            "UPDATE cards SET blocked=?, blocked_on=?, blocked_until=? WHERE id=?",
            params![text, on, until, id],
        )?;
        match &text {
            Some(r) => {
                let mut what = format!("by {r}");
                if let Some(o) = on {
                    what.push_str(&format!(" · on {o}"));
                }
                if let Some(u) = until {
                    what.push_str(&format!(" · until {u}"));
                }
                Self::log(&tx, id, actor, "blocked", &what)?;
            }
            None => Self::log(&tx, id, actor, "unblocked", "")?,
        }
        tx.commit()?;
        Ok(())
    }

    /// Everything the derived block fields need, read once per board.
    pub fn block_ctx(&self) -> Result<BlockCtx> {
        let mut st = self.conn.prepare(r#"SELECT id, "column" FROM cards"#)?;
        let columns = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()?;
        Ok(BlockCtx { due: Some(self.due_ctx()?), columns })
    }

    /// `wip-counts-blocked yes|no` — `yes` (the default) is today's behaviour.
    pub fn wip_counts_blocked(&self) -> Result<bool> {
        Ok(!wip_counts_blocked_off(&self.conn)?)
    }

    pub fn set_wip_counts_blocked(&self, value: &str) -> Result<bool> {
        let v = value.trim().to_ascii_lowercase();
        let yes = match v.as_str() {
            "yes" | "on" | "true" => true,
            "no" | "off" | "false" => false,
            _ => {
                return err(format!(
                    "'{}' is not yes|no — 'tb config wip-counts-blocked no' frees a work slot while a card is blocked",
                    value.trim()
                ))
            }
        };
        self.set_config("wip-counts-blocked", if yes { "yes" } else { "no" })?;
        Ok(yes)
    }

    /// `waiting-lane shown|hidden` — hidden (the default) is today's board.
    pub fn waiting_lane(&self) -> Result<bool> {
        let v: Option<String> = self
            .conn
            .query_row("SELECT value FROM config WHERE key='waiting-lane'", [], |r| r.get(0))
            .optional()?;
        Ok(v.is_some_and(|v| v.trim().eq_ignore_ascii_case("shown")))
    }

    pub fn set_waiting_lane(&self, value: &str) -> Result<bool> {
        let v = value.trim().to_ascii_lowercase();
        let shown = match v.as_str() {
            "shown" | "show" | "on" => true,
            "hidden" | "hide" | "off" => false,
            _ => {
                return err(format!(
                    "'{}' is not shown|hidden — 'tb config waiting-lane shown' gives blocked cards their own section",
                    value.trim()
                ))
            }
        };
        self.set_config("waiting-lane", if shown { "shown" } else { "hidden" })?;
        Ok(shown)
    }

    /// The block settings this board has SET, for the `tb config` listing (a board that sets
    /// none lists none, so its listing is unchanged).
    pub fn block_settings(&self) -> Result<Vec<(String, String)>> {
        let mut v = Vec::new();
        let set = |key: &str| -> Result<bool> {
            Ok(self.conn.query_row("SELECT 1 FROM config WHERE key=?", [key], |r| r.get::<_, i64>(0)).optional()?.is_some())
        };
        if set("wip-counts-blocked")? {
            v.push(("wip-counts-blocked".to_string(), if self.wip_counts_blocked()? { "yes" } else { "no" }.to_string()));
        }
        if set("waiting-lane")? {
            v.push(("waiting-lane".to_string(), if self.waiting_lane()? { "shown" } else { "hidden" }.to_string()));
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(today: &str, columns: &[(i64, &str)]) -> BlockCtx {
        BlockCtx {
            due: due::parse_date(today).map(|today| DueCtx { today, warn: 3 }),
            columns: columns.iter().map(|(i, c)| (*i, c.to_string())).collect(),
        }
    }

    fn card(blocked: Option<&str>, on: Option<&str>, until: Option<&str>) -> Card {
        Card {
            id: 1,
            title: "wait".into(),
            tag: None,
            description: String::new(),
            column: "doing".into(),
            owner: None,
            due: None,
            gh_ref: None,
            created_at: 0,
            column_since: 0,
            blocked: blocked.map(str::to_string),
            blocked_on: on.map(str::to_string),
            blocked_until: until.map(str::to_string),
            position: 0,
            reviewer: None,
        }
    }

    #[test]
    fn recheck_is_derived_from_the_boards_today_never_stored() {
        let c = card(Some("the signed copy"), Some("#7"), Some("2026-10-09"));
        for (today, want) in [("2026-10-07", false), ("2026-10-08", false), ("2026-10-09", true), ("2026-10-10", true)] {
            let i = ctx(today, &[(7, "doing")]).of(&c);
            assert_eq!(i.recheck, want, "today {today}");
            assert_eq!((i.blocked_on.as_deref(), i.blocked_until.as_deref()), (Some("#7"), Some("2026-10-09")));
        }
        // no `--until`, or a board with no today at all: never a recheck
        assert!(!ctx("2030-01-01", &[(7, "doing")]).of(&card(Some("x"), Some("#7"), None)).recheck);
        let no_today = BlockCtx { due: None, columns: Default::default() };
        assert!(!no_today.of(&c).recheck);
    }

    #[test]
    fn the_state_of_the_card_we_wait_on_is_open_done_or_gone() {
        let c = card(Some("x"), Some("#7"), None);
        assert_eq!(ctx("2026-10-09", &[(7, "todo")]).of(&c).blocked_on_state, Some("open"));
        assert_eq!(ctx("2026-10-09", &[(7, "review")]).of(&c).blocked_on_state, Some("open"));
        assert_eq!(ctx("2026-10-09", &[(7, "done")]).of(&c).blocked_on_state, Some("done"));
        assert_eq!(ctx("2026-10-09", &[]).of(&c).blocked_on_state, Some("gone"), "deleted or archived");
        // waiting on a NAME says nothing about a card
        assert_eq!(ctx("2026-10-09", &[(7, "done")]).of(&card(Some("x"), Some("the other side"), None)).blocked_on_state, None);
        // a card with no block at all has nothing to report
        assert_eq!(ctx("2026-10-09", &[]).of(&card(None, None, None)), BlockInfo::default());
    }

    #[test]
    fn on_is_cleaned_bounded_and_never_the_card_itself() {
        assert_eq!(clean_on(1, " #7 ").unwrap(), "#7");
        assert_eq!(clean_on(1, "#0007").unwrap(), "#7");
        assert_eq!(clean_on(1, "  the   other side \x1b[31m").unwrap(), "the other side");
        assert!(clean_on(1, "#1").unwrap_err().to_string().starts_with("#1 cannot wait for itself"));
        assert!(clean_on(1, "  ").unwrap_err().to_string().starts_with("say who you are waiting on"));
        let long = clean_on(1, &"n".repeat(41)).unwrap_err().to_string();
        assert!(long.contains("the limit is 40") && long.contains("the detail goes in the text"), "{long}");
        assert!(clean_on(1, &"n".repeat(40)).is_ok());
        // `#0` and `#-2` are not card refs, so they are names
        assert_eq!(clean_on(1, "#0").unwrap(), "#0");
        assert_eq!(on_card("#0"), None);
    }
}
