//! The holder rule for changes that are not column moves, and soft delete.
//!
//! **The holder rule.** A card in DOING with an owner is HELD. `done`/`drop`/`move` already
//! refuse to take a held card away from its holder (`Store::transition`); `rm`, `edit`,
//! `block`, `check` and `prio` follow the very same rule here — refused for anyone else,
//! `--force` goes through and is logged as its own event. Card ids are small shared integers,
//! and an off-by-one must not delete, rewrite, re-tick or reorder someone else's work in
//! progress. `note` stays open on purpose: it adds to a card, it does not take it over.
//!
//! **Soft delete** (`tb config rm archive`). By default `tb rm` destroys a card with its
//! checklist and history, as it always has. On an archive board it moves them, whole, into
//! one table, `archived_cards`, and `tb restore ID` moves them back under the same id with
//! every event as it was. The table is created the first time a board asks for it, so a
//! board that sets nothing keeps exactly the schema it had. Archived cards live OUTSIDE
//! `cards`: no list, count, WIP limit, `next`, sync or render can see them by accident, and
//! none of those queries had to change. Rows are stored as JSON objects keyed by column
//! name, so a column another part of tb adds to `cards` later is archived and restored
//! without this file knowing about it.

use super::{Code, bottom_of, err, get_card, now, ownership_err, wip_full_err, wip_of, BoardError, Card, Result, Store};
use rusqlite::{params, types::ValueRef, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

/// `rm` when the board does not set it.
pub const RM_DELETE: &str = "delete";
pub const RM_ARCHIVE: &str = "archive";

const ARCHIVE_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS archived_cards (
    card_id INTEGER PRIMARY KEY,
    archived_at INTEGER NOT NULL,
    archived_by TEXT NOT NULL,
    title TEXT NOT NULL,
    tag TEXT,
    "column" TEXT NOT NULL,
    owner TEXT,
    card TEXT NOT NULL,
    checklist TEXT NOT NULL,
    events TEXT NOT NULL,
    links TEXT NOT NULL DEFAULT '[]'
);
"#;

/// Add `links` to an `archived_cards` table created before links existed (the `reviewer` /
/// `blocked_on` pattern: a board written by an older tb and one made fresh end up the same
/// shape). A board that never archived has no table yet, so this is a no-op until it does.
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    let has_table: i64 =
        conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='archived_cards'", [], |r| r.get(0))?;
    if has_table == 0 {
        return Ok(());
    }
    let has_links: i64 =
        conn.query_row("SELECT COUNT(*) FROM pragma_table_info('archived_cards') WHERE name='links'", [], |r| r.get(0))?;
    if has_links == 0 {
        match conn.execute_batch("ALTER TABLE archived_cards ADD COLUMN links TEXT NOT NULL DEFAULT '[]'") {
            Err(e) if e.to_string().contains("duplicate column") => {}
            Err(e) => return Err(e.into()),
            Ok(()) => {}
        }
    }
    Ok(())
}

/// What `remove_card` did.
#[derive(Debug, Clone)]
pub struct Removed {
    /// The card as it was.
    pub card: Card,
    /// true: moved to the archive (`tb restore` brings it back). false: destroyed.
    pub archived: bool,
}

/// One row of `tb list --archived`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArchivedCard {
    pub id: i64,
    pub title: String,
    pub tag: Option<String>,
    /// The column it was in (and returns to).
    pub column: String,
    pub owner: Option<String>,
    pub archived_at: i64,
    pub archived_by: String,
}

/// The holder of `c` when that is someone other than `actor`: the owner of a DOING card.
/// (The same test `transition` applies to a move out of DOING.)
fn held_by_other<'a>(c: &'a Card, actor: &str) -> Option<&'a str> {
    if c.column != "doing" || actor == "github" {
        return None;
    }
    c.owner.as_deref().filter(|o| !o.eq_ignore_ascii_case(actor))
}

