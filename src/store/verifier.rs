//! Who moves a card into DONE: only a verifier, and only out of REVIEW.
//!
//! The role flow a board follows:
//! - **TODO** — anyone files work.
//! - **DOING** — the workers (one session or many — that is up to whoever runs them).
//! - **REVIEW → DONE** — only an independent verifier, never the one who did the work.
//!   Every move into DONE records who made it, with the full
//!   identity behind the name (`store::actors`).
//!
//! Two rules, both enforced inside `Store::transition` — the one function every column change
//! goes through, from the CLI, the full-screen board and tb's own GitHub sync alike:
//!
//! 1. **Nothing reaches DONE except from REVIEW.** `todo -> done` and `doing -> done` are
//!    refused (`not_from_review`) for everyone; `tb done` on a DOING card already moves it to
//!    REVIEW, and a TODO card is taken first.
//! 2. **REVIEW -> DONE only by a verifier** (`not_verifier`): an actor whose recorded role
//!    (`TB_ROLE`) is `verifier` or `reviewer`, or whose name is on the board's
//!    `config verifiers` list, or a person — an actor with no agent harness in its identity.
//!    The older guards still apply on top: never the card's author — its owner, or for an
//!    unowned card whoever moved it into review — and never its last holder (`self_approve`).
//!    A verifier that sends a card back and later returns it to review does not become its
//!    author by that move (`store::author_of`).
//!
//! Card #169 adds two questions on top of rule 2, so an env-only claim stops being enough:
//! - **The session must be registered** (`unregistered_verifier`): a close that rode on a
//!   verifier role is refused unless THIS session has an entry in the registry
//!   `tb-agent-start` writes at launch (`~/.local/state/terminal-board/verifiers/<session>`)
//!   with the same name and harness — `TB_ROLE=verifier` typed into any shell, including a
//!   script the agent writes itself, is no longer a verifier (the `registry` module below).
//! - **A 'person' close is refused from an agent's process** (`agent_as_person`): an
//!   identity with no harness in it can still be an agent that scrubbed its env — the
//!   kernel's parent chain (`store::proc`) is what settles it.
//!
//! Rule 2 is on by default and can be turned off per board (`tb config verifier-only off`,
//! logged on the board); rule 1 cannot. Who is on `config verifiers`, and whether rule 2
//! applies at all, are a person's settings: an agent that changes either is refused
//! (`person_only`), or a refused agent could list itself and close the card a moment later.
//!
//! **What counts as an agent** is what `actors::Identity::resolve` detects: a harness named by
//! `TB_HARNESS`, `AI_AGENT` (Claude Code, pi, …), `OMPCODE` (omp), the `CODEX_*` variables
//! (codex), `CLAUDECODE`, or a herdr pane's record. A harness that exports none of these is
//! not seen, and its actor counts as a person — the honest limit of a self-reported identity. Both give way to `--force`, exactly as every other
//! guard does, and a forced move is logged as a `force` event naming the rule it skipped.
//!
//! Like every name in tb, a role is self-reported: this stops the honest mistake — an agent
//! closing its own orchestration's work — not someone set on getting past it.

use super::actors::Identity;
use super::{err, BoardError, Code, Connection, Result, Store};
use rusqlite::OptionalExtension;

/// The roles that may move a card from REVIEW to DONE (compared ignoring case).
pub const ROLES: [&str; 2] = ["verifier", "reviewer"];

/// Rule 1's refusal: the card is not in REVIEW.
pub(super) fn not_from_review(id: i64, from: &str) -> BoardError {
    let next = match from {
        "doing" => format!("'tb done {id}' moves it to review"),
        _ => format!("take it first ('tb take {id}'), then 'tb done {id}' moves it to review"),
    };
    BoardError(
        format!(
            "#{id} is in {from} — nothing reaches done except from review: {next}; a verifier moves it to done (or --force, logged)"
        ),
        Code::NotFromReview,
    )
}

