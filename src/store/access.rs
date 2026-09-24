//! Who may write to this board, and whether anybody may: `TB_READONLY=1` / `--read-only`,
//! `tb config actors NAME,NAME,…` and `tb config wip-per-owner N`.
//!
//! **Read-only is enforced where every write really funnels: the SQLite connection.** There
//! are 80-odd `execute` sites across the store, and a list of them would be a list somebody
//! forgets to add to. So in read-only mode the database is OPENED read-only, and any write
//! that somehow reaches it fails at SQLite with `attempt to write a readonly database`, which
//! is turned into tb's own refusal. The command layer refuses earlier, with a better message
//! and before anything else happens; the connection is the backstop that makes "every write"
//! true by construction rather than by inspection.
//!
//! **Known names** stop a typo inventing an agent. Two rules keep the list from becoming a
//! trap: `tb config actors` itself is never gated (so a list you cannot satisfy can always be
//! fixed), and a list that does not name the person setting it is refused outright. The
//! `github` actor tb's own sync writes under is always allowed, because it is not a person
//! who can be added to a list.
//!
//! **The per-owner WIP cap** is a second, independent question from the board-wide limit:
//! the board-wide one asks "is the board full?", the per-owner one asks "are YOU full?".
//! Both must pass. Each discounts blocked cards the same way, under the same setting, capped
//! by its own limit — see `room_for`.

use super::{blocks, err, BoardError, Code, Result, Store};
use rusqlite::{Connection, OptionalExtension};

/// The highest per-owner cap, as for the board-wide limit.
pub const MAX_PER_OWNER: i64 = 99;

/// The name tb's own GitHub sync writes under. Never a person, so never on an actors list and
/// never refused by one.
pub const SYNC_ACTOR: &str = "github";

/// Is this process refusing every write? `TB_READONLY` (or `TTYBOARD_READONLY`) set to
/// anything but `0`/`no`/`false`/empty, or `--read-only` on the command line.
pub fn readonly_env() -> bool {
    readonly_value(crate::env("READONLY").as_deref())
}

/// What a `TB_READONLY` value means: on unless it is unset, empty, `0`, `no` or `false`. Kept
/// apart from the environment so it is tested without setting a process-wide variable, which
/// every other test in the same process would see — a read-only store under their feet.
fn readonly_value(v: Option<&str>) -> bool {
    v.is_some_and(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "" | "0" | "no" | "false"))
}

/// The refusal every write gets in read-only mode. One line: what happened, then what to do.
pub fn refusal(what: &str) -> BoardError {
    BoardError(format!(
        "read-only mode: '{what}' would change the board — unset TB_READONLY (or drop --read-only) to make changes; reads like 'tb list', 'tb board --json' and 'tb watch --json' work as usual"
        ),
        Code::ReadOnly,
    )
}

/// Did SQLite refuse this because the database was opened read-only? Those come back as
/// `SQLITE_READONLY`, and tb turns them into its own wording rather than showing the raw text.
pub fn is_readonly_error(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ReadOnly)
}

/// The names a board accepts as actors, in the order they were written; empty = anyone.
pub fn actors_of(conn: &Connection) -> Result<Vec<String>> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM config WHERE key='actors'", [], |r| r.get(0)).optional()?;
    Ok(v.map(|s| clean_list(&s)).unwrap_or_default())
}

/// A list as it is stored and shown: trimmed, empties dropped, order kept, one name once
/// (compared without case, so `Alice` and `alice` are one name).
pub fn clean_list(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in raw.split(',') {
        let n = name.trim();
        if n.is_empty() || out.iter().any(|k| k.eq_ignore_ascii_case(n)) {
            continue;
        }
        out.push(n.to_string());
    }
    out
}

/// Is `actor` allowed to write to this board? Names are compared trimmed and without case,
/// the way owners are compared everywhere else.
pub fn known(names: &[String], actor: &str) -> bool {
    let a = actor.trim();
    names.is_empty() || a.eq_ignore_ascii_case(SYNC_ACTOR) || names.iter().any(|n| n.eq_ignore_ascii_case(a))
}