/// Refuse a change to a card someone else holds, unless forced. `Ok(Some(owner))`: forced
/// past that holder — the caller logs it. `what` finishes "to … anyway" (`delete it`).
pub(crate) fn holder_guard(tx: &Connection, c: &Card, actor: &str, force: bool, what: &str) -> Result<Option<String>> {
    match held_by_other(c, actor) {
        None => Ok(None),
        Some(owner) if force => Ok(Some(owner.to_string())),
        Some(owner) => Err(ownership_err(tx, c.id, owner, actor, what)?),
    }
}

fn has_archive(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='archived_cards'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Every row of a query as a JSON object, column name → value, in row order.
fn rows_json(conn: &Connection, sql: &str, id: i64) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
    let mut st = conn.prepare(sql)?;
    let names: Vec<String> = st.column_names().iter().map(|n| n.to_string()).collect();
    let mut rows = st.query([id])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut m = serde_json::Map::new();
        for (i, name) in names.iter().enumerate() {
            let v = match r.get_ref(i)? {
                ValueRef::Null => serde_json::Value::Null,
                ValueRef::Integer(n) => serde_json::json!(n),
                ValueRef::Real(f) => serde_json::json!(f),
                ValueRef::Text(t) => serde_json::json!(String::from_utf8_lossy(t)),
                // tb stores no blobs; one written by something else has no JSON form
                ValueRef::Blob(_) => serde_json::Value::Null,
            };
            m.insert(name.clone(), v);
        }
        out.push(m);
    }
    Ok(out)
}

/// Insert a JSON object as a row of `table`: the columns the row has AND the table still has.
fn insert_json(conn: &Connection, table: &str, row: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
    let known: Vec<String> = {
        let mut st = conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?;
        let v = st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let cols: Vec<&String> = row.keys().filter(|k| known.contains(k)).collect();
    let names = cols.iter().map(|c| format!("\"{}\"", c.replace('"', "\"\""))).collect::<Vec<_>>().join(", ");
    let marks = vec!["?"; cols.len()].join(", ");
    let values: Vec<rusqlite::types::Value> = cols
        .iter()
        .map(|c| match &row[*c] {
            serde_json::Value::Null => rusqlite::types::Value::Null,
            serde_json::Value::Bool(b) => rusqlite::types::Value::Integer(*b as i64),
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) => rusqlite::types::Value::Integer(i),
                None => rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0)),
            },
            serde_json::Value::String(s) => rusqlite::types::Value::Text(s.clone()),
            other => rusqlite::types::Value::Text(other.to_string()),
        })
        .collect();
    conn.execute(
        &format!("INSERT INTO {table} ({names}) VALUES ({marks})"),
        rusqlite::params_from_iter(values.iter()),
    )?;
    Ok(())
}

fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

pub(crate) fn board_log(conn: &Connection, actor: &str, kind: &str, text: &str) -> Result<()> {
    Store::log_board(conn, actor, kind, text)
}

impl Store {
    /// `rm`: `delete` (the default — today's behaviour) or `archive`.
    pub fn rm_mode(&self) -> Result<String> {
        let v: Option<String> =
            self.conn.query_row("SELECT value FROM config WHERE key='rm'", [], |r| r.get(0)).optional()?;
        Ok(if v.as_deref() == Some(RM_ARCHIVE) { RM_ARCHIVE } else { RM_DELETE }.to_string())
    }

    /// `tb config rm delete|archive`. `archive` creates the archive table (once); going back to
    /// `delete` removes the setting and keeps whatever is archived, still restorable.
    pub fn set_rm_mode(&self, value: &str, actor: &str) -> Result<String> {
        let v = value.trim().to_ascii_lowercase();
        let old = self.rm_mode()?;
        match v.as_str() {
            RM_ARCHIVE => {
                self.conn.execute_batch(ARCHIVE_TABLE)?;
                self.conn.execute(
                    "INSERT INTO config(key, value) VALUES ('rm', 'archive') ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    [],
                )?;
            }
            RM_DELETE => {
                self.conn.execute("DELETE FROM config WHERE key='rm'", [])?;
            }
            _ => return err(format!("'{value}' is not delete|archive — try 'tb config rm archive'"), Code::InvalidValue),
        }
        if old != v {
            board_log(&self.conn, actor, "rm", &format!("rm {old} -> {v}"))?;
        }
        Ok(v)
    }