/// Rule 2's refusal: an agent with no verifier role.
pub(super) fn not_verifier_err(id: i64, actor: &str, who: &Identity) -> BoardError {
    let harness = who.harness.as_deref().unwrap_or("an agent");
    let role = match who.role.as_deref() {
        Some(r) => format!("role {r}"),
        None => "no role".to_string(),
    };
    BoardError(
        format!(
            "only a verifier moves #{id} from review to done — {actor} is {harness} with {role}. Leave it in review for an independent verifier — a session started with TB_ROLE=verifier, or a person (or --force, logged)"
        ),
        Code::NotVerifier,
    )
}

/// The refusal when an agent changes a setting only a person may change: who may verify, and
/// whether the verifier rule applies at all. Without it, an agent refused `not_verifier` could
/// list itself (or turn the rule off) and close the card a moment later.
pub(super) fn person_only_err(key: &str, actor: &str, who: &Identity) -> BoardError {
    let harness = who.harness.as_deref().unwrap_or("an agent");
    BoardError(
        format!(
            "only a person changes '{key}' — {actor} is {harness}, and an agent may not decide who verifies its own board's work. Ask the person who runs this board"
        ),
        Code::PersonOnly,
    )
}

/// Refuse a change to `key` when this process is an agent.
fn person_only(key: &str, actor: &str) -> Result<()> {
    let who = super::actors::current();
    if is_agent(&who) {
        return Err(person_only_err(key, actor, &who));
    }
    Ok(())
}

/// Refuse `what` (e.g. "delete a board") when this process is an agent: the same rule as the
/// verifier settings — something an agent cannot undo is a person's call.
pub fn person_only_to(what: &str) -> Result<()> {
    let who = super::actors::current();
    if is_agent(&who) {
        let harness = who.harness.as_deref().unwrap_or("an agent");
        return Err(BoardError(
            format!("only a person may {what} — this is {harness}, an agent. Ask the person who runs this board"),
            Code::PersonOnly,
        ));
    }
    Ok(())
}

/// Is this identity an agent? It is when a harness is on record — the one thing a plain
/// terminal (a person) never exports (`actors::Identity::resolve`).
pub fn is_agent(who: &Identity) -> bool {
    who.harness.is_some()
}

/// Does this identity carry a verifier role?
pub fn has_verifier_role(who: &Identity) -> bool {
    who.role.as_deref().is_some_and(|r| ROLES.iter().any(|v| v.eq_ignore_ascii_case(r.trim())))
}

/// May `actor`, with identity `who`, move a card from REVIEW to DONE on this board?
pub(super) fn may_verify(conn: &Connection, actor: &str, who: &Identity) -> Result<bool> {
    if !verifier_only_of(conn)? || !is_agent(who) || has_verifier_role(who) {
        return Ok(true);
    }
    Ok(verifiers_of(conn)?.iter().any(|n| n.eq_ignore_ascii_case(actor.trim())))
}

/// The one question the interim (card #169) adds on top of `may_verify`: is the SESSION this
/// agent runs in one tb-agent-start really launched as a verifier?
///
/// Applies to every agent whose close rides on a verifier role. A session is identified by
/// what tb records as the identity's session (`actors::session_token` of `TB_SESSION` or the
/// harness's exported id) — the same token `same_session` compares, and the same one the
/// registry file names. No session on record = no entry = refused: the env is the only claim
/// behind such a close, and the registry exists to answer exactly that.
///
/// On by default; `TB_VERIFIER_REGISTRY=off` turns it off. The off switch is a person's
/// knob only as far as every other self-asserted escape in tb is: `--force` still gets past
/// the refusal, logged — the registry is a fact to check, not a privilege to grant.
pub(super) fn registered(_conn: &Connection, actor: &str, who: &Identity) -> Result<bool> {
    if !registry_on() {
        return Ok(true);
    }
    let Some(session) = who.session.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(false);
    };
    match registry::of_session(session) {
        Some(e) => Ok(e.harness.eq_ignore_ascii_case(who.harness.as_deref().unwrap_or("").trim())
            && e.name.eq_ignore_ascii_case(actor.trim())),
        None => Ok(false),
    }
}

