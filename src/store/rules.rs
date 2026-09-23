//! A board's own conventions: `tb config rules "TEXT"` or `--file PATH`. Free text stored on
//! the board (config key `rules`), printed by `tb guide`, and shown once to each agent — the
//! first `tb next` since this text was last set or changed.
//!
//! **"Once per agent" needs no new table.** It is a `board_events` row, `kind='rules-seen'`,
//! `actor` the agent, `text` the rules text they were shown — the same place `wip`, `actors`
//! and every other setting change already logs itself (store/access.rs, store/closing.rs).
//! Comparing the stored TEXT, not a boolean, is what makes `tb config rules` "changed since
//! you last saw it" instead of "ever seen at all": edit the rules and every agent, including
//! one who saw the old wording, is shown the new text once more. It is scoped the same way
//! everything else on a board is — one `board_events` table per board file, so two boards
//! never share a "seen" mark — and it travels with the file exactly as the rest of the
//! board's history does: copy the `.db` and an agent who already saw these rules on the
//! original still has; a name that never ran `tb next` there, or the same name after the text
//! changes, sees it again on the copy too. Nothing here is a lock — it is a note to make sure
//! the manual eventually gets read, not a gate on doing the work.

use super::{Code, err, Connection, Result, Store};
use rusqlite::OptionalExtension;

impl Store {
    /// This board's rules text, or `None` — the default — when it has not set any.
    pub fn rules(&self) -> Result<Option<String>> {
        rules_of(&self.conn)
    }

    /// `Some(text)` sets it (blank text is refused — clear it with `None` instead); `None`
    /// clears it. Returns the text now in force. A board event is logged only when the text
    /// actually changes, exactly like `set_actors` / `set_wip_per_owner` (store/access.rs).
    pub fn set_rules(&self, text: Option<&str>, actor: &str) -> Result<Option<String>> {
        let cleaned = match text {
            Some(t) => {
                let t = t.trim();
                if t.is_empty() {
                    return err(
                        "rules text is empty — 'tb config rules \"TEXT\"', or clear it with 'tb config rules --off'"
                            .to_string(), Code::ArgRequired,
                    );
                }
                Some(t.to_string())
            }
            None => None,
        };
        let old = rules_of(&self.conn)?;
        match &cleaned {
            Some(t) => self.set_config("rules", t)?,
            None => {
                self.conn.execute("DELETE FROM config WHERE key='rules'", [])?;
            }
        }
        if old != cleaned {
            let what = if cleaned.is_some() { "rules changed" } else { "rules cleared" };
            Self::log_board(&self.conn, actor, "rules", what)?;
        }
        Ok(cleaned)
    }

    /// `rules` for the `tb config` listing — only once the board sets it, so a board that
    /// sets nothing lists exactly what it always did.
    pub fn rules_settings(&self) -> Result<Vec<(String, String)>> {
        Ok(self.rules()?.map(|r| vec![("rules".to_string(), r)]).unwrap_or_default())
    }

    /// Has `actor` already been shown THIS board's CURRENT rules text? `false` the first time
    /// (or whenever the text has changed since they last saw it) — the caller shows it and
    /// then calls `mark_rules_seen`.
    pub fn rules_seen(&self, actor: &str, text: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM board_events WHERE kind='rules-seen' AND actor=? COLLATE NOCASE AND text=?",
            rusqlite::params![actor.trim(), text],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Record that `actor` has now been shown this board's current rules text.
    pub fn mark_rules_seen(&self, actor: &str, text: &str) -> Result<()> {
        Self::log_board(&self.conn, actor, "rules-seen", text)
    }
}

fn rules_of(conn: &Connection) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT value FROM config WHERE key='rules'", [], |r| r.get(0)).optional()?)
}