    /// The `rm` row of `tb config`: listed once the board sets it, so a board that sets
    /// nothing lists exactly what it always did.
    pub(super) fn rm_settings(&self) -> Result<Vec<(String, String)>> {
        Ok(if self.rm_mode()? == RM_ARCHIVE { vec![("rm".to_string(), RM_ARCHIVE.to_string())] } else { Vec::new() })
    }

    /// The holder rule for `edit` and `block` (and anything else that rewrites or reorders a
    /// card without moving it): refused when someone else holds the card, unless forced.
    /// `what` finishes "to … anyway" in the refusal (`edit it`, `tick it`). `Ok(Some(owner))`:
    /// forced past that holder — once the change has gone through, record it with `log_forced`.
    pub fn holder_check(&self, id: i64, actor: &str, force: bool, what: &str) -> Result<Option<String>> {
        let c = get_card(&self.conn, id)?;
        holder_guard(&self.conn, &c, actor, force, what)
    }

    /// A forced change to a held card, as its own `force` event: `<did> #ID held by OWNER`.
    pub fn log_forced(&self, id: i64, actor: &str, did: &str, owner: &str) -> Result<()> {
        Self::log(&self.conn, id, actor, "force", &format!("{did} #{id} held by {owner}"))
    }

    /// Who holds card `id`, when that is someone other than `actor` (for a prompt to name).
    pub fn held_by_other(&self, id: i64, actor: &str) -> Result<Option<String>> {
        Ok(held_by_other(&self.card(id)?, actor).map(str::to_string))
    }

    /// `tb rm` and the delete key: follows the holder rule, then destroys the card (the
    /// default) or, on an archive board, moves it to the archive with its checklist and
    /// every event. One transaction. Logged on the board; a forced removal says so there
    /// too, because a destroyed card takes its own events with it.
    pub fn remove_card(&mut self, id: i64, actor: &str, force: bool) -> Result<Removed> {
        let archive = self.rm_mode()? == RM_ARCHIVE;
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // a DELETE writes no card event (the card's events go with it), so it would never
        // reach the check in `Store::log`: ask here, like the other hand-written writers
        crate::store::access::guard_actor(&tx, actor)?;
        let c = get_card(&tx, id)?;
        let forced = holder_guard(&tx, &c, actor, force, if archive { "archive it" } else { "delete it" })?;
        let verb = if archive { "archived" } else { "deleted" };
        if let Some(owner) = &forced {
            Self::log(&tx, id, actor, "force", &format!("{verb} #{id} held by {owner}"))?;
            board_log(&tx, actor, "force", &format!("{verb} #{id} held by {owner}"))?;
        }
        if archive {
            tx.execute_batch(ARCHIVE_TABLE)?;
            Self::log(&tx, id, actor, "archived", "")?;
            let card = rows_json(&tx, "SELECT * FROM cards WHERE id=?", id)?.into_iter().next().unwrap_or_default();
            let checklist = rows_json(&tx, "SELECT * FROM checklist WHERE card_id=? ORDER BY idx", id)?;
            let events = rows_json(&tx, "SELECT * FROM events WHERE card_id=? ORDER BY id", id)?;
            let links = rows_json(&tx, "SELECT * FROM links WHERE card_id=? ORDER BY idx", id)?;
            tx.execute(
                r#"INSERT INTO archived_cards(card_id, archived_at, archived_by, title, tag, "column", owner, card, checklist, events, links)
                   VALUES (?,?,?,?,?,?,?,?,?,?,?)"#,
                params![id, now(), actor, c.title, c.tag, c.column, c.owner, to_json(&card), to_json(&checklist), to_json(&events), to_json(&links)],
            )?;
        }
        tx.execute("DELETE FROM checklist WHERE card_id=?", [id])?;
        tx.execute("DELETE FROM links WHERE card_id=?", [id])?;
        tx.execute("DELETE FROM events WHERE card_id=?", [id])?;
        tx.execute("DELETE FROM cards WHERE id=?", [id])?;
        let kind = if archive { "archive" } else { "delete" };
        board_log(&tx, actor, kind, &format!("{verb} #{id} \"{}\"", c.title))?;
        tx.commit()?;
        Ok(Removed { card: c, archived: archive })
    }