/// REVIEW → DONE refusal for a close that rode on a role but no registry entry.
pub(super) fn unregistered_verifier_err(id: i64, actor: &str, who: &Identity) -> BoardError {
    registry::unregistered_err(id, actor, who)
}

/// The agent binaries one name covers: a 'person' close whose ancestry holds any of these
/// came OUT of an agent, whatever its environment said.
pub(super) const AGENT_BINARIES: [&str; 4] = ["omp", "claude", "codex", "pi"];

/// Is `ancestry` (oldest ancestor first) a person's? A person's chain names shells and
/// system processes (`bash`, `zsh`, `sshd`, `tmux`, `herdr`, `login`, …). It is an agent's
/// when one entry's name (without an extension, case-insensitive) is an agent binary.
pub(super) fn is_agent_ancestry(ancestry: &[String]) -> bool {
    ancestry.iter().any(|name| {
        let n = name.trim().trim_end_matches(".exe").to_ascii_lowercase();
        AGENT_BINARIES.contains(&n.as_str())
    })
}

/// The 'person' close whose parent chain says agent: the env was scrubbed (`env -u`, a
/// heredoc script), the kernel's record was not.
pub(super) fn agent_as_person_err(id: i64, actor: &str, ancestry: &[String]) -> BoardError {
    BoardError(
        format!(
            "#{id}: {actor} says it is a person (no harness in its identity), but this command was \
             started by {} — an agent's process. tb records the kernel's parent chain with every close; \
             an agent does not close as a person. Close it from your own terminal, or as a verifier \
             in a registered session (or --force, logged)",
            ancestry.iter().find(|n| is_agent_ancestry(std::slice::from_ref(n))).map(String::as_str).unwrap_or("an agent")
        ),
        Code::AgentAsPerson,
    )
}

/// `verifier-registry` — on unless the env says off (`TB_VERIFIER_REGISTRY` /
/// `TTYBOARD_VERIFIER_REGISTRY`, through `crate::env`; no board config: the fleet-wide
/// launcher owns the registry, so the knob stays with the process environment, exactly like
/// `TB_READONLY`).
fn registry_on() -> bool {
    match crate::env("VERIFIER_REGISTRY").as_deref() {
        Some(v) => !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "no" | "false" | "off" | ""),
        None => true,
    }
}

/// `verifier-only` — on unless the board says off.
pub(super) fn verifier_only_of(conn: &Connection) -> Result<bool> {
    let v: Option<String> = conn.query_row("SELECT value FROM config WHERE key='verifier-only'", [], |r| r.get(0)).optional()?;
    Ok(v.as_deref() != Some("off"))
}

/// `verifiers` — the names that may verify whatever their recorded role; empty = none.
pub(super) fn verifiers_of(conn: &Connection) -> Result<Vec<String>> {
    let v: Option<String> = conn.query_row("SELECT value FROM config WHERE key='verifiers'", [], |r| r.get(0)).optional()?;
    Ok(v.map(|s| super::closing::parse_names(&s)).unwrap_or_default())
}

