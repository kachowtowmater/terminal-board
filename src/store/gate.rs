//! The board's side of a hook: which NAME it asks for, and the record of what happened.
//!
//! A board file holds a name and nothing else — never a command (see [`crate::hooks`]). The
//! name is `[a-z0-9_-]{1,32}`, so it is safe to print, and it is meaningless on a machine that
//! has not been told what it runs: that machine says so and refuses the change, rather than
//! quietly dropping a gate the board asked for.
//!
//! Every run that led to a change is written on the card as a `hook` event, so `tb show` is
//! the record of what approved what. A run that REFUSED writes nothing at all — a refusal
//! changes the board in no way whatever, which is what lets an agent retry safely — and the
//! refusal text is what the caller sees instead. `--break-glass` is the exception that must be
//! recorded: it writes a `break-glass` event on the card AND a row in the board log, because
//! skipping a gate is a fact about the board, not about one card.

use super::{err, BoardError, Code, Result, Store};
use crate::hooks::Event;
use rusqlite::{Connection, OptionalExtension};

/// The hook name a board asks for at this event, read inside whatever transaction is open.
pub fn hook_of(conn: &Connection, event: Event) -> Result<Option<String>> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM config WHERE key=?", [event.key()], |r| r.get(0)).optional()?;
    Ok(v.filter(|s| !s.trim().is_empty()))
}

impl Store {
    /// The hook this board asks for at `event`, if any.
    pub fn hook(&self, event: Event) -> Result<Option<String>> {
        hook_of(&self.conn, event)
    }

    /// `tb config hook NAME` / `--off`. The name is checked here, but what it runs — and
    /// whether this machine is willing to run it — is not: a board is written on one machine
    /// and opened on another, and it is the opening machine that decides.
    pub fn set_hook(&self, event: Event, name: Option<&str>, actor: &str) -> Result<String> {
        let old = self.hook(event)?;
        let new = match name.map(str::trim) {
            None | Some("") => None,
            Some(n) if crate::hooks::valid_name(n) => Some(n.to_string()),
            Some(n) => {
                return err(
                    format!(
                        "'{n}' is not a hook name (a-z 0-9 _ -, up to 32) — name one this machine knows, e.g. 'tb config {} approve'",
                        event.key()
                    ),
                    Code::InvalidValue,
                )
            }
        };
        match &new {
            Some(n) => self.set_config(event.key(), n)?,
            None => {
                self.conn.execute("DELETE FROM config WHERE key=?", [event.key()])?;
            }
        }
        if old != new {
            let text = format!("{} {} -> {}", event.key(), old.as_deref().unwrap_or("off"), new.as_deref().unwrap_or("off"));
            Self::log_board(&self.conn, actor, "hook", &text)?;
        }
        Ok(new.unwrap_or_else(|| "off".into()))
    }

    /// The `hook` / `hook-after` rows of `tb config`: listed once the board asks for one, so a
    /// board that asks for none lists exactly what it always did.
    pub(super) fn hook_settings(&self) -> Result<Vec<(String, String)>> {
        let mut v = Vec::new();
        for e in Event::all() {
            if let Some(name) = self.hook(e)? {
                v.push((e.key().to_string(), name));
            }
        }
        Ok(v)
    }

    /// Record a hook run — allowed or (for `hook-after`) merely attempted. Written in its own
    /// transaction: the change it belongs to is already committed, so a post-change hook's
    /// record never rolls back with a failure that is not the change's own.
    pub fn log_hook(&self, id: i64, actor: &str, text: &str) -> Result<()> {
        Self::log(&self.conn, id, actor, "hook", text)
    }

    /// Record a break-glass: skipped the pre-change hook `name` (or "no hook was set" is never
    /// reached — see [`no_gate_err`]) and applied the change anyway. Both a card event (so `tb
    /// show` carries it) and a board-level row (so it is not lost if the card is later
    /// archived or deleted) — skipping a gate is a fact about the BOARD, not only the card.
    pub fn log_break_glass(&self, id: i64, actor: &str, name: &str, why: &str) -> Result<()> {
        let text = format!("{name}: {why}");
        Self::log(&self.conn, id, actor, "break-glass", &text)?;
        Self::log_board(&self.conn, actor, "break-glass", &format!("#{id} {text}"))?;
        Ok(())
    }
}

/// The one refusal for `--break-glass` where there is nothing to break: a flag that quietly
/// does nothing is a flag that will be trusted when it matters.
pub fn no_gate_err() -> BoardError {
    BoardError(
        "--break-glass is for getting past a hook, and this board asks for none — run the same command without it".to_string(),
        Code::InvalidValue,
    )
}
