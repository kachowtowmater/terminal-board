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
//! **The source is held for the WHOLE move.** Reading the card, writing the far end and
//! removing the near one all happen inside ONE `BEGIN IMMEDIATE` on the source, so the source
//! holds its write lock from the first read to the last delete. That is not tidiness — without
//! it there is a window as wide as the destination's whole transaction in which `tb note` and
//! `tb edit` on the source succeed, tell the person "noted #1", and are then thrown away by the
//! delete. Any write that arrives during a move now waits for it and then finds the card gone
//! (`no card #1`, non-zero) instead of being acknowledged and destroyed, and a second `tb mv`
//! of the same card waits, then refuses the same way rather than making a second copy.
//!
//! **The id changes.** Ids are per board (`AUTOINCREMENT`), so the card cannot keep its
//! number — another card may already hold it. The new id is printed and returned, the
//! destination's first event records where the card came from (`moved-in #OLD from BOARD`),
//! and the source keeps a `moved-out #NEW to BOARD` event on the board log, so both ends say
//! where it went even though the card itself is gone from one of them.
//!
//! **What travels:** the card, its checklist and its whole history, with the original actor
//! and timestamp of every event — and the identity behind each of those actors (`actors`
//! rows, re-keyed to the ids the destination gives them), so a history that names its
//! sessions still names them on the board it lands on. What does NOT travel: the owner and
//! the column — a card lands in TODO, unowned, because the destination has its own WIP limit
//! and its own people, and because a card held by somebody on one board cannot be held by
//! them on a board they may not be working. A block that names another card (`--on #7`) is
//! kept as TEXT but its `--on` is dropped: `#7` means a different card over there, and a
//! link that silently points at the wrong card is worse than no link.
//!
//! **Known limit (issue #112).** tb has no lock on a board file's lifetime yet, so nothing
//! stops another process moving or replacing the destination file between the open and the
//! commit. `tb mv` does not make that worse — it never creates, moves or replaces a board
//! file, and it REFUSES a destination that does not exist rather than creating one, which is
//! the path that would resurrect a board somebody had just retired. Once #112 lands, this
//! should take the shared lock on both boards and the duplicate window closes.

use super::archive::board_log;
use super::links::LinkItem;
use super::{get_card, now, Card, Code, Result, Store};
use rusqlite::{params, TransactionBehavior};
use std::collections::HashMap;

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
    /// The evidence links that travelled with it (`store::links`).
    pub links: usize,
}

/// One checklist item as it travels: its number, its text, whether it is ticked.
type Item = (i64, String, bool);

/// Everything about a card that travels to another board.
type Packed = (Card, Vec<Item>, Vec<Past>, Vec<LinkItem>, Vec<IdentityRow>);

/// One event as it sits in the source, so it can be written again at the far end with its
/// original actor and time.
struct Past {
    ts: i64,
    actor: String,
    kind: String,
    text: String,
    actor_id: Option<i64>,
    /// The structured holder of an `assigned` event (#111) — carried over so the self-approval
    /// guard on the destination board never has to fall back to parsing `text`, the way a
    /// pre-migration row does.
    assignee: Option<String>,
}

/// One identity row as it travels: the destination keeps the same record behind the name,
/// under its own id (`receive_identities`, below).
struct IdentityRow {
    id: i64,
    actor: String,
    who: super::actors::Identity,
    first_seen: i64,
    last_seen: i64,
}