/// The shared session that makes `same_session` refuse, with the builder whose session it is,
/// or None. The panel's design (card
/// #134, 3/3 DO-NOW): a name and a self-reported role are exactly what a same-session
/// verifier can fake — renaming past `self_approve` and claiming `TB_ROLE=verifier` is how a
/// session graded its own work on this fleet (ops #18/#19). The session id is what the
/// harness records under whatever name is used, so it is the identity that does not lie.
///
/// What counts as doing the work is what `author_of` already reads — every `taken` event and
/// every move into review over the card's whole history, in any round, skipping moves by
/// verifier-role identities — plus nothing else: a note or a check is not work, an
/// `assigned` event is an orchestrator's routing, not a hold. The sessions of THOSE events'
/// actors, not their names, are compared with this process's session: any equal pair
/// refuses, whatever the names differ.
///
/// `None` never matches — a session is only known when the harness exports it, so a person's
/// plain terminal and every event written before the record existed are invisible to this,
/// and the older guards answer for them. The same for a blank (trimmed to empty) session.
pub(super) fn same_session_of(conn: &Connection, id: i64, who: &Identity) -> Result<Option<(String, String)>> {
    // The acting session: exactly what `stamp` records, so the same identity is compared
    // that every event carries. A person's terminal (no harness, no exported session) reads
    // as none, and is refused by nothing here.
    let Some(mine) = who.session.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let listed = verifiers_of(conn)?;
    let mut stmt = conn.prepare(
        "SELECT a.session, a.role, e.actor, e.kind
         FROM events e LEFT JOIN actors a ON a.id = e.actor_id
         WHERE e.card_id=? AND (e.kind='taken' OR (e.kind='moved' AND e.text LIKE '% -> review'))
           AND a.session IS NOT NULL",
    )?;
    let rows: Vec<(Option<String>, Option<String>, String, String)> =
        stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
    for (session, role, event_actor, kind) in rows {
        let (Some(session), role) = (session.as_deref().map(str::trim).filter(|s| !s.is_empty()), role) else {
            continue; // NULL or empty never matches
        };
        // for MOVERS a verifier role means the identity was CHECKING the work, not doing
        // it — its sessions are skipped the way its name is skipped in author_of. A TAKEN
        // row is the build itself, so it is compared regardless of role: a builder that
        // took the card under TB_ROLE=verifier still owns session S, and any verifier in
        // S is grading the session that built it (rv-lead-tb, #134 round 2).
        let is_verifier = kind == "moved" && (role.is_some_and(|r| ROLES.iter().any(|v| v.eq_ignore_ascii_case(r.trim())))
            || listed.iter().any(|n| n.eq_ignore_ascii_case(event_actor.trim())));
        if is_verifier {
            continue;
        }
        if session.eq_ignore_ascii_case(mine) {
            return Ok(Some((session.to_string(), event_actor.trim().to_string())));
        }
    }
    Ok(None)
}

/// `same_session`'s refusal: names the shared session and the builder, and says what to do.
pub(super) fn same_session_err(id: i64, session: &str, builder: &str) -> BoardError {
    BoardError(
        format!(
            "#{id} shares your session ({session}) with {builder}, who built it — start the verifier in its own session (TB_ROLE=verifier), never this one (or --force, logged)"
        ),
        Code::SameSession,
    )
}

impl Store {
    /// `tb config verifier-only` — on (the default) or off.
    pub fn verifier_only(&self) -> Result<bool> {
        verifier_only_of(&self.conn)
    }

    /// `tb config verifier-only on|off`, logged on the board when it changes. A person only:
    /// an agent is refused (`person_only`).
    pub fn set_verifier_only(&self, value: &str, actor: &str) -> Result<bool> {
        person_only("verifier-only", actor)?;
        let on = match value.trim().to_ascii_lowercase().as_str() {
            "on" | "yes" | "true" => true,
            "off" | "no" | "false" => false,
            _ => {
                return err(
                    format!("'{}' is not on|off — 'tb config verifier-only on' (the default) lets only a verifier close a card", value.trim()),
                    Code::InvalidValue,
                )
            }
        };
        let old = self.verifier_only()?;
        self.set_config("verifier-only", if on { "on" } else { "off" })?;
        if old != on {
            let said = |b: bool| if b { "on" } else { "off" };
            Self::log_board_with_ancestry(&self.conn, actor, "verifier-only", &format!("verifier-only {} -> {}", said(old), said(on)))?;
        }
        Ok(on)
    }

