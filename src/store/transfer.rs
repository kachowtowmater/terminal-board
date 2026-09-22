//! A card moved to another board: `tb mv ID --to BOARD`.
//!
//! This is the only thing in tb that writes to TWO board files, and each file gets its own
//! transaction — SQLite has no transaction across databases here. So the ORDER is the design:
//!
//! 1. the card is written to the destination and **committed**;
//! 2. only then is it removed from the source.
//!
//! A crash, a kill or a full disk between the two leaves the card on BOTH boards. That is a
//! duplicate somebody can see and delete. The other order would lose the card outright, and a
//! lost card is not recoverable. When the second step does fail, the refusal says exactly
//! that: the card is on the destination, still on the source, and which command removes it.
//!
//! **The id changes.** Ids are per board (`AUTOINCREMENT`), so the card cannot keep its
//! number — another card may already hold it. The new id is printed and returned, the
//! destination's first event records where the card came from (`moved-in #OLD from BOARD`),
//! and the source keeps a `moved-out #NEW to BOARD` event on the board log, so both ends say
//! where it went even though the card itself is gone from one of them.
//!
//! **What travels:** the card, its checklist and its whole history, with the original actor
//! and timestamp of every event. What does NOT travel: the owner and the column — a card
//! lands in TODO, unowned, because the destination has its own WIP limit and its own people,
//! and because a card held by somebody on one board cannot be held by them on a board they
//! may not be working. A block that names another card (`--on #7`) is kept as TEXT but its
//! `--on` is dropped: `#7` means a different card over there, and a link that silently points
//! at the wrong card is worse than no link.
//!
//! **Known limit (issue #112).** tb has no lock on a board file's lifetime yet, so nothing
//! stops another process moving or replacing the destination file between the open and the
//! commit. `tb mv` does not make that worse — it never creates, moves or replaces a board
//! file, and it REFUSES a destination that does not exist rather than creating one, which is
//! the path that would resurrect a board somebody had just retired. Once #112 lands, this
//! should take the shared lock on both boards and the duplicate window closes.

use super::archive::board_log;
use super::{get_card, now, Card, Result, Store};
use rusqlite::{params, TransactionBehavior};

/// What a move did: where the card came from, and the number it has now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub from_board: String,
    pub to_board: String,
    pub old_id: i64,
    pub new_id: i64,
    pub title: String,
    /// The checklist items and events that travelled with it.
    pub checklist: usize,
    pub events: usize,
}

/// One checklist item as it travels: its number, its text, whether it is ticked.
type Item = (i64, String, bool);

/// Everything about a card that travels to another board.
type Packed = (Card, Vec<Item>, Vec<Past>);

/// One event as it sits in the source, so it can be written again at the far end with its
/// original actor and time.
struct Past {
    ts: i64,
    actor: String,
    kind: String,
    text: String,
    actor_id: Option<i64>,
}