/// Everything about card `id` that travels to another board, read through the transaction
/// that holds the source — never through the bare connection, or the read would not be
/// covered by the lock that makes the move safe.
fn pack(tx: &rusqlite::Transaction, id: i64) -> Result<Packed> {
    let card = get_card(tx, id)?;
    let mut st = tx.prepare("SELECT idx, text, done FROM checklist WHERE card_id=? ORDER BY idx")?;
    let checklist = st
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut st = tx.prepare("SELECT ts, actor, kind, text, actor_id, assignee FROM events WHERE card_id=? ORDER BY ts, id")?;
    let events = st
        .query_map([id], |r| {
            Ok(Past { ts: r.get(0)?, actor: r.get(1)?, kind: r.get(2)?, text: r.get(3)?, actor_id: r.get(4)?, assignee: r.get(5)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut st = tx.prepare("SELECT idx, label, value, added_by, added_at FROM links WHERE card_id=? ORDER BY idx")?;
    let links = st.query_map([id], super::links::row_link)?.collect::<rusqlite::Result<Vec<_>>>()?;
    // the identities behind the history, read whole: the destination inserts-or-finds each of
    // them under its own id, or every `actor_id` copied over would point at a row it does not
    // have (the FOREIGN KEY refusal this file's bug report is about)
    let identities = identities_of(tx, &events)?;
    Ok((card, checklist, events, links, identities))
}

/// The `actors` rows behind `events.actor_id`, one per distinct id, in id order. Read from
/// the same transaction as the events, so a concurrent write cannot split the two.
fn identities_of(tx: &rusqlite::Transaction, events: &[Past]) -> Result<Vec<IdentityRow>> {
    let mut ids: Vec<i64> = events.iter().filter_map(|e| e.actor_id).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut st = tx.prepare(&format!(
        "SELECT id, actor, harness, model, role, session, host, first_seen, last_seen FROM actors WHERE id IN ({})",
        vec!["?"; ids.len()].join(",")
    ))?;
    let rows = st
        .query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok(IdentityRow {
                id: r.get(0)?,
                actor: r.get(1)?,
                who: super::actors::Identity {
                    harness: r.get(2)?,
                    model: r.get(3)?,
                    role: r.get(4)?,
                    session: r.get(5)?,
                    host: r.get(6)?,
                },
                first_seen: r.get(7)?,
                last_seen: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Insert-or-find each packed identity in the destination and map the source id to the id the
/// row has HERE — matched on the identity (`actors::upsert`, written exactly as the unique index
/// `actors_identity` is), never on the id, which is per board. The row keeps the span the
/// history really had: `last_seen` from the upsert, `first_seen` pulled back if the source saw
/// the identity earlier than the destination did.
fn receive_identities(conn: &rusqlite::Connection, rows: &[IdentityRow]) -> Result<HashMap<i64, i64>> {
    let mut map = HashMap::new();
    for row in rows {
        let id = super::actors::upsert(conn, &row.actor, &row.who, row.last_seen)?;
        conn.execute("UPDATE actors SET first_seen=? WHERE id=? AND first_seen>?", params![row.first_seen, id, row.first_seen])?;
        map.insert(row.id, id);
    }
    Ok(map)
}

impl Store {
    /// Write a card that came from another board into this one, in TODO at the bottom,
    /// unowned, with its checklist, its links and its whole history. Returns the id it has
    /// here.
    #[allow(clippy::too_many_arguments)]
    fn receive(
        &mut self,
        from: &str,
        card: &Card,
        checklist: &[Item],
        events: &[Past],
        links: &[LinkItem],
        identities: &[IdentityRow],
        actor: &str,
        forced: Option<&str>,
    ) -> Result<i64> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // the board it ARRIVES at keeps its own list of names
        super::access::guard_actor(&tx, actor)?;
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
        // the evidence, with the original label, value and who attached it
        for l in links {
            tx.execute(
                "INSERT INTO links(card_id, idx, label, value, added_by, added_at) VALUES (?,?,?,?,?,?)",
                params![new_id, l.idx, l.label, l.value, l.added_by, l.added_at],
            )?;
        }
        // the history, with the actor and the time each event really had; `assignee` travels
        // too (#111), so an `assigned` event's holder is still read from a structured column
        // on the destination board, not silently dropped back to parsing `text`. Each
        // event's `actor_id` is rewritten to the row the identity has HERE, so the
        // destination's own `actors` records travel with the history instead of pointing off
        // the board.
        let ids = receive_identities(&tx, identities)?;
        for e in events {
            tx.execute(
                "INSERT INTO events(card_id, ts, actor, kind, text, actor_id, assignee) VALUES (?,?,?,?,?,?,?)",
                params![new_id, e.ts, e.actor, e.kind, e.text, e.actor_id.and_then(|id| ids.get(&id).copied()), e.assignee],
            )?;
        }
        // then the move itself, so the card says where it came from
        tx.execute(
            "INSERT INTO events(card_id, ts, actor, kind, text, actor_id) VALUES (?,?,?,'moved-in',?,NULL)",
            params![new_id, t, actor, format!("#{} from {from}", card.id)],
        )?;
        // a move over its holder's head is logged where the card now lives, because the row
        // it used to be is about to be deleted
        if let Some(owner) = forced {
            tx.execute(
                "INSERT INTO events(card_id, ts, actor, kind, text, actor_id) VALUES (?,?,?,'force',?,NULL)",
                params![new_id, t, actor, format!("moved #{} held by {owner} from {from}", card.id)],
            )?;
        }
        board_log(&tx, actor, "moved-in", &format!("#{} from {from} is #{new_id} here", card.id))?;
        tx.commit()?;
        Ok(new_id)
    }

    /// `tb mv ID --to BOARD`. `dest` is already open, and the caller has checked it is a
    /// different board that really exists.
    ///
    /// ONE transaction holds the SOURCE for the whole move — read, far-end write, delete — so
    /// no other write to the source can slip in, be acknowledged, and then be deleted (see the
    /// module notes). Inside it the destination is written and committed FIRST; the source row
    /// goes only after that succeeded. If the source commit then fails, the card is on both
    /// boards and the error says which command removes the old one.
    pub fn move_to_board(&mut self, id: i64, dest: &mut Store, actor: &str, forced: Option<&str>) -> Result<Moved> {
        let from = self.name.clone();
        let to = dest.name.clone();
        // BEGIN IMMEDIATE: the source's write lock, taken before the card is even read and
        // held until the delete commits. A concurrent `tb note` waits here and then finds the
        // card gone; a second `tb mv` of the same card does the same, instead of copying twice.
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // this writer builds its events by hand (it replays the card's history with the
        // original actors), so it does not pass through `Store::log` and asks for itself
        super::access::guard_actor(&tx, actor)?;
        let (card, checklist, events, links, identities) = pack(&tx, id)?;
        let new_id = dest.receive(&from, &card, &checklist, &events, &links, &identities, actor, forced)?;
        // from here the card exists on the destination: a failure below leaves a duplicate,
        // never a hole
        let cleared = (|| -> Result<()> {
            tx.execute("DELETE FROM checklist WHERE card_id=?", [id])?;
            tx.execute("DELETE FROM links WHERE card_id=?", [id])?;
            tx.execute("DELETE FROM events WHERE card_id=?", [id])?;
            tx.execute("DELETE FROM cards WHERE id=?", [id])?;
            board_log(&tx, actor, "moved-out", &format!("#{id} to {to} is #{new_id} there"))?;
            if let Some(owner) = forced {
                board_log(&tx, actor, "force", &format!("moved #{id} held by {owner} to {to}"))?;
            }
            tx.commit()?;
            Ok(())
        })();
        cleared.map_err(|e| {
            super::BoardError(format!(
                "#{id} was copied to '{to}' as #{new_id} but could not be removed from '{from}' ({e}) — the card is on BOTH boards; delete the old one with 'tb {from} rm {id}'"
            ), Code::DbError)
        })?;
        Ok(Moved {
            from_board: from,
            to_board: to,
            old_id: id,
            new_id,
            title: super::raw_title(&card),
            checklist: checklist.len(),
            events: events.len(),
            links: links.len(),
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
