//! The board's reader list (`tb config readers NAME,NAME`): who may run a command against
//! this board at all.
//!
//! Charles's private boards (`bar`, …) hold things no worker needs to read, and #1079 showed
//! the gap: a worker inside a Claude session scrubbed `TB_AS` and read the board through
//! `TB_DB`. `readers` is the board's own answer: when the list is set, every command that
//! opens the board — read or write, a named board or `TB_DB` — asks the caller's identity.
//! A person passes; an agent (harness in the identity, or an agent binary in the kernel's
//! process ancestry, so a scrubbed env under `omp`/`claude` does not pass as a person)
//! passes only when the name it acts under (`--as`/`TB_AS`) is on the list; an agent with no
//! name at all is refused. The refusal names the board, the caller, the list, and who may
//! change it: only a person sets or clears `readers`, the same rule the verifier settings
//! follow (`person_only`), so an agent a refusal annoyed cannot rewrite the list and walk in.
//!
//! Like every name in tb, a reader name is self-asserted: this stops the honest mistake — a
//! worker wandering into a private board — not an agent set on getting past it (it can still
//! forge `TB_AS` until privilege separation, #915).
//!
//! The check runs where every open goes through, `Store::open` — not per command — so no
//! command can skip it, and boards without a list behave exactly as before. `main` re-asks
//! after the open with the actor it dispatched under, which is also what covers the
//! in-memory default-board open that skips `open`'s tail. The GitHub sync and the reaper
//! pass as today: they are library callers with identity recording off, so they read as a
//! person here, the same reading every verifier-style guard gives them.

use super::actors::Identity;
use super::verifier::{agent_as_person, is_agent, is_agent_ancestry};
use super::{err, BoardError, Code, Connection, Result, Store};
use rusqlite::OptionalExtension;

/// The reader list on `conn`: empty = no restriction, every caller passes.
fn readers_of(conn: &Connection) -> Result<Vec<String>> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM config WHERE key='readers'", [], |r| r.get(0)).optional()?;
    // `off` is the word a person typed to drop the list — a name nobody carries, and the
    // value an older binary of this same feature once stored literally (a round-2 build).
    // Whatever wrote it, the board it left must read as "no readers": any agent may open it.
    Ok(v.filter(|s| !s.trim().eq_ignore_ascii_case("off"))
        .map(|s| super::closing::parse_names(&s))
        .unwrap_or_default())
}

/// May this caller run a command on the board behind `conn`?
///
/// - No list: yes, unchanged.
/// - The `github` actor passes (tb's own sync, the exemption `known` gives every name list —
///   its moves are evidence, not a person proposing a change).
/// - A person passes.
/// - An agent passes only when the name it acts under is on the list.
/// - A 'person' identity inside an agent's process (`agent_as_person`'s scrubbed env, card
///   #169) is the agent it really is: refused unless its ancestry's binary gave it a name
///   on the list — which a scrubbed env never does, so it is refused.
pub(super) fn may_read(conn: &Connection, actor: &str, who: &Identity) -> Result<bool> {
    let readers = readers_of(conn)?;
    if readers.is_empty()
        || actor.trim().eq_ignore_ascii_case(super::access::SYNC_ACTOR)
        || (!is_agent(who) && !is_agent_ancestry(&crate::proc::ancestry()))
    {
        return Ok(true);
    }
    // a scrubbed env that still carries a name on the list passes; none at all is refused
    let name = actor.trim();
    Ok(!name.is_empty() && readers.iter().any(|n| n.eq_ignore_ascii_case(name)))
}

/// The refusal: names the board, the caller and the list, and says who lifts it.
pub(super) fn not_a_reader_err(board: &str, actor: &str, readers: &[String]) -> BoardError {
    BoardError(
        format!(
            "board '{board}' is read-only to its reader list ({list}) — {actor} is not on it: ask a person to add you \
             ('tb {board} config readers NAME,NAME', or 'off' to drop it; only a person sets or clears the list)",
            list = readers.join(", "),
        ),
        Code::NotAReader,
    )
}