/// Refuse a write by a name this board does not know. Called from `Store::log`, which every
/// card change goes through, so there is no path that skips it — and from the two writers
/// that build their events by hand (`store::transfer`).
///
/// A board with no list lets everyone in, so this is one config read on an untouched board.
pub(crate) fn guard_actor(conn: &Connection, actor: &str) -> Result<()> {
    let names = actors_of(conn)?;
    if known(&names, actor) {
        return Ok(());
    }
    Err(unknown_actor(actor, &names))
}

/// The refusal for a name the board does not know.
pub fn unknown_actor(actor: &str, names: &[String]) -> BoardError {
    BoardError(format!(
        "'{}' is not one of this board's names ({}) — check the spelling of --as, or add it with 'tb config actors {},{}'",
        actor.trim(),
        names.join(", "),
        names.join(","),
        actor.trim()
        ),
        Code::UnknownActor,
    )
}

impl Store {
    /// The rows `tb config` prints for this lane — ONLY once the board sets them, so a board
    /// that sets nothing lists exactly what it always did.
    pub fn access_settings(&self) -> Result<Vec<(String, String)>> {
        let mut v = Vec::new();
        let cap = self.wip_per_owner()?;
        if cap > 0 {
            v.push(("wip-per-owner".to_string(), cap.to_string()));
        }
        let names = self.actors_allowed()?;
        if !names.is_empty() {
            v.push(("actors".to_string(), names.join(", ")));
        }
        Ok(v)
    }

    /// `tb config actors` — the names, or empty when the board accepts anyone.
    pub fn actors_allowed(&self) -> Result<Vec<String>> {
        actors_of(&self.conn)
    }

    /// Set the list (empty clears it). Refused when the person setting it is not on it: a
    /// list you cannot satisfy would lock you out of your own board.
    pub fn set_actors(&self, raw: &str, actor: &str) -> Result<Vec<String>> {
        let names = clean_list(raw);
        if !names.is_empty() && !known(&names, actor) {
            return err(format!(
                "'{}' is not in that list, and setting it would lock you out of your own board — include yourself: 'tb config actors {},{}'",
                actor.trim(),
                names.join(","),
                actor.trim()
                ),
                Code::InvalidValue,
            );
        }
        let old = self.actors_allowed()?;
        self.set_config("actors", &names.join(","))?;
        if old != names {
            let said = |v: &[String]| if v.is_empty() { "anyone".to_string() } else { v.join(", ") };
            Self::log_board(&self.conn, actor, "actors", &format!("actors {} -> {}", said(&old), said(&names)))?;
        }
        Ok(names)
    }

    /// Refuse `actor` when the board keeps a list and the name is not on it.
    pub fn check_actor(&self, actor: &str) -> Result<()> {
        let names = self.actors_allowed()?;
        if known(&names, actor) {
            return Ok(());
        }
        Err(unknown_actor(actor, &names))
    }

    /// `tb config wip-per-owner` — 0 (the default) means no per-owner cap.
    pub fn wip_per_owner(&self) -> Result<i64> {
        per_owner_of(&self.conn)
    }

    pub fn set_wip_per_owner(&self, n: i64, actor: &str) -> Result<()> {
        if !(0..=MAX_PER_OWNER).contains(&n) {
            return err(format!(
                "wip-per-owner must be 0-{MAX_PER_OWNER} (0 = no per-owner limit) — try 'tb config wip-per-owner 1'"
                ),
                Code::InvalidValue,
            );
        }
        let old = self.wip_per_owner()?;
        self.set_config("wip-per-owner", &n.to_string())?;
        if old != n {
            let said = |v: i64| if v == 0 { "off".to_string() } else { v.to_string() };
            Self::log_board(&self.conn, actor, "wip-per-owner", &format!("wip-per-owner {} -> {}", said(old), said(n)))?;
        }
        Ok(())
    }
}

pub(super) fn per_owner_of(conn: &Connection) -> Result<i64> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM config WHERE key='wip-per-owner'", [], |r| r.get(0)).optional()?;
    Ok(v.and_then(|s| s.trim().parse().ok()).filter(|n| *n > 0).unwrap_or(0))
}