    /// The archived cards, most recently archived first. A board that never archived has none.
    pub fn archived(&self) -> Result<Vec<ArchivedCard>> {
        if !has_archive(&self.conn)? {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare(
            r#"SELECT card_id, title, tag, "column", owner, archived_at, archived_by FROM archived_cards
               ORDER BY archived_at DESC, card_id DESC"#,
        )?;
        let v = st
            .query_map([], |r| {
                Ok(ArchivedCard {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    tag: r.get(2)?,
                    column: r.get(3)?,
                    owner: r.get(4)?,
                    archived_at: r.get(5)?,
                    archived_by: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// `tb restore ID`: the card comes back under its id, in the column it was in, with its
    /// owner, checklist and every event exactly as they were, at the bottom of that column —
    /// plus one `restored` event. A card that was in DOING needs a free slot (the WIP limit).
    pub fn restore(&mut self, id: i64, actor: &str) -> Result<Card> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: Option<(String, String, String, String, String)> = if has_archive(&tx)? {
            tx.query_row(
                r#"SELECT card, checklist, events, "column", links FROM archived_cards WHERE card_id=?"#,
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?
        } else {
            None
        };
        let Some((card, checklist, events, column, links)) = row else {
            return err(format!("no archived card #{id} — see 'tb list --archived'"), Code::NoCard);
        };
        if tx.query_row("SELECT COUNT(*) FROM cards WHERE id=?", [id], |r| r.get::<_, i64>(0))? > 0 {
            return err(format!("a card #{id} is on the board already — see 'tb show {id}'"), Code::Unknown);
        }
        if column == "doing" {
            let (wip, doing) = (
                wip_of(&tx)?,
                tx.query_row(r#"SELECT COUNT(*) FROM cards WHERE "column"='doing'"#, [], |r| r.get::<_, i64>(0))?,
            );
            if doing >= wip {
                return Err(wip_full_err(&tx, doing, wip, actor));
            }
        }
        let broken = |what: &str| BoardError(format!("the archived {what} of #{id} cannot be read — nothing was restored; see 'tb list --archived'"), Code::DbError);
        let mut card: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&card).map_err(|_| broken("card"))?;
        let checklist: Vec<serde_json::Map<String, serde_json::Value>> =
            serde_json::from_str(&checklist).map_err(|_| broken("checklist"))?;
        let events: Vec<serde_json::Map<String, serde_json::Value>> =
            serde_json::from_str(&events).map_err(|_| broken("history"))?;
        // a row archived before links existed has the column's default '[]' — an empty list,
        // never a parse failure
        let links: Vec<serde_json::Map<String, serde_json::Value>> =
            serde_json::from_str(&links).map_err(|_| broken("links"))?;
        card.insert("position".into(), serde_json::json!(bottom_of(&tx, &column)?));
        insert_json(&tx, "cards", &card)?;
        for item in &checklist {
            insert_json(&tx, "checklist", item)?;
        }
        for e in &events {
            insert_json(&tx, "events", e)?;
        }
        for l in &links {
            insert_json(&tx, "links", l)?;
        }
        tx.execute("DELETE FROM archived_cards WHERE card_id=?", [id])?;
        Self::log(&tx, id, actor, "restored", "")?;
        let c = get_card(&tx, id)?;
        board_log(&tx, actor, "restore", &format!("restored #{id} \"{}\"", c.title))?;
        tx.commit()?;
        Ok(c)
    }
}