    /// `tb config verifiers` — the names that may verify whatever their recorded role.
    pub fn verifiers(&self) -> Result<Vec<String>> {
        verifiers_of(&self.conn)
    }

    /// `anna,ben` sets the list; None clears it. Logged on the board when it changes. A person
    /// only: an agent is refused (`person_only`).
    pub fn set_verifiers(&self, list: Option<&str>, actor: &str) -> Result<Vec<String>> {
        person_only("verifiers", actor)?;
        let names = match list {
            Some(l) => {
                let names = super::closing::parse_names(l);
                if names.is_empty() {
                    return err(
                        "say who may verify — 'tb config verifiers rv-1,rv-2', or 'tb config verifiers --off' to clear the list".to_string(),
                        Code::ArgRequired,
                    );
                }
                if let Some(n) = names.iter().find(|n| n.chars().count() > 32 || n.contains(char::is_whitespace)) {
                    return err(format!("'{n}' does not look like a name — 'tb config verifiers rv-1,rv-2'"), Code::InvalidValue);
                }
                names
            }
            None => Vec::new(),
        };
        let old = self.verifiers()?;
        if names.is_empty() {
            self.conn.execute("DELETE FROM config WHERE key='verifiers'", [])?;
        } else {
            self.set_config("verifiers", &names.join(","))?;
        }
        if old != names {
            let said = |v: &[String]| if v.is_empty() { "none".to_string() } else { v.join(", ") };
            Self::log_board_with_ancestry(&self.conn, actor, "verifiers", &format!("verifiers {} -> {}", said(&old), said(&names)))?;
        }
        Ok(names)
    }

    /// `verifier-only` (listed once the board sets it) and `verifiers` (listed once non-empty),
    /// so a board that sets nothing lists exactly what it always did.
    pub fn verifier_settings(&self) -> Result<Vec<(String, String)>> {
        let mut v = Vec::new();
        let set: bool = self
            .conn
            .query_row("SELECT 1 FROM config WHERE key='verifier-only'", [], |r| r.get::<_, i64>(0))
            .optional()?
            .is_some();
        if set {
            v.push(("verifier-only".to_string(), if self.verifier_only()? { "on" } else { "off" }.to_string()));
        }
        let names = self.verifiers()?;
        if !names.is_empty() {
            v.push(("verifiers".to_string(), names.join(",")));
        }
        Ok(v)
    }
}

/// The verifier registry: which sessions tb-agent-start really launched as verifiers.
///
/// An env forge (`TB_ROLE=verifier` in a script the agent writes itself) is indistinguishable
/// from a real verifier launch by the env alone, so REVIEW → DONE by an agent is refused
/// (`unregistered_verifier`) unless THIS process's resolved session has an entry under
/// `~/.local/state/terminal-board/verifiers/<session>` — written by `tb-agent-start --role
/// verifier` at launch, one JSON object `{"session","name","harness"}` per file. All three
/// must match this command's resolved identity: the session it runs in, the `--as` name it
/// ran under, and the harness that session belongs to. `TB_ROLE` / `TB_AS` alone never
/// suffice — that is the forge this stops.
///
/// The registry is same-uid-writable (docs/AGENTS.md, residual risk), so it stops the env
/// forge and the accident, not a determined attacker with a shell on the same account — the
/// kernel-ancestry stamp on every close answers for that (store::proc).
pub(super) mod registry {
    use super::{BoardError, Code, Identity};
    use crate::boards::state_dir;
    use std::path::PathBuf;

    /// One registry entry, as `tb-agent-start` writes it.
    #[derive(serde::Deserialize, Debug, Clone, PartialEq, Eq)]
    pub struct Entry {
        pub session: String,
        pub name: String,
        pub harness: String,
    }