impl Store {
    /// Everything about card `id` that travels to another board.
    fn packed(&self, id: i64) -> Result<Packed> {
        let card = get_card(&self.conn, id)?;
        let mut st = self.conn.prepare("SELECT idx, text, done FROM checklist WHERE card_id=? ORDER BY idx")?;
        let checklist = st
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut st = self
            .conn
            .prepare("SELECT ts, actor, kind, text, actor_id FROM events WHERE card_id=? ORDER BY ts, id")?;
        let events = st
            .query_map([id], |r| {
                Ok(Past { ts: r.get(0)?, actor: r.get(1)?, kind: r.get(2)?, text: r.get(3)?, actor_id: r.get(4)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((card, checklist, events))
    }

    /// Write a card that came from another board into this one, in TODO at the bottom,
    /// unowned, with its checklist and its whole history. Returns the id it has here.
    fn receive(&mut self, from: &str, card: &Card, checklist: &[Item], events: &[Past], actor: &str) -> Result<i64> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let pos: i64 =
            tx.query_row(r#"SELECT COALESCE(MAX(position), -1) + 1 FROM cards WHERE "column"='todo'"#, [], |r| r.get(0))?;
        let t = now();
        // the column and the owner do NOT travel: a board has its own WIP limit and its own
        // people. The `--on` of a block does not either — `#7` is a different card here.
        tx.execute(
            r#"INSERT INTO cards(title, tag, description, "column", owner, due, gh_ref, blocked, blocked_until,
                                 created_at, column_since, position)
               VALUES (?,?,?,'todo',NULL,?,?,?,?,?,?,?)"#,
            params![
                card.title,
                card.tag,
                card.description,
                card.due,
                card.gh_ref,
                card.blocked,
                card.blocked_until,
                card.created_at,
                t,
                pos
            ],
        )?;
        let new_id = tx.last_insert_rowid();
        for (idx, text, done) in checklist {
            tx.execute(
                "INSERT INTO checklist(card_id, idx, text, done) VALUES (?,?,?,?)",
                params![new_id, idx, text, *done as i64],
            )?;
        }
        // the history, with the actor and the time each event really had
        for e in events {
            tx.execute(
                "INSERT INTO events(card_id, ts, actor, kind, text, actor_id) VALUES (?,?,?,?,?,?)",
                params![new_id, e.ts, e.actor, e.kind, e.text, e.actor_id],
            )?;
        }
        // then the move itself, so the card says where it came from
        tx.execute(
            "INSERT INTO events(card_id, ts, actor, kind, text, actor_id) VALUES (?,?,?,'moved-in',?,NULL)",
            params![new_id, t, actor, format!("#{} from {from}", card.id)],
        )?;
        board_log(&tx, actor, "moved-in", &format!("#{} from {from} is #{new_id} here", card.id))?;
        tx.commit()?;
        Ok(new_id)
    }

    /// Remove a card that has been written to another board, leaving a board-level note of
    /// where it went. The card's own events go with it — the record lives at the far end and
    /// on this board's log.
    fn released(&mut self, id: i64, to: &str, new_id: i64, actor: &str) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM checklist WHERE card_id=?", [id])?;
        tx.execute("DELETE FROM events WHERE card_id=?", [id])?;
        tx.execute("DELETE FROM cards WHERE id=?", [id])?;
        board_log(&tx, actor, "moved-out", &format!("#{id} to {to} is #{new_id} there"))?;
        tx.commit()?;
        Ok(())
    }

    /// `tb mv ID --to BOARD`. `dest` is already open, and the caller has checked it is a
    /// different board that really exists.
    ///
    /// The destination is written and committed FIRST; the source is cleared only after that
    /// succeeded. If the clearing fails the card is on both boards and the error says so.
    pub fn move_to_board(&mut self, id: i64, dest: &mut Store, actor: &str) -> Result<Moved> {
        let (card, checklist, events) = self.packed(id)?;
        let from = self.name.clone();
        let to = dest.name.clone();
        let new_id = dest.receive(&from, &card, &checklist, &events, actor)?;
        // from here the card exists on the destination: a failure below leaves a duplicate,
        // never a hole
        self.released(id, &to, new_id, actor).map_err(|e| {
            super::BoardError(format!(
                "#{id} was copied to '{to}' as #{new_id} but could not be removed from '{from}' ({e}) — the card is on BOTH boards; delete the old one with 'tb {from} rm {id}'"
            ))
        })?;
        Ok(Moved {
            from_board: from,
            to_board: to,
            old_id: id,
            new_id,
            title: super::raw_title(&card),
            checklist: checklist.len(),
            events: events.len(),
        })
    }

    /// Cards on this board that `owner` holds or is reviewing — what `--all-boards` collects
    /// per board.
    pub fn held_by(&self, owner: &str) -> Result<Vec<Card>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|c| {
                c.owner.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(owner))
                    || c.reviewer.as_deref().is_some_and(|r| r.eq_ignore_ascii_case(owner))
            })
            .collect())
    }
}
