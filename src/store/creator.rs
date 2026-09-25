//! Who made a board (#137): one row in the board's own file, written once, when tb creates
//! that file — by `tb new` or by the first command that creates a board on first use.
//!
//! - **One place writes it:** `Store::record_creator`, which both creating paths call. It is
//!   `INSERT OR IGNORE` on a table that holds at most one row, so two processes creating the
//!   same board at once still leave one creator (the first), and a second call is a no-op.
//! - **The same identity as a card's actor** (`store::actors`): the name as `--as`/`TB_AS`
//!   resolve it, then harness, model, role, session and host exactly as `actors::current`
//!   reads them — a person in a plain terminal records a name and a time, nothing else.
//! - **It lives in the board file**, so an archived board (a moved file) keeps it.
//! - **A board made before this existed has no row.** It may still be named in the fleet's
//!   `board-creations.log` (`time<TAB>board<TAB>key=value…`); `from_log` reads it from there,
//!   marked `source: "log"`. Nothing is written back: the log is a claim about the past, the
//!   row is what tb saw.

use super::{actors, Result, Store};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

/// Who created a board, and when (`at`, RFC 3339 UTC). `source` says where the answer came
/// from: `"board"` (the board's own record) or `"log"` (`board-creations.log`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Creator {
    pub actor: String,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub role: Option<String>,
    pub session: Option<String>,
    pub host: Option<String>,
    pub at: String,
    pub source: &'static str,
}

impl Creator {
    /// `maker (claude-code opus coder, session s1 on box) 2026-09-25T10:06:25Z` — unknown
    /// fields are left out.
    pub fn line(&self) -> String {
        let what: Vec<&str> = [&self.harness, &self.model, &self.role].iter().filter_map(|v| v.as_deref()).collect();
        let mut detail = what.join(" ");
        if let Some(s) = &self.session {
            detail = if detail.is_empty() { format!("session {s}") } else { format!("{detail}, session {s}") };
        }
        if let Some(h) = &self.host {
            detail = if detail.is_empty() { format!("on {h}") } else { format!("{detail} on {h}") };
        }
        let from = if self.source == "log" { " (from board-creations.log)" } else { "" };
        if detail.is_empty() {
            format!("{} {}{from}", self.actor, self.at)
        } else {
            format!("{} ({detail}) {}{from}", self.actor, self.at)
        }
    }
}

/// The table: at most one row (`id` is always 1). Idempotent and additive, like
/// `actors::migrate`: an existing board gets an empty table and nothing is back-filled.
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS board_creator (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            actor TEXT NOT NULL,
            harness TEXT,
            model TEXT,
            role TEXT,
            session TEXT,
            host TEXT,
            created_at INTEGER NOT NULL
        );",
    )?;
    Ok(())
}

fn rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| ts.to_string())
}

/// The board's own record, from any connection (a read-only one on an archived file
/// included). A file made before the table existed has no record: `None`, never an error.
pub fn read(conn: &Connection) -> Option<Creator> {
    conn.query_row(
        "SELECT actor, harness, model, role, session, host, created_at FROM board_creator WHERE id = 1",
        [],
        |r| {
            Ok(Creator {
                actor: r.get(0)?,
                harness: r.get(1)?,
                model: r.get(2)?,
                role: r.get(3)?,
                session: r.get(4)?,
                host: r.get(5)?,
                at: rfc3339(r.get(6)?),
                source: "board",
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// The creator `log` names for `board`: the last line whose second field is the board (a
/// name made again later was made by whoever made it last). Fields are `key=value`; `-` and
/// empty values are unknown. Every value is cleaned the way an identity field is.
pub fn from_log_text(log: &str, board: &str) -> Option<Creator> {
    let line = log.lines().rfind(|l| l.split('\t').nth(1).map(str::trim) == Some(board))?;
    let mut parts = line.split('\t');
    let time = parts.next()?.trim();
    parts.next();
    let mut c = Creator {
        actor: String::new(),
        harness: None,
        model: None,
        role: None,
        session: None,
        host: None,
        at: chrono::DateTime::parse_from_rfc3339(time)
            .map(|t| t.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_else(|_| crate::text::sanitize(time)),
        source: "log",
    };
    for kv in parts {
        let Some((k, v)) = kv.split_once('=') else { continue };
        let v = v.trim();
        if v.is_empty() || v == "-" {
            continue;
        }
        let v = if k.trim() == "session" { actors::session_token(v) } else { actors::field(v) };
        match k.trim() {
            "actor" => c.actor = v.unwrap_or_default(),
            "harness" => c.harness = v,
            "model" => c.model = v,
            "role" => c.role = v,
            "session" => c.session = v,
            "host" => c.host = v,
            _ => {}
        }
    }
    (!c.actor.is_empty()).then_some(c)
}

/// `from_log_text` over the file at `path`; `None` when it cannot be read.
pub fn from_log(path: &Path, board: &str) -> Option<Creator> {
    from_log_text(&std::fs::read_to_string(path).ok()?, board)
}

impl Store {
    /// Did THIS open create the board file? (Only then does `record_creator` have anything
    /// true to say.)
    pub fn was_created(&self) -> bool {
        self.created
    }

    /// Record `actor`, with this process's identity (`actors::current`), as the board's
    /// creator — THE one write of a creator: `tb new` and create-on-first-use both call it.
    /// Keeps the first record there is; a later call changes nothing.
    pub fn record_creator(&self, actor: &str) -> Result<()> {
        let who = actors::current();
        self.conn.execute(
            "INSERT OR IGNORE INTO board_creator(id, actor, harness, model, role, session, host, created_at)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?)",
            params![actor, who.harness, who.model, who.role, who.session, who.host, super::now()],
        )?;
        Ok(())
    }

    /// The board's creator: its own record, else what `board-creations.log` says.
    pub fn creator(&self) -> Option<Creator> {
        read(&self.conn).or_else(|| from_log(&crate::boards::creations_log(), &self.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_line_is_read_with_its_fields_and_the_last_line_wins() {
        let log = "2026-09-18T18:52:20Z\tdefault\tactor=chick\tsession=-\thost=chick\tworkspace=-\n\
                   2026-09-19T09:54:57Z\tllm\tactor=orch-llm\tsession=s-1\thost=box.lan\n\
                   2026-09-20T01:00:00Z\tllm\tactor=later\tsession=-\thost=-\n";
        let c = from_log_text(log, "default").unwrap();
        assert_eq!((c.actor.as_str(), c.session.as_deref(), c.host.as_deref()), ("chick", None, Some("chick")));
        assert_eq!((c.at.as_str(), c.source), ("2026-09-18T18:52:20Z", "log"));
        assert_eq!(from_log_text(log, "llm").unwrap().actor, "later");
        assert!(from_log_text(log, "nope").is_none());
    }

    #[test]
    fn the_first_record_is_kept() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        assert!(read(&s.conn).is_none());
        s.record_creator("first").unwrap();
        s.record_creator("second").unwrap();
        let c = read(&s.conn).unwrap();
        assert_eq!((c.actor.as_str(), c.source), ("first", "board"));
    }
}