    /// The directory the registry lives in: `~/.local/state/terminal-board/verifiers`,
    /// moved wholesale by `TB_VERIFIERS_DIR` (tests — never set it in production; a person
    /// moving it changes who may close their board).
    pub fn dir() -> PathBuf {
        match crate::env("VERIFIERS_DIR") {
            Some(d) => PathBuf::from(d),
            None => state_dir().join("verifiers"),
        }
    }

    /// The entry `session_id` carries, when the file exists and parses. Read errors (a
    /// directory that cannot be listed, a file that vanished mid-read) count as missing:
    /// tb refuses the close, and says so, rather than inventing an entry.
    pub fn of_session(session_id: &str) -> Option<Entry> {
        let name = session_file_name(session_id)?;
        let raw = std::fs::read_to_string(dir().join(name)).ok()?;
        serde_json::from_str::<Entry>(&raw).ok()
    }

    /// A session id as a file name: the ids tb records are already tokens (`actors::
    /// session_token` — no path, no separator), but a caller-crafted id must never reach
    /// the FS as one. Anything but plain name characters is refused.
    fn session_file_name(session_id: &str) -> Option<String> {
        let ok = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.';
        (session_id.chars().all(ok) && !session_id.starts_with('.'))
            .then(|| session_id.to_string())
    }

    /// REVIEW → DONE refusal when the session is not a registered verifier.
    pub(super) fn unregistered_err(id: i64, actor: &str, who: &Identity) -> BoardError {
        let session = who.session.as_deref().unwrap_or("none");
        BoardError(
            format!(
                "only a registered verifier moves #{id} from review to done — {actor} runs in session \
                 {session}, which tb-agent-start did not start as a verifier (harness claude-code). \
                 Start the verifier with: tb-agent-start <name> --kind claude --model <model> --role verifier \
                 (or --force, logged)"
            ),
            Code::UnregisteredVerifier,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(harness: Option<&str>, role: Option<&str>) -> Identity {
        Identity { harness: harness.map(Into::into), role: role.map(Into::into), ..Identity::default() }
    }

    #[test]
    fn a_person_is_no_agent_and_a_role_is_read_ignoring_case() {
        assert!(!is_agent(&Identity::default()), "a plain terminal exports no harness");
        assert!(is_agent(&who(Some("claude-code"), None)));
        assert!(has_verifier_role(&who(Some("omp"), Some("Verifier"))));
        assert!(has_verifier_role(&who(Some("omp"), Some("reviewer"))));
        assert!(!has_verifier_role(&who(Some("omp"), Some("builder"))));
        assert!(!has_verifier_role(&who(Some("omp"), None)));
    }

    #[test]
    fn the_refusals_name_what_to_do_next() {
        let e = not_from_review(4, "doing");
        assert_eq!(e.1, Code::NotFromReview);
        assert!(e.0.contains("'tb done 4' moves it to review") && e.0.contains("a verifier moves it to done"), "{}", e.0);
        let e = not_from_review(4, "todo");
        assert!(e.0.contains("'tb take 4'"), "{}", e.0);
        let e = not_verifier_err(4, "bob", &who(Some("claude-code"), None));
        assert_eq!(e.1, Code::NotVerifier);
        assert!(e.0.contains("TB_ROLE=verifier") && e.0.contains("claude-code with no role"), "{}", e.0);
        assert!(!e.0.contains("config verifiers"), "a refused agent is never told how to list itself: {}", e.0);
        let e = person_only_err("verifiers", "bob", &who(Some("codex"), None));
        assert_eq!(e.1, Code::PersonOnly);
        assert!(e.0.contains("only a person changes 'verifiers'") && e.0.contains("bob is codex"), "{}", e.0);
    }

    #[test]
    fn the_same_session_refusal_names_the_session_and_the_action() {
        let e = same_session_err(4, "S1", "bld-1");
        assert_eq!(e.1, Code::SameSession);
        assert!(e.0.contains("(S1)") && e.0.contains("bld-1") && e.0.contains("TB_ROLE=verifier"), "{}", e.0);
    }
}