/// How many DOING cards `owner` holds, and how many of those count against their cap.
///
/// The per-owner cap discounts blocked cards exactly as the board-wide limit does, and for
/// the same reason: `wip-counts-blocked no` exists so that waiting on somebody else does not
/// stall the board, and a person waiting on somebody else is stalled in precisely the same
/// way. Each limit caps its own discount by its own number — the board discounts at most
/// `wip` blocked cards, a person at most `cap` — so blocking everything can never hand out
/// unlimited work at either level.
pub(super) fn owner_counts(tx: &Connection, owner: &str, cap: i64) -> Result<(i64, i64)> {
    let held: i64 = tx.query_row(
        r#"SELECT COUNT(*) FROM cards WHERE "column"='doing' AND owner=? COLLATE NOCASE"#,
        [owner.trim()],
        |r| r.get(0),
    )?;
    if !blocks::counts_blocked_off(tx)? {
        return Ok((held, held));
    }
    let blocked: i64 = tx.query_row(
        r#"SELECT COUNT(*) FROM cards WHERE "column"='doing' AND owner=? COLLATE NOCASE
           AND (blocked IS NOT NULL OR blocked_on IS NOT NULL)"#,
        [owner.trim()],
        |r| r.get(0),
    )?;
    Ok((held - blocked.min(cap.max(0)), held))
}

/// Is there room for one more card in `actor`'s hands? `Ok(())`, or the refusal naming what
/// they already hold and what they can do about it.
pub(super) fn room_for(tx: &Connection, actor: &str) -> Result<()> {
    let cap = per_owner_of(tx)?;
    if cap <= 0 {
        return Ok(());
    }
    let (counted, held) = owner_counts(tx, actor, cap)?;
    if counted < cap {
        return Ok(());
    }
    let mut st = tx.prepare(
        r#"SELECT id, title FROM cards WHERE "column"='doing' AND owner=? COLLATE NOCASE ORDER BY id"#,
    )?;
    let mine: Vec<String> = st
        .query_map([actor.trim()], |r| Ok(format!("#{} {}", r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let first = mine.first().and_then(|m| m.split_whitespace().next().map(|s| s.trim_start_matches('#').to_string()));
    let finish = match first {
        Some(id) => format!("finish one with 'tb done {id}'"),
        None => "finish one first".to_string(),
    };
    let discounted = held - counted;
    let waiting = if discounted > 0 { format!(" ({discounted} of them blocked and not counted)") } else { String::new() };
    Err(BoardError(format!(
        "you already hold {counted} of {cap} ({}){waiting} — {finish}, or ask for the limit to be raised with 'tb config wip-per-owner {}'",
        mine.join(", "),
        cap + 1
        ),
        Code::WipOwnerFull,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_is_trimmed_deduped_and_case_insensitive() {
        assert_eq!(clean_list(" alice , bob ,,  alice ,ALICE "), ["alice", "bob"]);
        assert_eq!(clean_list(""), Vec::<String>::new());
        assert_eq!(clean_list("  ,  "), Vec::<String>::new());
        let names = clean_list("alice,bob");
        assert!(known(&names, "alice") && known(&names, "ALICE") && known(&names, "  bob  "));
        assert!(!known(&names, "carol"));
        // an empty list lets anyone in, and the sync's own name is always allowed
        assert!(known(&[], "anyone") && known(&names, SYNC_ACTOR));
    }

    #[test]
    fn readonly_reads_the_environment_the_way_a_person_expects() {
        // the value alone, never through `set_var`: TB_READONLY set here was seen by every
        // test running beside this one, and their stores refused writes ("read-only mode")
        for (v, want) in [("1", true), ("yes", true), ("true", true), ("0", false), ("no", false), ("false", false), ("FALSE", false), (" 0 ", false)] {
            assert_eq!(readonly_value(Some(v)), want, "TB_READONLY={v}");
        }
        assert!(!readonly_value(None), "unset");
    }

    #[test]
    fn the_refusal_says_what_to_do() {
        let e = refusal("tb add").0;
        assert!(e.starts_with("read-only mode: 'tb add' would change the board — "), "{e}");
        assert!(e.contains("unset TB_READONLY") && e.contains("tb watch --json"), "{e}");
    }
}