/// The person-only refusal for `tb config readers`, the same shape the verifier settings use.
pub(super) fn readers_person_only_err(actor: &str, who: &Identity) -> BoardError {
    if is_agent(who) {
        let harness = who.harness.as_deref().unwrap_or("an agent");
        return BoardError(
            format!(
                "only a person may change 'config readers' — this is {harness}, an agent. Ask the person who runs this board"
            ),
            Code::PersonOnly,
        );
    }
    let ancestry = agent_as_person("this person", who).unwrap_or_default();
    super::verifier::agent_as_person_change_err("change 'config readers'", actor, &ancestry)
}

impl Store {
    /// `tb config readers` — the names, or empty when the board has no reader list.
    pub fn readers(&self) -> Result<Vec<String>> {
        readers_of(&self.conn)
    }

    /// `tb config readers NAME,NAME` sets the list, `None` clears it. A person only: an agent
    /// (a harness on record, or one hiding behind a scrubbed env) is refused, the way the
    /// verifier settings are. Logged on the board when it changes.
    pub fn set_readers(&self, list: Option<&str>, actor: &str) -> Result<Vec<String>> {
        let who = super::actors::current();
        if is_agent(&who) || agent_as_person(actor, &who).is_some() {
            return Err(readers_person_only_err(actor, &who));
        }
        // `off` as the value is the clearing `--off` is: the word a person typed to drop the
        // list (a person-only change either way), never a name to store
        let list = list.filter(|l| !l.trim().eq_ignore_ascii_case("off"));
        let names = match list {
            Some(l) => {
                let names = super::closing::parse_names(l);
                if names.is_empty() {
                    return err(
                        "say who may open this board — 'tb config readers tb-box-enforcer,lead-bar', or 'tb config readers --off' to drop the list".to_string(),
                        Code::ArgRequired,
                    );
                }
                if let Some(n) = names.iter().find(|n| n.chars().count() > 32 || n.contains(char::is_whitespace)) {
                    return err(format!("'{n}' does not look like a name — 'tb config readers tb-box-enforcer,lead-bar'"), Code::InvalidValue);
                }
                names
            }
            None => Vec::new(),
        };
        let old = self.readers()?;
        if names.is_empty() {
            self.conn.execute("DELETE FROM config WHERE key='readers'", [])?;
        } else {
            self.set_config("readers", &names.join(","))?;
        }
        if old != names {
            let said = |v: &[String]| if v.is_empty() { "none".to_string() } else { v.join(", ") };
            Self::log_board_with_ancestry(&self.conn, actor, "readers", &format!("readers {} -> {}", said(&old), said(&names)))?;
        }
        Ok(names)
    }

    /// The `readers` row of `tb config`: listed once the board sets it, so a board that sets
    /// nothing lists exactly what it always did.
    pub fn reader_settings(&self) -> Result<Vec<(String, String)>> {
        let names = self.readers()?;
        Ok(if names.is_empty() { Vec::new() } else { vec![("readers".to_string(), names.join(","))] })
    }

    /// Refuse this caller when the board keeps a reader list and they are not on it. The one
    /// check every `Store::open` runs (before the connection is handed back), so no command
    /// — read or write, by name or `TB_DB` — can skip it.
    ///
    /// The identity is resolved only when the board actually keeps a list: on a board
    /// without one (every board until a person sets `config readers`) the check is a single
    /// config SELECT and asks nobody anything — a herdr pane, in particular, is not asked
    /// `agent list` for it, so an ordinary command behaves byte-for-byte as it did before.
    /// `actor` is `None` only from `Store::open`, which has not resolved a name yet; the
    /// dispatch later re-asks with the name it runs under.
    pub fn check_readers(&self, actor: Option<&str>) -> Result<()> {
        let readers = readers_of(&self.conn)?;
        if readers.is_empty() {
            return Ok(());
        }
        // the caller's name: the one the dispatch resolved when it has one (`--as`/TB_AS),
        // else the one `resolve_actor` finds — an agent under an agent ancestor has to ask
        // herdr for the pane's name, and that question is only worth asking on a board that
        // actually keeps a list (a board without one asks nobody anything, so an ordinary
        // command behaves byte-for-byte as it did before this module existed)
        let actor = match actor {
            Some(a) => a.trim().to_string(),
            None => crate::resolve_actor(None),
        };
        let who = super::actors::current();
        if may_read(&self.conn, &actor, &who)? {
            return Ok(());
        }
        Err(not_a_reader_err(&self.name, &actor, &readers))
    }
}
