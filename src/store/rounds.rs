//! `max-rounds`: a cap on how many times a card may be sent back before it is marked
//! **escalate** — so a loop between a worker and a reviewer cannot run forever unnoticed.
//!
//! `escalate` is DERIVED, never stored — exactly like `recheck` (`store::blocks`) and
//! `due_state` (`store::due`): a board that has never set `max-rounds` computes it as `false`
//! for every card, so a board that sets nothing is unchanged, and raising or clearing the cap
//! changes every card's escalate state on the next read, with nothing to migrate. The round
//! count itself already exists (`round_of`, from `returned` events) — this module builds on
//! it rather than adding a second counter.
//!
//! An escalated card is `tb next` / `tb next --review`'s AUTOMATIC top pick only: it is
//! skipped there so nobody quietly hands the loop another round, but it is never hidden —
//! `tb list`, `tb board` and `tb show` show it exactly as before, and `tb take ID` / `tb move`
//! / `tb done` act on it directly, same as any other card. The only way to clear it is to
//! finish the card (a `done` card is never escalate) or to raise or turn off `max-rounds`; a
//! human is never blocked from acting on it, only from being handed it by accident.

use super::{err, Connection, Result, Store};
use rusqlite::OptionalExtension;

/// Longest accepted `max-rounds` value. Rework loops this long are not realistic; the bound
/// exists so a typo (`max-rounds 999999999`) fails loudly instead of parsing as "never".
pub const MAX_MAX_ROUNDS: i64 = 999;

/// The board's `max-rounds`, read inside any transaction (`tb next` / `tb next --review` /
/// `Store::transition` all need the value as it stands in their own transaction, not a second
/// connection's view of it).
pub(super) fn max_rounds_of(conn: &Connection) -> Result<Option<i64>> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM config WHERE key='max-rounds'", [], |r| r.get(0)).optional()?;
    Ok(v.and_then(|s| s.parse::<i64>().ok()))
}

/// Sent back more times than the cap allows: `round` is 1 plus the number of `returned`
/// events, so "sent back more than N times" is `round - 1 > n`. Never true without a cap, and
/// never true for a `done` card — a finished card is not "still looping", whatever its history.
pub fn escalate_of(round: i64, column: &str, max_rounds: Option<i64>) -> bool {
    column != "done" && max_rounds.is_some_and(|n| round - 1 > n)
}

/// `escalate_of`, reading `max-rounds` and the round count itself inside a transaction — for
/// `tb next` / `tb next --review`'s auto-pick filter, which needs one card at a time and must
/// see the cap and the card exactly as this transaction does.
pub(super) fn is_escalated(conn: &Connection, id: i64, column: &str, max_rounds: Option<i64>) -> Result<bool> {
    if max_rounds.is_none() || column == "done" {
        return Ok(false);
    }
    let returned: i64 =
        conn.query_row("SELECT COUNT(*) FROM events WHERE card_id=? AND kind='returned'", [id], |r| r.get(0))?;
    Ok(escalate_of(returned + 1, column, max_rounds))
}

impl Store {
    /// The board's `max-rounds` cap; None = uncapped (the default — no card ever escalates).
    pub fn max_rounds(&self) -> Result<Option<i64>> {
        max_rounds_of(&self.conn)
    }

    /// `Some(n)` sets the cap (1-`MAX_MAX_ROUNDS`); `None` clears it (uncapped again).
    pub fn set_max_rounds(&self, n: Option<i64>) -> Result<()> {
        match n {
            None => {
                self.conn.execute("DELETE FROM config WHERE key='max-rounds'", [])?;
                Ok(())
            }
            Some(n) if (1..=MAX_MAX_ROUNDS).contains(&n) => self.set_config("max-rounds", &n.to_string()),
            Some(n) => err(format!(
                "max-rounds must be 1-{MAX_MAX_ROUNDS}, got {n} — try 'tb config max-rounds 5'"
            )),
        }
    }

    /// `max-rounds` for the `tb config` listing — listed only once the board sets it, so a
    /// board that sets nothing lists exactly what it always did.
    pub fn rounds_settings(&self) -> Result<Vec<(String, String)>> {
        Ok(match self.max_rounds()? { Some(n) => vec![("max-rounds".to_string(), n.to_string())], None => Vec::new() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escalate_is_sent_back_more_times_than_the_cap_allows() {
        // max-rounds 5: rounds r1..r6 (0..5 send-backs) are fine; the 6th send-back (r7) escalates
        for (round, want) in [(1, false), (2, false), (5, false), (6, false), (7, true), (8, true)] {
            assert_eq!(escalate_of(round, "review", Some(5)), want, "round {round}");
        }
        // uncapped: never escalate, whatever the round
        assert!(!escalate_of(50, "doing", None));
        // a finished card is never escalate, whatever its history
        assert!(!escalate_of(50, "done", Some(1)));
    }

    #[test]
    fn max_rounds_config_round_trips_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("b.db")).unwrap();
        assert_eq!(s.max_rounds().unwrap(), None, "uncapped by default");
        assert!(s.rounds_settings().unwrap().is_empty(), "unlisted while unset");
        s.set_max_rounds(Some(5)).unwrap();
        assert_eq!(s.max_rounds().unwrap(), Some(5));
        assert_eq!(s.rounds_settings().unwrap(), [("max-rounds".to_string(), "5".to_string())]);
        s.set_max_rounds(None).unwrap();
        assert_eq!(s.max_rounds().unwrap(), None);
        assert!(s.rounds_settings().unwrap().is_empty(), "cleared = unlisted again");
        for bad in [0, -1, MAX_MAX_ROUNDS + 1] {
            let e = s.set_max_rounds(Some(bad)).unwrap_err().to_string();
            assert!(e.starts_with("max-rounds must be 1-999"), "{bad}: {e}");
        }
    }
}
