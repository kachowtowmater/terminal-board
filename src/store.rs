//! SQLite store: cards, checklist, events, config. WAL mode, atomic claims.
//!
//! **A board's file, not just its bytes, has a lifetime** (#112): `Store::open` creates the
//! file if it is not there, and most write paths take their own short transaction, so nothing
//! stopped a process holding an open handle from being told the file had moved, or a process
//! arriving just after a move from recreating the board it was meant to find gone. `Store::open`
//! now takes `crate::lock`'s SHARED lock on a path-keyed `.lock` file for as long as the
//! `Store` lives; a command that moves or replaces a board file (`lock_for_move`) takes the
//! EXCLUSIVE half across the whole operation, so it waits for every live reader/writer and none
//! can arrive mid-move. `link_into_place` is the other half: placing a file back without ever
//! clobbering one a racing writer just created. This primitive is what `crate::boards::archive`
//! and `crate::boards::restore` (#80) are built on — anything else that retires or revives a
//! board file must use it too, never a plain `rename`.

use crate::hooks;
use crate::lock;
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

pub mod access;
pub mod actors;
pub mod archive;
pub mod blocks;
pub mod closing;
pub mod creator;
pub mod bulk;
pub mod display;
pub mod due;
pub mod gate;
pub mod kinds;
pub mod links;
pub mod order;
pub mod release;
pub mod rounds;
pub mod rules;
pub mod transfer;
pub mod verifier;

pub const COLUMNS: [&str; 4] = ["todo", "doing", "review", "done"];
pub const DEFAULT_WIP: i64 = 3;
pub const MAX_WIP: i64 = 99;
/// Layout preferences, in the order `L` cycles through them.
pub const LAYOUTS: [&str; 6] = ["auto", "focus", "third-h", "third-v", "half-h", "half-v"];

/// Older layout names still found in boards written by earlier versions.
fn layout_alias(l: &str) -> &str {
    match l {
        "full" => "half-h",
        "sidebar" => "third-v",
        "strip" => "third-h",
        "minimum" => "focus",
        other => other,
    }
}

/// A stable machine-readable symbol for a `--json` failure (`docs/JSON.md`). It lives on
/// `BoardError` itself (`.1`), not on the message text, so the compiler requires one at every
/// construction site instead of a caller re-deriving it from prose later.
///
/// The vocabulary is OPEN: new variants may be added at any time, and a consumer that meets a
/// `code` it does not recognize falls back to `error` and the exit status — exactly as it
/// would for a future addition. Once shipped, a variant's `as_str()` is a contract: it is
/// never renamed or reused for a different meaning. `Unknown` is the mandatory catch-all, so
/// no failure path can construct a `BoardError` without picking one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// Changing a DOING card held by another actor without `--force` (the holder rule).
    NotOwner,
    /// A board name given while `TB_DB` pins one file.
    DbPinned,
    /// The named board does not exist.
    NoBoard,
    /// `--as ""`.
    EmptyActor,
    /// Sending a card back to DOING with no reason.
    ReasonRequired,
    /// No card with that id (on the board, or in the archive).
    NoCard,
    /// DOING is at the board's `wip` limit.
    WipFull,
    /// One actor is at this board's `wip-per-owner` cap.
    WipOwnerFull,
    /// A write attempted while the board is open read-only (`TB_READONLY` / `--read-only`).
    ReadOnly,
    /// The actor is not on this board's `config actors` list.
    UnknownActor,
    /// A board name that is not `[a-z0-9_-]{1,32}`.
    InvalidBoardName,
    /// A board name that collides with a command word.
    BoardNameIsCommand,
    /// `tb done` refused: the linked GitHub issue is still open.
    GhIssueOpen,
    /// A GitHub-only command run on a board with no `github` repo configured.
    GithubOff,
    /// A GitHub API/network call failed.
    GithubError,
    /// An approval outside REVIEW.
    NotInReview,
    /// `tb release` on a card that is not in DOING with a holder (store/release.rs).
    NotInDoing,
    /// `tb release` refused: a liveness probe vouches for the card's holder (store/release.rs).
    HolderAlive,
    /// `tb release` by an agent whose role is not lead or orchestrator (store/release.rs).
    NotReleaser,
    /// The actor who did the work tried to approve or review their own card
    /// (never-approve-your-own-work).
    SelfApprove,
    /// REVIEW -> DONE by a verifier running in the same recorded session as an identity that
    /// took the card or moved it into review, in any round (store/verifier.rs).
    SameSession,
    /// `config done-by` restricts who may close a card, and the actor is not on the list.
    DoneByRestricted,
    /// A move into DONE from a column other than REVIEW (`todo -> done`, `doing -> done`):
    /// nothing reaches DONE except from REVIEW (store/verifier.rs).
    NotFromReview,
    /// REVIEW -> DONE by an agent with no verifier role and not on `config verifiers`
    /// (store/verifier.rs).
    NotVerifier,
    /// A setting only a person may change (`config verifiers`, `config verifier-only`) was
    /// changed by an agent — an actor with a harness in its identity (store/verifier.rs).
    PersonOnly,
    /// `config done-needs-note` requires a note written during this stay before DONE.
    DoneNeedsNote,
    /// `config done-needs-link` requires a link with that label before DONE.
    DoneNeedsLink,
    /// A required argument or value was not given.
    ArgRequired,
    /// An unrecognized subcommand or command word.
    UnknownCommand,
    /// An unrecognized `tb config` key.
    UnknownSetting,
    /// A value given for a recognized field/setting/flag is not one it accepts.
    InvalidValue,
    /// The database could not be opened, read or written (including "locked, try again").
    DbError,
    /// Reading or writing a file (settings, text-from-file, stdin, export) failed.
    IoError,
    /// The interactive TUI failed to start or run.
    TerminalError,
    /// A board could not be locked: `Store::open` waited out an exclusive holder (a move in
    /// progress) without getting the shared lock, or a command that moves/replaces a board
    /// file waited out every reader/writer without getting the exclusive one.
    BoardBusy,
    /// `tb boards archive` refused: `NAME` is the board a bare `tb` opens right now.
    DefaultBoard,
    /// `tb boards restore` refused: a live board (or a stray `-wal`/`-shm`) is already at that
    /// name.
    BoardExists,
    /// `tb boards restore` refused: no archived board has that name.
    NoArchive,
    /// `tb boards delete` refused: the name is a live board, not an archived one — archive it
    /// first (the archive is the undo window).
    BoardLive,
    /// `tb boards delete` refused: not on a terminal and `--yes` was not given.
    ConfirmRequired,
    /// A command-line argument failed to parse (clap): missing/extra/malformed flags,
    /// unrecognized subcommands caught at the parser level, wrong arity, etc.
    Usage,
    /// A pre-change hook (`config hook`, `crate::hooks`) refused the change, could not be run,
    /// is not trusted on this machine, or this machine does not know it.
    HookRefused,
    /// A pre-change hook allowed the change, but the card was changed by someone else while
    /// the hook ran, so the approval no longer describes it. Nothing was written: retry.
    HookRace,
    /// `tb trust NAME …` for a name this machine has no hook under.
    NoHook,
    /// Every failure path that predates this vocabulary, or that does not yet warrant its own
    /// symbol. A consumer that meets it falls back to `error` and the exit status, exactly as
    /// it would for a code it does not recognize.
    Unknown,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::NotOwner => "not_owner",
            Code::DbPinned => "db_pinned",
            Code::NoBoard => "no_board",
            Code::EmptyActor => "empty_actor",
            Code::ReasonRequired => "reason_required",
            Code::NoCard => "no_card",
            Code::WipFull => "wip_full",
            Code::WipOwnerFull => "wip_owner_full",
            Code::ReadOnly => "read_only",
            Code::UnknownActor => "unknown_actor",
            Code::InvalidBoardName => "invalid_board_name",
            Code::BoardNameIsCommand => "board_name_is_command",
            Code::GhIssueOpen => "gh_issue_open",
            Code::GithubOff => "github_off",
            Code::GithubError => "github_error",
            Code::NotInReview => "not_in_review",
            Code::NotInDoing => "not_in_doing",
            Code::HolderAlive => "holder_alive",
            Code::NotReleaser => "not_releaser",
            Code::SelfApprove => "self_approve",
            Code::SameSession => "same_session",
            Code::DoneByRestricted => "done_by_restricted",
            Code::NotFromReview => "not_from_review",
            Code::NotVerifier => "not_verifier",
            Code::PersonOnly => "person_only",
            Code::DoneNeedsNote => "done_needs_note",
            Code::DoneNeedsLink => "done_needs_link",
            Code::ArgRequired => "arg_required",
            Code::UnknownCommand => "unknown_command",
            Code::UnknownSetting => "unknown_setting",
            Code::InvalidValue => "invalid_value",
            Code::DbError => "db_error",
            Code::IoError => "io_error",
            Code::TerminalError => "terminal_error",
            Code::BoardBusy => "board_busy",
            Code::DefaultBoard => "default_board",
            Code::BoardExists => "board_exists",
            Code::NoArchive => "no_archive",
            Code::BoardLive => "board_live",
            Code::ConfirmRequired => "confirm_required",
            Code::Usage => "usage",
            Code::HookRefused => "hook_refused",
            Code::HookRace => "hook_race",
            Code::NoHook => "no_hook",
            Code::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `msg` is the human-readable text (`Display`, `.0`); `code` (`.1`) is the stable symbol a
/// `--json` caller branches on — see `Code`.
#[derive(Debug)]
pub struct BoardError(pub String, pub Code);

impl fmt::Display for BoardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BoardError {}

/// SQLITE_BUSY (the whole file is locked) or SQLITE_LOCKED (a table is, inside a shared
/// connection): both mean another connection holds the lock right now — nothing to do with
/// whether the file itself is writable.
fn is_contended(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(err, _)
            if matches!(err.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

/// The hint for a real path problem (cannot open, read-only, no such file/directory, …):
/// TB_DB is worth naming, but only when `tb_db` says it is actually set — otherwise it was
/// never the pin, and naming it points at the wrong thing (#105). Never says "the board
/// file": `position_error` already names the real file for a house message this same `From`
/// impl also carries (`bad_position`'s FromSqlConversionFailure), and a second, vaguer file
/// reference tacked onto that one is not just redundant — `position_guard.rs` pins that no
/// refusal may print an unusable "the board file" placeholder in place of the real path.
fn db_error_hint(tb_db: Option<&str>) -> String {
    match tb_db {
        Some(path) => format!("check TB_DB ({path}) points at a writable file"),
        None => "check it is writable".to_string(),
    }
}

impl From<rusqlite::Error> for BoardError {
    fn from(e: rusqlite::Error) -> Self {
        if access::is_readonly_error(&e) {
            return access::refusal("that command");
        }
        if is_contended(&e) {
            // the file is fine — another `tb` is mid-write and holds the lock; TB_DB is not
            // the problem here, so it is not named (#105)
            return BoardError("database is locked — another tb is writing this board right now: wait a moment and try again".to_string(), Code::DbError);
        }
        if db_error_is_constraint(&e) {
            // the write reached the database, so the file was fine — the DATA was rejected
            // (a FOREIGN KEY with no row behind it, a UNIQUE index, …). The "is it writable"
            // hint would send somebody with a data problem off to check permissions.
            return BoardError(format!("database error: {e} — the board refused this write on its data, not its file"), Code::DbError);
        }
        BoardError(format!("database error: {e} — {}", db_error_hint(crate::env("DB").as_deref())), Code::DbError)
    }
}

/// A constraint the DATABASE refused (`FOREIGN KEY constraint failed`, a UNIQUE index, …) is
/// a data problem, not a filesystem one — the board file was writable or the write would
/// never have reached the constraint. The generic hint above points the person at the file
/// anyway, which for #138's move sent them looking at permissions while their history was
/// the problem; a constraint names itself instead.
fn db_error_is_constraint(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation)
}

pub type Result<T> = std::result::Result<T, BoardError>;

/// The refusal for changing someone else's DOING card: what it is held by, what the actor
/// holds, and the escape hatch. `what` finishes "to … anyway": `move it to review`,
/// `delete it`, `edit it`.
fn ownership_err(tx: &Connection, id: i64, owner: &str, actor: &str, what: &str) -> Result<BoardError> {
    let mine: Vec<i64> = {
        let mut st =
            tx.prepare(r#"SELECT id FROM cards WHERE "column"='doing' AND owner=? COLLATE NOCASE ORDER BY id"#)?;
        let v = st.query_map([actor], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let yours = if mine.is_empty() { "none".to_string() } else { mine.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(", ") };
    Ok(BoardError(format!(
        "#{id} is held by {owner} — your cards: {yours} · to {what} anyway use --force (logged)"
    ), Code::NotOwner))
}

/// The actor-aware WIP message: the board-wide limit with who holds what, and what the
/// actor can actually do (finish their own card, or wait — never finish someone else's).
fn wip_full_err(conn: &Connection, doing: i64, wip: i64, actor: &str) -> BoardError {
    let holders: Vec<String> = {
        let mut st = conn
            .prepare(r#"SELECT id, owner FROM cards WHERE "column"='doing' ORDER BY id"#)
            .unwrap();
        st.query_map([], |r| {
            let id: i64 = r.get(0)?;
            let owner: Option<String> = r.get(1)?;
            Ok(format!("#{id} {}", owner.unwrap_or_else(|| "?".into())))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    let mine = conn
        .query_row(
            r#"SELECT id FROM cards WHERE "column"='doing' AND owner=? COLLATE NOCASE LIMIT 1"#,
            [actor],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .unwrap_or(None);
    let tail = match mine {
        Some(id) => format!("finish #{id} with 'tb done {id}' first"),
        None => "you hold none; wait, or ask one of them to finish".to_string(),
    };
    BoardError(format!("doing is full ({doing}/{wip}: {}) — {tail}", holders.join(", ")), Code::WipFull)
}

/// Who did the work on a card: its OWNER — the agent that held it in DOING — whenever it has
/// one. Only a card that reached REVIEW with no owner falls back to whoever moved it there,
/// and never to the `github` sync (its moves are automation, not work).
///
/// Keying on the owner and not on the last mover is what makes the never-self-approve rule
/// point at the right agent: a reviewer who pushes a stuck card into REVIEW does not inherit
/// the work, and the worker who held the card cannot escape the rule by letting someone else
/// move it.
/// One guard on the way into DONE that a change does not satisfy: which rule, its refusal, and
/// the `force` event text written when `--force` gets past it.
pub(crate) struct DoneCheck {
    pub rule: &'static str,
    pub err: BoardError,
    pub forced: String,
}

/// Every guard a move of card `c` into DONE by `actor` would fail, in the order
/// `transition_inner` applies them (see its doc comment). The ONE list both the transition and
/// the full-screen board's force prompt read, so they can never disagree about what a forced
/// close skips.
///
/// - review-first (store/verifier.rs, rule 1): only from REVIEW — binds everyone.
/// - self-approval: never the card's author or last holder. Every way into DONE is guarded,
///   not just REVIEW -> DONE (#55, the LAUNDERING hole: moving a card out of review first must
///   not be a way round it), and `last_holder_of` closes the DROPPED-WORK hole: a drop clears
///   the owner, but the agent that held the card still did the work.
/// - the verifier rule (store/verifier.rs, rule 2): an agent needs a verifier role or a place on
///   `config verifiers`; a person always qualifies. Checked after self-approval: a verifier's own
///   work is still its own work.
/// - `done-by` (store/closing.rs): an honest-mistake stop, never security. `github` is exempt.
/// - `done-needs-note` (store/closing.rs): a note written during the stay being left. `github`
///   is exempt — a merged PR is its own trace.
/// - `done-needs-link` (store/links.rs): a link with the required label. `github` is exempt.
fn done_checks(conn: &Connection, c: &Card, actor: &str) -> Result<Vec<DoneCheck>> {
    let id = c.id;
    let mut v = Vec::new();
    if c.column != "review" {
        v.push(DoneCheck {
            rule: "review first",
            err: verifier::not_from_review(id, &c.column),
            forced: format!("closed #{id} from {}, skipping review", c.column),
        });
    }
    let self_approving = author_of(conn, c)?.is_some_and(|a| a.eq_ignore_ascii_case(actor))
        || last_holder_of(conn, id)?.is_some_and(|a| a.eq_ignore_ascii_case(actor));
    if self_approving {
        v.push(DoneCheck {
            rule: "never approve your own work",
            err: BoardError("you did this work — ask another person or agent to review it".to_string(), Code::SelfApprove),
            forced: "approved own work".to_string(),
        });
    }
    let who = actors::current();
    if !verifier::may_verify(conn, actor, &who)? {
        v.push(DoneCheck {
            rule: "only a verifier closes",
            err: verifier::not_verifier_err(id, actor, &who),
            forced: format!("closed #{id} with no verifier role"),
        });
    }
    if let Some((session, builder)) = verifier::same_session_of(conn, id, &who)? {
        v.push(DoneCheck {
            rule: "same session as the builder",
            err: verifier::same_session_err(id, &session, &builder),
            forced: format!("closed #{id} in the builder's same_session"),
        });
    }
    if let Some(names) = closing::may_close(conn, actor)? {
        v.push(DoneCheck { rule: "done-by", err: closing::not_allowed(id, actor, &names), forced: format!("closed #{id}, not on the done-by list") });
    }
    if actor != "github" && closing::needs_note(conn, id)? {
        v.push(DoneCheck {
            rule: "done-needs-note",
            err: closing::no_note_err(id),
            forced: format!("closed #{id} with no note since it entered {}", c.column),
        });
    }
    if actor != "github" {
        if let Some(label) = links::required_label(conn)? {
            if !links::has_label(conn, id, &label)? {
                v.push(DoneCheck {
                    rule: "done-needs-link",
                    err: links::missing_link_err(id, &label),
                    forced: format!("closed #{id} without a link labeled {label}"),
                });
            }
        }
    }
    Ok(v)
}

/// Who did the work on this card: its owner, or for an unowned card whoever moved it into
/// review. A verifier that sends a card back (review -> todo, which clears the owner) and later
/// moves it into review again is checking the work, not doing it: when someone else moved the
/// card into review before, that move is skipped and the earlier mover is the author. A
/// verifier that is the only one ever to move the card into review is still its author.
/// A verifier here is a move whose recorded role is `verifier`/`reviewer`, or a name on
/// `config verifiers`.
fn author_of(conn: &Connection, c: &Card) -> Result<Option<String>> {
    if c.owner.is_some() {
        return Ok(c.owner.clone());
    }
    let listed = verifier::verifiers_of(conn)?;
    let mut stmt = conn.prepare(
        "SELECT e.actor, a.role FROM events e LEFT JOIN actors a ON a.id = e.actor_id
         WHERE e.card_id=? AND e.kind='moved' AND e.text LIKE '% -> review' ORDER BY e.id DESC",
    )?;
    let movers: Vec<(String, Option<String>)> =
        stmt.query_map([c.id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let is_verifier = |(actor, role): &(String, Option<String>)| {
        role.as_deref().is_some_and(|r| verifier::ROLES.iter().any(|v| v.eq_ignore_ascii_case(r.trim())))
            || listed.iter().any(|n| n.eq_ignore_ascii_case(actor.trim()))
    };
    let mover = movers.iter().find(|m| !is_verifier(m)).or(movers.first()).map(|(a, _)| a.clone());
    Ok(match mover {
        Some(a) if a != "github" => Some(a),
        _ => None,
    })
}

/// Who most recently HELD this card in DOING, by EITHER route (`tb next`/`tb take`, a
/// `taken` event, or `tb assign`, an `assigned` event) — unlike `author_of`, dropping the
/// card does not erase this: a drop logs a `dropped` event, not a `taken`/`assigned` one, so
/// the last holder stays on record until someone else claims or is assigned it. Used
/// alongside `author_of` by the self-approval guard in `transition` to close the drop-then-
/// reassign hole (#55): `author_of` alone forgets who held the card once it is dropped and
/// unowned, and credits whoever happens to move the orphaned card into review instead.
///
/// An `assigned` event's own `actor` column is who ASSIGNED the card (the orchestrator), not
/// who now holds it — `transition`'s `Kind::Assign` arm logs it that way on purpose, so `tb
/// show`/`tb log` can answer "who assigned this" and "who holds this" as two different
/// questions. So for that kind the holder's name is read out of the event's structured
/// `assignee` column (written by `log_assign`) instead of `actor`.
///
/// `assignee` is a STRUCTURED field, never parsed out of `text` — #111: a reviewer showed that
/// reading the holder by string-matching "assigned to NAME" out of the event's human-readable
/// prose means changing only the write-site's wording (not on purpose, or not noticing this
/// function also depends on it) silently stops the guard from recognizing the assignee at all,
/// with no compiler error and no failing test, reopening the self-approval bypass on an
/// assign-then-drop-then-force-move path. A structured column cannot be defeated by a reworded
/// message: `mod tests`' `a_reworded_assign_event_does_not_change_who_last_held_the_card` pins
/// exactly that — it rewrites an `assigned` event's `text` to something unrecognizable and
/// checks the refusal is unchanged.
///
/// Migration: `assignee` is NULL on every `assigned` event written before this column existed
/// (existing boards' only record of that holder is the old prose) — for those rows ONLY, this
/// falls back to parsing `text` exactly as before, so an upgraded board's history is not
/// silently forgotten. Every `assigned` event written from here on always sets `assignee`, and
/// once it is set this never looks at `text` again.
fn last_holder_of(conn: &Connection, id: i64) -> Result<Option<String>> {
    let row: Option<(String, String, String, Option<String>)> = conn
        .query_row(
            "SELECT kind, actor, text, assignee FROM events WHERE card_id=? AND kind IN ('taken','assigned') ORDER BY id DESC LIMIT 1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    Ok(row.and_then(|(kind, actor, text, assignee)| {
        if kind == "assigned" {
            // the structured column, set by every `assigned` event `log_assign` writes; the
            // text parse only ever runs for a pre-migration row that never got one
            assignee.or_else(|| text.strip_prefix("assigned to ").map(str::to_string))
        } else {
            Some(actor)
        }
    }))
}

pub(crate) fn err<T>(msg: impl Into<String>, code: Code) -> Result<T> {
    Err(BoardError(msg.into(), code))
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Card {
    pub id: i64,
    pub title: String,
    pub tag: Option<String>,
    pub description: String,
    pub column: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub gh_ref: Option<i64>,
    pub created_at: i64,
    pub column_since: i64,
    /// Why the card is blocked (e.g. `#7`); None when not blocked.
    pub blocked: Option<String>,
    /// `--on`: who or what it waits for — `#7` or a name (`store::blocks`); None without one.
    #[serde(default)]
    pub blocked_on: Option<String>,
    /// `--until`: a local calendar date to look again (`store::blocks`); None without one.
    #[serde(default)]
    pub blocked_until: Option<String>,
    /// Order within its column (0 = top).
    #[serde(default)]
    pub position: i64,
    /// Who claimed the card for review (`tb next --review`); None when unclaimed.
    #[serde(default)]
    pub reviewer: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckItem {
    pub idx: i64,
    pub text: String,
    pub done: bool,
}

/// JSON shape of a checklist item, identical in `tb board --json` and `tb show --json`:
/// `n` is the item number; `idx` is a deprecated alias kept so existing readers don't break.
impl Serialize for CheckItem {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("CheckItem", 4)?;
        st.serialize_field("n", &self.idx)?;
        st.serialize_field("idx", &self.idx)?;
        st.serialize_field("text", &self.text)?;
        st.serialize_field("done", &self.done)?;
        st.end()
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Event {
    pub card_id: i64,
    pub ts: i64,
    pub actor: String,
    pub kind: String,
    pub text: String,
    /// The identity behind `actor` (`actors.id`, see `store::actors`); None when nothing but
    /// the name is known, and on every event written before identities were recorded.
    pub actor_id: Option<i64>,
}

/// One row of `Store::for_each_log_event`: a card event, or a board-level one (no card —
/// `card_id` is `None` everywhere this is rendered). See `board_events` and #106.
pub enum LogEvent {
    Card(Event),
    Board { ts: i64, actor: String, kind: String, text: String, actor_id: Option<i64> },
}

/// An event with its database id (for `tb watch --events` resumption).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WatchEvent {
    pub id: i64,
    #[serde(flatten)]
    pub event: Event,
}

#[derive(Debug, Clone, Serialize)]
pub struct CardDetail {
    #[serde(flatten)]
    pub card: Card,
    pub checklist: Vec<CheckItem>,
    pub events: Vec<Event>,
    /// Rework round: 1, plus one per send-back (`returned` event).
    pub round: i64,
    /// Sent back more times than `config max-rounds` allows (`store::rounds`) — derived, never
    /// stored, always false once `done`.
    pub escalate: bool,
    /// Everyone who recorded `tb done ID --approve`, oldest first (`store::closing`).
    pub approved_by: Vec<String>,
    /// The identities behind this card's events (`Event::actor_id`), in id order.
    pub actors: Vec<actors::Actor>,
    /// Evidence attached with `tb link` (`store::links`), in the order they were added.
    pub links: Vec<links::LinkItem>,
}

/// Everything a board render needs, loaded in one go.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub cards: Vec<Card>,
    pub wip: i64,
    /// last note text per card
    pub last_note: HashMap<i64, String>,
    /// unix ts of each card's last event (any kind)
    pub last_event_at: HashMap<i64, i64>,
    /// last two events per card, oldest first
    pub recent: HashMap<i64, Vec<Event>>,
    /// (done, total) checklist counts per card
    pub checks: HashMap<i64, (i64, i64)>,
    /// rework round per card that was sent back at least once (2 = back once)
    pub rounds: HashMap<i64, i64>,
    /// UI theme: `dark` (default) or `light`
    pub theme: String,
    /// Board name
    pub board: String,
    /// View preference: auto (default), focus, third-h, third-v, half-h or half-v
    pub layout: String,
    /// Panel visibility (`config github-panel` / `agents-panel`, or G / A on the board).
    /// (false by default, so a default Snapshot shows both.)
    pub github_panel_hidden: bool,
    pub agents_panel_hidden: bool,
    pub now: i64,
    /// Each actor's latest card event, keyed by the lowercased name (`roster`: who is on
    /// this board even while holding no card).
    pub actor_last: HashMap<String, Event>,
    /// The board's `sort` (`store::order`): what `in_column` orders by. Default = position.
    pub sort: order::Sort,
    /// The board's look (`store::display`): card line, column labels, the due mark's today.
    /// Default = the look tb always had.
    pub display: display::Display,
    /// What each card's block means today (`store::blocks`), and whether blocked cards get
    /// their own WAITING section. Default = no block detail and no lane: today's board.
    pub blocks: blocks::BlockCtx,
    pub waiting_lane: bool,
}

/// The board's DONE column only shows cards finished in the last 24h.
pub const DONE_WINDOW_SECS: i64 = 86400;

impl Snapshot {
    /// Cards shown on the board: like `in_column`, but `done` is limited to the last 24h.
    pub fn on_board(&self, col: &str) -> Vec<&Card> {
        let mut v = self.in_column(col);
        if col == "done" {
            v.retain(|c| self.now - c.column_since < DONE_WINDOW_SECS);
        }
        v
    }

    pub fn in_column(&self, col: &str) -> Vec<&Card> {
        let mut v: Vec<&Card> = self.cards.iter().filter(|c| c.column == col).collect();
        // the one ordering (store/order.rs) — the same function `tb next` picks with
        v.sort_by(|a, b| order::cmp(self.sort, a, b));
        v
    }
}

/// The unparsed `TB_NOW` / `TTYBOARD_NOW` (unix seconds): `None` when unset or empty. Any
/// other value — text that is not an integer, or one outside `TB_NOW_MIN..TB_NOW_MAX` (the
/// upper bound is EXCLUSIVE, so 4102444799 is the last second accepted) — is refused by
/// `pinned_now()` (exit 1, nothing written): it would be written into a real board as fact.
/// No cfg gate: the suite pins the clock on the release binary, and `now()` is a library
/// read, not a validated `--json` argument, so the refusal is raised by `pinned_now()` at
/// the top of `main`'s `run()`, before any command is dispatched.
pub const TB_NOW_MIN: i64 = 946_684_800; // 2000-01-01T00:00:00Z
pub const TB_NOW_MAX: i64 = 4_102_444_800; // the first refused second (one past the window)

/// The clock to use: the pinned `TB_NOW` when it is set and in range, else the real clock.
pub fn pinned_now() -> Result<Option<i64>> {
    let Some(raw) = crate::env("NOW") else { return Ok(None) };
    let v = raw.trim();
    match v.parse::<i64>() {
        Ok(t) if (TB_NOW_MIN..TB_NOW_MAX).contains(&t) => Ok(Some(t)),
        _ => Err(BoardError(format!(
            "TB_NOW is not a plausible unix second: '{v}' — unset it, or pass seconds between {TB_NOW_MIN} and {TB_NOW_MAX} (2000, last accepted 4102444799)"
        ), Code::InvalidValue)),
    }
}

pub fn now() -> i64 {
    match crate::env("NOW") {
        Some(v) => v.trim().parse::<i64>().unwrap_or_else(|_| chrono::Utc::now().timestamp()),
        None => chrono::Utc::now().timestamp(),
    }
}

/// `parse_title` without the tag guess: the whole string stays the title (minus a leading
/// `gh#N`). What an explicit `--tag` uses, so a title like `due 10/9 (file by 10/6): …` is
/// kept exactly as it was typed.
pub fn parse_title_keeping_prefix(raw: &str) -> (Option<String>, Option<i64>, String) {
    let (_, gh, _) = parse_title(raw);
    let mut words: Vec<&str> = raw.split_whitespace().collect();
    if let Some(first) = words.first() {
        let lower = first.to_ascii_lowercase();
        if lower.strip_prefix("gh#").and_then(|n| n.parse::<i64>().ok()).is_some() {
            words.remove(0);
        }
    }
    let title = if words.is_empty() { raw.trim().to_string() } else { words.join(" ") };
    (None, gh, title)
}

/// Split `tag: rest` and `gh#N` out of a title.
/// `widgets: gh#327 login form rejects` -> (tag=widgets, gh=327, title="login form rejects")
pub fn parse_title(raw: &str) -> (Option<String>, Option<i64>, String) {
    let raw = raw.trim();
    let mut tag = None;
    let mut rest = raw;
    if let Some((head, tail)) = raw.split_once(':') {
        let head_ok = !head.is_empty()
            && head.len() <= 20
            && head
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if head_ok && tail.starts_with(' ') && !tail.trim().is_empty() {
            tag = Some(head.to_ascii_lowercase());
            rest = tail.trim();
        }
    }
    // Only a LEADING gh#N (the first word of the rest) is moved out of the title — the
    // conventional link token. A gh#N later in the sentence stays in the text; it still
    // sets the link when no leading one exists (documented in JSON.md).
    let mut gh = None;
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    if let Some(first) = words.first() {
        let lower = first.to_ascii_lowercase();
        if let Some(n) = lower.strip_prefix("gh#").and_then(|n| n.parse::<i64>().ok()) {
            gh = Some(n);
            words.remove(0);
        }
    }
    if gh.is_none() {
        for w in &words {
            let lower = w.to_ascii_lowercase();
            if let Some(n) = lower.strip_prefix("gh#").and_then(|n| n.parse::<i64>().ok()) {
                gh = Some(n);
                break; // the link is set, the words stay
            }
        }
    }
    let title = if words.is_empty() { rest.to_string() } else { words.join(" ") };
    (tag, gh, title)
}


pub struct Store {
    conn: Connection,
    /// Board name shown in headers.
    pub name: String,
    /// The SHARED lock on this board's slot, held for as long as this `Store` lives (dropped
    /// with it). `None` for an in-memory board (`:memory:`) and for a read-only open, which
    /// never creates or moves anything — see `Store::open`'s doc comment.
    _lock: Option<lock::Guard>,
    /// This open created the board file (see `creator`: only then is a creator recorded).
    created: bool,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS cards (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    tag TEXT,
    description TEXT NOT NULL DEFAULT '',
    "column" TEXT NOT NULL DEFAULT 'todo' CHECK ("column" IN ('todo','doing','review','done')),
    owner TEXT,
    due TEXT,
    gh_ref INTEGER,
    created_at INTEGER NOT NULL,
    column_since INTEGER NOT NULL,
    blocked TEXT,
    position INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS cards_column ON cards("column");
CREATE TABLE IF NOT EXISTS checklist (
    card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    text TEXT NOT NULL,
    done INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (card_id, idx)
);
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    ts INTEGER NOT NULL,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL,
    text TEXT NOT NULL DEFAULT '',
    assignee TEXT
);
CREATE INDEX IF NOT EXISTS events_card ON events(card_id, id);
CREATE TABLE IF NOT EXISTS links (
    card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    label TEXT NOT NULL,
    value TEXT NOT NULL,
    added_by TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (card_id, idx)
);
CREATE TABLE IF NOT EXISTS github_snapshot (
    key INTEGER PRIMARY KEY CHECK (key = 1),
    fetched_at INTEGER NOT NULL DEFAULT 0,
    json TEXT,
    error TEXT,
    fails INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS board_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL,
    text TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

const CARD_COLS: &str =
    r#"id, title, tag, description, "column", owner, due, gh_ref, created_at, column_since, blocked, position, reviewer, blocked_on, blocked_until"#;

fn row_card(r: &rusqlite::Row) -> rusqlite::Result<Card> {
    // `position` is tb's business (`docs/SCHEMA.md`), but a file written by something else can
    // hold anything there. Read it as its stored type first: a value that is not a whole
    // number is not tolerated into an i64 (that surfaces as a raw "database error" that names
    // neither card nor file) — it is refused with the board file and the one fix. A foreign
    // real value inside the integer range still enters: the card orders, by_position is total.
    // A NULL reads as the schema default 0 (a foreign tool may have relaxed NOT NULL; a card
    // with no position orders last under `by_position`, ties broken by id, and the next write
    // by tb gives the row a real number).
    let pos_type = r.get_ref(11)?.data_type();
    let pos = match pos_type {
        Type::Integer => r.get::<_, i64>(11),
        Type::Null => Ok(0),
        Type::Real => Ok(r.get_ref(11)?.as_f64().unwrap_or_default() as i64),
        Type::Text => return bad_position(r.get::<_, i64>(0), r.get_ref(11)?.as_str().unwrap_or_default()),
        Type::Blob => {
            return bad_position(r.get::<_, i64>(0), format!("<{} bytes>", r.get_ref(11)?.as_blob().map(|b| b.len()).unwrap_or(0)))
        }
    };
    Ok(Card {
        id: r.get(0)?,
        title: r.get(1)?,
        tag: r.get(2)?,
        description: r.get(3)?,
        column: r.get(4)?,
        owner: r.get(5)?,
        due: r.get(6)?,
        gh_ref: r.get(7)?,
        created_at: r.get(8)?,
        column_since: r.get(9)?,
        blocked: r.get(10)?,
        position: pos?,
        reviewer: r.get(12)?,
        blocked_on: r.get(13)?,
        blocked_until: r.get(14)?,
    })
}

/// The refusal for a position that is not a number: the card, the value as stored, and the
/// ONE fix. Every read path (`next`, `list`, `board`, `show`, the full-screen board, the
/// board picker) reads cards through `row_card`, so they all fail the same way — a house
/// message that names the file, never a raw database error.
fn bad_position<T>(id: rusqlite::Result<i64>, value: impl std::fmt::Display) -> rusqlite::Result<T> {
    let id = id?; // the id of the very row being read: nothing between it and here can fail
    // index usize::MAX renders the message ALONE, without a "Conversion error from type …"
    // prefix (rusqlite treats that index as "unknown column") — the message is the whole error
    Err(rusqlite::Error::FromSqlConversionFailure(
        usize::MAX,
        Type::Integer,
        Box::new(crate::store::BoardError(position_error(id, value), Code::DbError)),
    ))
}

/// The board file the last `Store::open` opened. `row_card` is a row mapper — SQLite hands it
/// a row, never the store that is loading it — so the one message that has to name the file
/// (`position_error`) reads the path from here. It is remembered when the file is OPENED
/// because a board file is chosen four ways (`TB_DB`, a board named on the command line,
/// `TB_BOARD`, the saved default) and only one of them is an environment variable to read
/// back: on a named board `TB_DB` is unset, and a repair command that says "the board file"
/// is a command nobody can run.
static OPEN_BOARD_FILE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Remember the file a board was just opened from (`None` for an in-memory board, which has
/// no file to name).
fn remember_board_file(path: Option<&Path>) {
    if let Ok(mut open) = OPEN_BOARD_FILE.lock() {
        *open = path.map(|p| p.display().to_string()).filter(|p| !p.is_empty());
    }
}

/// The board file a refusal names: the one actually open, else `TB_DB` for a message raised
/// before any board was opened, else a phrase that at least reads as English.
fn board_file() -> String {
    OPEN_BOARD_FILE
        .lock()
        .ok()
        .and_then(|open| open.clone())
        .or_else(|| crate::env("DB").filter(|f| !f.is_empty()))
        .unwrap_or_else(|| "the board file".into())
}

/// The message itself, so the tests can hold it in one place: what is wrong, which FILE, and
/// the one command that fixes it.
fn position_error(id: i64, value: impl std::fmt::Display) -> String {
    let file = board_file();
    format!(
        "card #{id} has a position that is not a number ({value}) — {file} was written by something other than tb; give card #{id} a whole-number position again with: sqlite3 \"{file}\" \"UPDATE cards SET position=0 WHERE id={id}\""
    )
}

fn row_event(r: &rusqlite::Row) -> rusqlite::Result<Event> {
    Ok(Event {
        card_id: r.get(0)?,
        ts: r.get(1)?,
        actor: r.get(2)?,
        kind: r.get(3)?,
        text: r.get(4)?,
        actor_id: r.get(5)?,
    })
}

/// Bring a board's schema up to date: the tables, then every column added since the first
/// release. Idempotent, and the ONLY place a schema change may live — `upgrade` runs it inside
/// a transaction and backs the board file up first whenever it would change an existing board.
fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    // migration: `blocked` arrived after the first release
    let has_blocked: bool = conn
        .query_row("SELECT COUNT(*) FROM pragma_table_info('cards') WHERE name='blocked'", [], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n > 0)?;
    if !has_blocked {
        if let Err(e) = conn.execute_batch("ALTER TABLE cards ADD COLUMN blocked TEXT") {
            if !e.to_string().contains("duplicate column") {
                return Err(e.into());
            }
        }
    }
    // migration: `position` (order within a column) — existing cards ordered by created_at
    let has_pos: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('cards') WHERE name='position'",
        [],
        |r| r.get(0),
    )?;
    if has_pos == 0 {
        match conn.execute_batch("ALTER TABLE cards ADD COLUMN position INTEGER NOT NULL DEFAULT 0") {
            Ok(()) => {
                conn.execute_batch(
                    r#"UPDATE cards SET position = (SELECT COUNT(*) FROM cards c2 WHERE c2."column" = cards."column"
                       AND (c2.created_at < cards.created_at OR (c2.created_at = cards.created_at AND c2.id < cards.id)))"#,
                )?;
            }
            Err(e) if e.to_string().contains("duplicate column") => {}
            Err(e) => return Err(e.into()),
        }
    }
    // migration: github fail counter (red only after 3 consecutive failed refreshes)
    let has_fails: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('github_snapshot') WHERE name='fails'",
        [],
        |r| r.get(0),
    )?;
    if has_fails == 0 {
        match conn.execute_batch("ALTER TABLE github_snapshot ADD COLUMN fails INTEGER NOT NULL DEFAULT 0") {
            Err(e) if e.to_string().contains("duplicate column") => {}
            Err(e) => return Err(e.into()),
            _ => {}
        }
    }
    // migration: `reviewer` (v2, `tb next --review`)
    let has_reviewer: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('cards') WHERE name='reviewer'",
        [],
        |r| r.get(0),
    )?;
    if has_reviewer == 0 {
        if let Err(e) = conn.execute_batch("ALTER TABLE cards ADD COLUMN reviewer TEXT") {
            if !e.to_string().contains("duplicate column") {
                return Err(e.into());
            }
        }
    }
    // migration: `assignee` on `events` (v2, #111) — the `assigned` event's holder, read
    // directly by the self-approval guard (`last_holder_of`) instead of parsed out of `text`'s
    // prose, so a reworded message can never change who the guard refuses. Existing `assigned`
    // rows keep this NULL; `last_holder_of` still falls back to parsing their `text` for those
    // (the only record they have), but every `assigned` event written from here on sets it,
    // and it is the only thing the guard trusts once it is set.
    let has_assignee: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('events') WHERE name='assignee'",
        [],
        |r| r.get(0),
    )?;
    if has_assignee == 0 {
        if let Err(e) = conn.execute_batch("ALTER TABLE events ADD COLUMN assignee TEXT") {
            if !e.to_string().contains("duplicate column") {
                return Err(e.into());
            }
        }
    }
    // migration: `blocked_on` / `blocked_until` (v2, `tb block --on … --until …`)
    blocks::migrate(conn)?;
    // migration: `links` on an `archived_cards` table made before links existed (v2, `tb link`)
    archive::migrate(conn)?;
    Ok(())
}

/// Everything `sqlite_master` says about the schema, as one string: any table, index or
/// column a migration adds changes it.
fn schema_fingerprint(conn: &Connection) -> Result<String> {
    let mut st = conn.prepare("SELECT type, name, tbl_name, COALESCE(sql, '') FROM sqlite_master ORDER BY type, name")?;
    let rows = st
        .query_map([], |r| {
            Ok(format!("{}|{}|{}|{}", r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.join("\n"))
}

fn is_board(conn: &Connection) -> Result<bool> {
    let n: i64 =
        conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='cards'", [], |r| r.get(0))?;
    Ok(n > 0)
}

/// The ONE place `notice`'s board key is computed — `Store::path`/`notice_key` and every
/// `notice::push_for` about THIS connection all call this, never re-derive their own guess
/// from the pre-open `Path`. A hand-rolled `Path::display()` on that path missed a relative
/// input, `TB_DB` passing one through, a path with `..` in it, or the same board opened a
/// second time under a spelling that resolves the same way but is not byte-identical — SQLite
/// resolves the connection's OWN filename once, consistently, and both sides just ask it for
/// that same answer, so a push and its later drain can never disagree, whatever the input
/// looked like. `conn.path()` is available the instant `Connection::open` returns, which is
/// always after the file exists (`create_board` creates it first) — so this never needs to
/// canonicalise a not-yet-created path by hand, the one thing that cannot be done portably.
fn conn_notice_key(conn: &Connection) -> Option<String> {
    conn.path().filter(|p| !p.is_empty()).map(str::to_string)
}

/// Would `migrate` change this board's schema? Asked on a READ-ONLY connection, where the
/// upgrade itself cannot run: a probe that only reads.
fn needs_upgrade(conn: &Connection) -> Result<bool> {
    let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table'", [], |r| r.get(0))?;
    if tables == 0 {
        return Ok(true);
    }
    // every column the current code reads, in one probe: a missing one means an old schema
    let probe = conn.query_row(&format!("SELECT {CARD_COLS} FROM cards LIMIT 1"), [], |_| Ok(()));
    match probe {
        Ok(()) | Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(_) => Ok(true),
    }
}

/// Run `migrate`, and keep a copy of an existing board before its schema changes.
///
/// 1. A dry run in a plain (deferred) transaction that is always rolled back. On an up-to-date
///    board — every command, nearly every time — `migrate` is all no-ops, the write lock is
///    never taken, and that is the end of it.
/// 2. Otherwise ONE critical section, under the board's write lock (`BEGIN IMMEDIATE`), held
///    from the decision to the commit:
///    - probe under the lock (inside a savepoint that is undone): would `migrate` still change
///      this board? A process that waited for the lock while another one upgraded the board
///      finds nothing to do, and does nothing — no backup, no warning;
///    - an existing board that would change is backed up first, through a second connection.
///      `VACUUM INTO` cannot run inside a transaction, and it does not have to: it only READS
///      the board, it reads the last COMMITTED state, and nobody can commit while this
///      connection holds the write lock. So the copy is always the board as the older tb
///      left it, and there is exactly one however many processes open the board at once;
///    - then `migrate`, and COMMIT. A backup that cannot be written ends the transaction
///      with nothing changed and refuses the command.
///
/// The lock is SQLite's own, so a process that dies holding it leaves nothing stale.
///
/// It compares the schema before and after instead of keeping a list of migrations, so a
/// migration written later, by anyone, in any style, is backed up without registering anything.
fn upgrade(conn: &mut Connection, path: &Path, on_disk: bool) -> Result<()> {
    let pending = {
        let tx = conn.unchecked_transaction()?;
        let before = schema_fingerprint(&tx)?;
        // an error here (e.g. the lock was busy mid-run) just means "find out for real below"
        let changed = migrate(&tx).and_then(|()| schema_fingerprint(&tx)).map_or(true, |after| after != before);
        tx.rollback()?;
        changed
    };
    if !pending {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existed = is_board(&tx)?;
    let before = schema_fingerprint(&tx)?;
    tx.execute_batch("SAVEPOINT tb_probe")?;
    migrate(&tx)?;
    let changes = schema_fingerprint(&tx)? != before;
    // undo the probe but keep the transaction — and with it the write lock
    tx.execute_batch("ROLLBACK TO tb_probe; RELEASE tb_probe")?;
    if !changes {
        // another process upgraded the board while this one waited for the lock
        tx.rollback()?;
        return Ok(());
    }
    let backup = if on_disk && existed { Some(backup_aside(path)?) } else { None };
    if let Err(e) = migrate(&tx).and_then(|()| Ok(tx.commit()?)) {
        // the board is unchanged, so a copy made for this upgrade has nothing to go back from
        if let Some(b) = &backup {
            let _ = std::fs::remove_file(b);
        }
        return Err(e);
    }
    if let Some(backup) = backup {
        // keyed via `conn_notice_key`, not `path.display()` — see its doc comment: only the
        // board that raised this ever drains it, whatever the input path looked like
        if let Some(key) = conn_notice_key(conn) {
            crate::notice::push_for(
                &key,
                format!(
                    "{} was written by an older tb: it was backed up to {} before its schema was upgraded — to go back, see \"Going back to an older tb\" in UPGRADING.md",
                    path.display(),
                    backup.display()
                ),
            );
        }
    }
    Ok(())
}

/// Copy the board aside, next to itself: `<file>.before-<tb version>.<UTC date-time>.bak`.
/// The caller holds the board's write lock (see `upgrade`), so one process is here at a time.
///
/// Written by SQLite (`VACUUM INTO`, on a connection of its own), not by copying the file:
/// committed cards may still sit in a hot `-wal`, and a copy of the `.db` alone would lose
/// them. The result is one complete database with no sidecars, created private, that never
/// ends in `.db` (so it is never listed as a board). It is written as `….bak.partial` and
/// renamed when complete, so a file named `….bak` is always a whole backup — a process killed
/// half way leaves a `.partial`, which the next upgrade of that board clears.
fn backup_aside(path: &Path) -> Result<std::path::PathBuf> {
    use chrono::TimeZone;
    let cannot = |why: String| {
        BoardError(format!(
            "cannot back up {} before upgrading it: {why} — nothing was changed; make room next to it (or fix the directory's permissions) and run the command again",
            path.display()
        ), Code::IoError)
    };
    let stamp = chrono::Utc
        .timestamp_opt(now(), 0)
        .single()
        .map(|t| t.format("%Y%m%d-%H%M%S").to_string())
        .unwrap_or_else(|| now().to_string());
    let base = format!("{}.before-{}.{stamp}", path.display(), env!("CARGO_PKG_VERSION"));
    let target = (1..100)
        .map(|n| std::path::PathBuf::from(if n == 1 { format!("{base}.bak") } else { format!("{base}-{n}.bak") }))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .ok_or_else(|| cannot("too many backups with this name".into()))?;
    let partial = std::path::PathBuf::from(format!("{}.partial", target.display()));
    // a leftover from a run that was killed mid-copy; O_EXCL below never follows a link
    let _ = std::fs::remove_file(&partial);
    let _ = std::fs::remove_file(format!("{}-journal", partial.display()));
    let write = || -> std::result::Result<(), String> {
        if !crate::fsperm::create_private(&partial).map_err(|e| e.to_string())? {
            return Err(format!("{} is in the way", partial.display()));
        }
        let to = partial.to_str().ok_or("its path is not valid UTF-8")?;
        let reader = Connection::open(path).map_err(|e| e.to_string())?;
        crate::waits::busy(&reader, BUSY_WAIT).map_err(|e| e.to_string())?;
        reader.execute("VACUUM INTO ?1", [to]).map_err(|e| e.to_string())?;
        drop(reader);
        std::fs::rename(&partial, &target).map_err(|e| e.to_string())
    };
    if let Err(why) = write() {
        let _ = std::fs::remove_file(&partial);
        return Err(cannot(why));
    }
    Ok(target)
}

/// The board a hint should name for the file at `path`: None when a bare `tb` reaches it
/// (`TB_DB` pins it, or it is the default board), else its name in the boards directory.
fn board_of(path: &Path) -> Option<String> {
    if crate::boards::db_pinned() || path.parent() != Some(crate::boards::boards_dir().as_path()) {
        return None;
    }
    let name = path.file_name()?.to_str()?.strip_suffix(".db")?;
    (crate::boards::validate(name).is_ok() && name != crate::boards::default_name()).then(|| name.to_string())
}

/// `tb config …` / `tb NAME config …` for the board in `path`.
fn config_cmd(path: &Path, rest: &str) -> String {
    match board_of(path) {
        Some(name) => format!("'tb {name} config {rest}'"),
        None => format!("'tb config {rest}'"),
    }
}

/// An existing board file that other users can open is REPORTED, never quietly re-moded: it
/// may be shared with a group on purpose. `tb config file-mode private` tightens it (and says
/// so); `tb config file-mode shared` records that it is meant to be, which ends the report.
fn report_wide_file(conn: &Connection, path: &Path, real: &Path) {
    let Some(mode) = crate::fsperm::mode_of(real).filter(|m| crate::fsperm::is_wide(*m)) else { return };
    let shared = conn
        .query_row("SELECT value FROM config WHERE key='file-mode'", [], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .is_some_and(|v| v == "shared");
    if shared {
        return;
    }
    // keyed via `conn_notice_key`, not `real.display()` — see its doc comment on `upgrade`
    let Some(key) = conn_notice_key(conn) else { return };
    crate::notice::push_for(
        &key,
        format!(
            "{} is open to other users (mode {}) — make it private with {}, or keep it that way with {}",
            real.display(),
            crate::fsperm::fmt_mode(mode),
            config_cmd(path, "file-mode private"),
            config_cmd(path, "file-mode shared"),
        ),
    );
}

/// How long a connection waits for another `tb` to finish writing the same board.
const BUSY_WAIT: Duration = Duration::from_secs(10);

/// Put a board into WAL mode. The switch needs SQLite's exclusive lock on the file, and SQLite
/// does not run the busy handler for it: while another `tb` has the file open it returns
/// SQLITE_BUSY at once instead of waiting out `BUSY_WAIT`. That only happens on a board that
/// is not in WAL mode yet — one being created — and every process that opens a new board
/// races for it, so several `tb NAME add` started together on a new board used to fail with
/// "database is locked". Retry the switch (it holds no lock between tries) until it succeeds or
/// `BUSY_WAIT` runs out; once any process has made the switch it is recorded in the file and
/// the pragma returns at once for everyone else.
fn set_wal(conn: &Connection) -> Result<()> {
    let deadline = std::time::Instant::now() + BUSY_WAIT;
    let mut pause = Duration::from_millis(1);
    loop {
        match conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0)) {
            Ok(_) => return Ok(()),
            Err(e) if is_contended(&e) && std::time::Instant::now() < deadline => {
                crate::waits::pause("wal switch", pause);
                pause = (pause * 2).min(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// How long `Store::open` waits for a stuck EXCLUSIVE holder (a move that never finished) to
/// let go, and how long `lock_for_move` waits for every SHARED holder (every open `Store` on
/// that board) to close. Both bounded, both overridable in tests with `TB_LOCK_WAIT_MS` — the
/// ordinary case never gets near either: an uncontended `flock` returns immediately.
fn lock_wait() -> Duration {
    match crate::env("LOCK_WAIT_MS").and_then(|s| s.parse().ok()) {
        Some(ms) => Duration::from_millis(ms),
        None => Duration::from_secs(10),
    }
}

/// `lock::Error` into a `BoardError` naming the board and what the caller was doing
/// (`"open"` or `"move"`), with `Code::BoardBusy` — never the catch-all, so a `--json` caller
/// can tell "still open somewhere" from every other refusal.
fn lock_err(doing: &str, path: &Path, e: lock::Error) -> BoardError {
    match e {
        lock::Error::Io(io) => BoardError(format!("cannot lock '{}': {io}", path.display()), Code::IoError),
        lock::Error::Busy(pids) => {
            let who = if pids.is_empty() {
                "close whatever process has it open".to_string()
            } else if pids.len() == 1 {
                format!("close process {} (see 'kill {}' if it is stuck)", pids[0], pids[0])
            } else {
                format!("close these processes: {}", pids.iter().map(i32::to_string).collect::<Vec<_>>().join(", "))
            };
            BoardError(
                format!(
                    "cannot {doing} '{}': still open in another process — {who}, then try again",
                    path.display()
                ),
                Code::BoardBusy,
            )
        }
    }
}

/// The EXCLUSIVE lock for `path`'s slot: waits for every live `Store::open` on it (the SHARED
/// half) to close, and for any other move already in progress to finish first. A command that
/// ARCHIVES, RESTORES or otherwise REPLACES the file a board name points at must hold this
/// across the WHOLE operation — from the first look at what is there to the last file removed
/// or put in place — so no `Store::open` can land in the middle and either recreate what is
/// being retired or open what has not finished being put back. On timeout, refuses naming
/// which process to close where that can be determined (`crate::lock`, best-effort via Linux's
/// `/proc/locks`).
pub fn lock_for_move(path: &Path) -> Result<lock::Guard> {
    lock::take(&lock::sibling(path), lock::Mode::Exclusive, lock_wait()).map_err(|e| lock_err("move", path, e))
}

/// The same EXCLUSIVE lock as `lock_for_move`, for a command that DELETES the file at `path`:
/// it waits for every `Store::open` on it to close, and refuses (naming the holder where it
/// can) past the wait, so a board another tb still has open is never removed under it.
pub fn lock_for_delete(path: &Path) -> Result<lock::Guard> {
    lock::take(&lock::sibling(path), lock::Mode::Exclusive, lock_wait()).map_err(|e| lock_err("delete", path, e))
}

/// Place `from` at `to` without ever clobbering an existing file there: hard-link `from` into
/// `to` — refused by the filesystem itself, atomically, if `to` already exists (`EEXIST`),
/// rather than tb checking "is anything there" first and racing its own answer — then remove
/// `from`. A future `restore` (or anything else that revives a retired board file) must use
/// this, never a plain rename: a board that a concurrent `add` created afresh while the caller
/// waited for `lock_for_move` is left exactly as that `add` left it, never overwritten. Callers
/// hold `lock_for_move(to)` across both steps (this function does not take it itself, so a
/// caller placing several sidecars — `.db`, `-wal`, `-shm` — can do it all under one lock).
pub fn link_into_place(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::hard_link(from, to)?;
    std::fs::remove_file(from)
}

/// Seed spec for test fixtures (explicit column, age, checklist, notes).
pub struct Seed<'a> {
    pub title: &'a str,
    pub desc: &'a str,
    pub column: &'a str,
    pub owner: Option<&'a str>,
    pub age_secs: i64,
    pub due: Option<&'a str>,
    pub checks: &'a [(&'a str, bool)],
    pub notes: &'a [(&'a str, &'a str)],
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        // `:memory:` (and SQLite's unnamed temporary database) have no file to look after
        let on_disk = !path.as_os_str().is_empty() && path.as_os_str() != ":memory:";
        // The SHARED lock, held for as long as this `Store` lives — taken BEFORE anything
        // below so much as asks whether the file exists. A command that MOVES or REPLACES a
        // board file (`lock_for_move`) holds the EXCLUSIVE half of this same lock across the
        // whole move, so this wait either returns instantly (nobody is moving anything, the
        // ordinary case) or blocks until that move is fully done — and only then do we look:
        // never mid-move, never a half-moved file, never "not there yet" when it is about to
        // be put back. See the module doc comment for the whole shape.
        //
        // A genuine timeout (`Error::Busy`: the lock file opened fine, something else holds
        // it EXCLUSIVE past the wait) refuses — that is the whole point. But an outright
        // failure to even open or create the `.lock` file (`Error::Io`: no permission, a
        // read-only boards directory) degrades to opening WITHOUT the lock, exactly the
        // pre-#112 behavior, rather than turning "the directory is read-only" into "boards
        // cannot be read at all" — a regression nothing here should introduce. A deployment
        // that cannot create this file cannot take the EXCLUSIVE half either, so nothing
        // capable of racing this open is possible there in the first place.
        let _lock = if !on_disk {
            None
        } else {
            match lock::take(&lock::sibling(path), lock::Mode::Shared, lock_wait()) {
                Ok(g) => Some(g),
                Err(lock::Error::Busy(pids)) => return Err(lock_err("open", path, lock::Error::Busy(pids))),
                Err(lock::Error::Io(_)) => None,
            }
        };
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| {
                    BoardError(format!(
                        "cannot create {}: {e} — set TB_DB to a writable path",
                        dir.display()
                    ), Code::IoError)
                })?;
            }
        }
        // A board file is born private (0600): SQLite then opens the empty file as a new
        // database and gives the -wal/-shm sidecars the same mode. An existing file is never
        // re-moded here; `report_wide_file` says so instead. `real` is where the board lives:
        // `path` itself, or the end of its chain of symbolic links — tb creates THAT file
        // (SQLite would create a link's missing target 0644) and opens the database there.
        let (real, created) = if on_disk {
            crate::fsperm::create_board(path).map_err(|e| BoardError(e, Code::IoError))?
        } else {
            (path.to_path_buf(), false)
        };
        // the file every later refusal names (`position_error`): where the board really is,
        // whether it was named by `TB_DB`, by `-b NAME`, by `TB_BOARD` or by the saved default
        remember_board_file(on_disk.then_some(real.as_path()));
        // In read-only mode the DATABASE is opened read-only. There are eighty-odd write
        // sites in this crate, and a list of them is a list somebody forgets to add to; this
        // way a write that slips past the command layer fails at SQLite instead of landing.
        // (See `store::access`. The command layer refuses first, with a better message.)
        let readonly = access::readonly_env();
        let mut conn = if readonly && on_disk {
            use rusqlite::OpenFlags;
            Connection::open_with_flags(&real, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?
        } else {
            Connection::open(&real)?
        };
        crate::waits::busy(&conn, BUSY_WAIT)?;
        if readonly {
            // a read-only connection cannot upgrade the schema; say so plainly rather than
            // failing later in SQLite's own words
            conn.execute_batch("PRAGMA foreign_keys=ON")?;
            if on_disk && needs_upgrade(&conn)? {
                return Err(BoardError(
                    format!(
                        "board '{}' was made by an older tb and needs an upgrade, which read-only mode cannot do — run any command without TB_READONLY (or --read-only) once, then read it",
                        real.display()
                    ),
                    Code::ReadOnly,
                ));
            }
            return Ok(Store { conn, name: crate::boards::DEFAULT_BOARD.into(), _lock, created });
        }
        set_wal(&conn)?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;")?;
        upgrade(&mut conn, &real, on_disk)?;
        if on_disk && !created {
            report_wide_file(&conn, path, &real);
        }
        // migration: who did the work (`actors`, `events.actor_id`, `board_events.actor_id`)
        actors::migrate(&conn)?;
        // migration: who made the board (`board_creator`, #137)
        creator::migrate(&conn)?;
        Ok(Store { conn, name: crate::boards::DEFAULT_BOARD.into(), _lock, created })
    }

    /// Like [`open`](Self::open), for a caller that must NEVER conjure a missing board:
    /// `Ok(None)` when the file genuinely is not there, rather than `open`'s usual
    /// create-if-missing.
    ///
    /// A plain `path.exists()` before calling `open` is NOT enough — it is a second,
    /// unlocked look at the filesystem, so `archive`/`restore` (#80) can land in the gap
    /// between that check finding the board there and `open` actually running, and `open`
    /// unconditionally creates what it does not find. That gap is exactly door 1 from #112,
    /// reopened one layer up: a command policy never allows to create a board (`tb NAME note`,
    /// `tb NAME show`, …) would resurrect an empty one anyway, which then blocks `restore`
    /// with "already exists" — the very failure this feature exists to remove. Measured: a
    /// `note` writer racing `boards archive` resurrected an empty board in up to 19 of 100
    /// rounds before this existed.
    ///
    /// The fix takes the SAME SHARED lock `open` takes, keeps it held across BOTH the
    /// existence check and the call to `open` below (an `flock` process may hold any number
    /// of SHARED guards on the same file without blocking itself, so re-taking it inside
    /// `open` is not a second wait) — so no EXCLUSIVE `lock_for_move` can land in that gap
    /// either. Degrades exactly like `open` when the lock file itself cannot be used
    /// (`lock::Error::Io`): no lock, no gap-closing, the pre-#112 behavior.
    pub fn open_if_exists(path: &Path) -> Result<Option<Store>> {
        let on_disk = !path.as_os_str().is_empty() && path.as_os_str() != ":memory:";
        if !on_disk {
            return Self::open(path).map(Some);
        }
        let _hold = match lock::take(&lock::sibling(path), lock::Mode::Shared, lock_wait()) {
            Ok(g) => Some(g),
            Err(lock::Error::Busy(pids)) => return Err(lock_err("open", path, lock::Error::Busy(pids))),
            Err(lock::Error::Io(_)) => None,
        };
        if !path.is_file() {
            return Ok(None);
        }
        Self::open(path).map(Some)
    }

    /// The database file (None for an in-memory board).
    pub fn path(&self) -> Option<std::path::PathBuf> {
        conn_notice_key(&self.conn).map(std::path::PathBuf::from)
    }

    /// The key `notice` warnings about this board are pushed and drained under (see
    /// `conn_notice_key`) — the same connection, so it is the same string `path()` reports,
    /// every time, for any input path shape.
    pub fn notice_key(&self) -> Option<String> {
        conn_notice_key(&self.conn)
    }

    /// Changes whenever another connection commits (for `watch`).
    pub fn data_version(&self) -> Result<i64> {
        Ok(self.conn.query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }

    /// Set the board name shown in headers.
    pub fn named(mut self, name: &str) -> Store {
        self.name = name.to_string();
        self
    }

    /// Order every column by time (todo: created, others: entered) — for seeded fixtures.
    pub fn order_by_time(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"UPDATE cards SET position = (SELECT COUNT(*) FROM cards c2 WHERE c2."column" = cards."column" AND
                 (CASE WHEN cards."column" = 'todo' THEN c2.created_at < cards.created_at OR (c2.created_at = cards.created_at AND c2.id < cards.id)
                       ELSE c2.column_since < cards.column_since OR (c2.column_since = cards.column_since AND c2.id < cards.id) END))"#,
        )?;
        Ok(())
    }

    /// Delete every card, checklist item, link and event (config is kept).
    pub fn wipe(&self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM checklist; DELETE FROM links; DELETE FROM events; DELETE FROM cards; DELETE FROM board_events;
             DELETE FROM sqlite_sequence WHERE name IN ('cards','events','board_events');",
        )?;
        Ok(())
    }

    /// Every card event is written here, so this is where it gets its identity (`actor_id`).
    /// EVERY change to a card writes an event through here, from the CLI, the full-screen
    /// board, an import, a move between boards and tb's own GitHub sync alike. So this is
    /// where `config actors` is enforced: a check at the command layer is a check with a door
    /// next to it (the board called `add` directly and walked straight past one), and the
    /// same argument that put read-only at the connection puts this at the event.
    ///
    /// The actor checked here is the one really doing the writing, not the ambient `--as`,
    /// which is what makes tb's own `github` sync exempt by ORIGIN rather than by whoever
    /// happened to type `tb sync`.
    fn log(conn: &Connection, id: i64, actor: &str, kind: &str, text: &str) -> Result<()> {
        access::guard_actor(conn, actor)?;
        let ts = now();
        let actor_id = actors::stamp(conn, actor, ts)?;
        conn.execute(
            "INSERT INTO events(card_id, ts, actor, kind, text, actor_id) VALUES (?,?,?,?,?,?)",
            params![id, ts, actor, kind, text, actor_id],
        )?;
        Ok(())
    }

    /// Like `log`, for the one event kind (`assigned`) whose holder must survive a reworded
    /// message: `assignee` carries the name in its OWN column, so `last_holder_of` (#111) never
    /// has to parse it back out of `text`. `text` stays the same human-readable "assigned to
    /// NAME" prose `tb show`/`tb log` already print — only what the self-approval guard reads
    /// changes.
    fn log_assign(conn: &Connection, id: i64, actor: &str, assignee: &str) -> Result<()> {
        access::guard_actor(conn, actor)?;
        let ts = now();
        let actor_id = actors::stamp(conn, actor, ts)?;
        conn.execute(
            "INSERT INTO events(card_id, ts, actor, kind, text, actor_id, assignee) VALUES (?,?,?,?,?,?,?)",
            params![id, ts, actor, "assigned", format!("assigned to {assignee}"), actor_id, assignee],
        )?;
        Ok(())
    }

    /// Every board-level event is written here (the `log` of `board_events`).
    pub(crate) fn log_board(conn: &Connection, actor: &str, kind: &str, text: &str) -> Result<()> {
        let ts = now();
        let actor_id = actors::stamp(conn, actor, ts)?;
        conn.execute(
            "INSERT INTO board_events(ts, actor, kind, text, actor_id) VALUES (?,?,?,?,?)",
            params![ts, actor, kind, text, actor_id],
        )?;
        Ok(())
    }

    pub fn wip(&self) -> Result<i64> {
        wip_of(&self.conn)
    }

    pub fn set_wip(&self, n: i64) -> Result<()> {
        if !(1..=MAX_WIP).contains(&n) {
            return err(format!("wip must be 1-{MAX_WIP} — try 'tb config wip 3'"), Code::InvalidValue);
        }
        self.set_config("wip", &n.to_string())
    }

    /// Set the WIP limit and record "wip A -> B" in the board-level log.
    pub fn change_wip(&self, n: i64, actor: &str) -> Result<()> {
        let old = self.wip()?;
        self.set_wip(n)?;
        if old != n {
            Self::log_board(&self.conn, actor, "wip", &format!("wip {old} -> {n}"))?;
        }
        Ok(())
    }

    /// Board-level log (not tied to a card), oldest first: (ts, actor, kind, text).
    pub fn board_events(&self) -> Result<Vec<(i64, String, String, String)>> {
        let mut st = self
            .conn
            .prepare("SELECT ts, actor, kind, text FROM board_events ORDER BY id")?;
        let v = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn theme(&self) -> Result<String> {
        theme_of(&self.conn)
    }

    pub fn set_theme(&self, theme: &str) -> Result<()> {
        let t = theme.trim().to_ascii_lowercase();
        if t != "dark" && t != "light" {
            return err(format!(
                "unknown theme '{theme}' — use 'tb config theme dark' or 'tb config theme light'"
            ), Code::InvalidValue);
        }
        self.set_config("theme", &t)
    }

    /// The board's GitHub repo (`owner/repo`), if the panel is on.
    pub fn github_repo(&self) -> Result<Option<String>> {
        let v: Option<String> = self
            .conn
            .query_row("SELECT value FROM config WHERE key='github'", [], |r| r.get(0))
            .optional()?;
        Ok(v.filter(|s| !s.is_empty()))
    }

    /// `Some("owner/repo")` turns the GitHub panel on; None turns it off and drops the cache.
    pub fn set_github(&self, repo: Option<&str>) -> Result<()> {
        match repo {
            Some(r) if crate::github::valid_repo(r.trim()) => {
                if self.github_repo()?.as_deref() != Some(r.trim()) {
                    self.conn.execute("DELETE FROM github_snapshot", [])?;
                }
                self.set_config("github", r.trim())?;
                // connecting a repo un-hides the panel
                self.set_config("github-panel", "shown")?
            }
            Some(r) => {
                return err(format!(
                    "'{r}' is not owner/repo — try 'tb config github acme/widgets'"
                ), Code::InvalidValue)
            }
            None => {
                self.conn.execute("DELETE FROM config WHERE key='github'", [])?;
                self.conn.execute("DELETE FROM github_snapshot", [])?;
            }
        }
        Ok(())
    }

    /// Store a fetch result: a snapshot replaces the cache and clears the error; an error is
    /// recorded next to the last good snapshot, and counted (red only after 3 in a row).
    pub fn save_github(&self, r: &std::result::Result<crate::github::GhSnapshot, String>) -> Result<()> {
        match r {
            Ok(s) => {
                let json = serde_json::to_string(s).unwrap_or_default();
                self.conn.execute(
                    "INSERT INTO github_snapshot(key, fetched_at, json, error, fails) VALUES (1, ?, ?, NULL, 0)
                     ON CONFLICT(key) DO UPDATE SET fetched_at=excluded.fetched_at, json=excluded.json, error=NULL, fails=0",
                    params![s.fetched_at, json],
                )?;
            }
            Err(e) => {
                self.conn.execute(
                    "INSERT INTO github_snapshot(key, fetched_at, json, error, fails) VALUES (1, 0, NULL, ?, 1)
                     ON CONFLICT(key) DO UPDATE SET error=excluded.error, fails=fails+1",
                    params![e],
                )?;
            }
        }
        Ok(())
    }

    /// Raw cached snapshot JSON, its error (if any) and consecutive fail count.
    pub fn github_cache(&self) -> Result<(Option<String>, Option<String>, i64)> {
        Ok(self
            .conn
            .query_row("SELECT json, error, fails FROM github_snapshot WHERE key=1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?
            .unwrap_or((None, None, 0)))
    }

    /// Repo + cached snapshot (only if it is for that repo) + last error + fail count.
    pub fn github_view(&self) -> Result<crate::github::GhView> {
        let repo = self.github_repo()?;
        let (json, error, fails) = self.github_cache()?;
        let snap = json
            .and_then(|j| serde_json::from_str::<crate::github::GhSnapshot>(&j).ok())
            .filter(|s| Some(&s.repo) == repo.as_ref());
        Ok(crate::github::GhView { repo, snap, error, fails })
    }

    /// `github-panel` / `agents-panel`: shown (default) or hidden.
    pub fn panel(&self, key: &str) -> Result<bool> {
        let v: Option<String> =
            self.conn.query_row("SELECT value FROM config WHERE key=?", [key], |r| r.get(0)).optional()?;
        Ok(v.as_deref() != Some("hidden"))
    }

    /// The panel's setting if one was ever made (`None` = never set, so the default applies).
    pub fn panel_choice(&self, key: &str) -> Result<Option<bool>> {
        let v: Option<String> =
            self.conn.query_row("SELECT value FROM config WHERE key=?", [key], |r| r.get(0)).optional()?;
        Ok(v.map(|v| v != "hidden"))
    }

    pub fn set_panel(&self, key: &str, value: &str) -> Result<()> {
        let v = value.trim().to_ascii_lowercase();
        let v = match v.as_str() {
            "shown" | "show" | "on" => "shown",
            "hidden" | "hide" | "off" => "hidden",
            _ => return err(format!("'{value}' is not shown|hidden — try 'tb config {key} hidden'"), Code::InvalidValue),
        };
        if key != "github-panel" && key != "agents-panel" {
            return err(format!("unknown panel '{key}' — use github-panel or agents-panel"), Code::InvalidValue);
        }
        self.set_config(key, v)
    }

    /// Every setting, for `tb config` with no arguments.
    pub fn settings(&self) -> Result<Vec<(String, String)>> {
        let mut all = vec![
            ("wip".into(), self.wip()?.to_string()),
            ("theme".into(), self.theme()?),
            ("layout".into(), self.layout()?),
            ("github".into(), self.github_repo()?.unwrap_or_else(|| "off".into())),
            ("github-panel".into(), if self.panel("github-panel")? { "shown" } else { "hidden" }.into()),
            ("agents-panel".into(), if self.panel("agents-panel")? { "shown" } else { "hidden" }.into()),
        ];
        // due dates (store/due.rs): listed once the board sets them, so a board that sets
        // nothing lists exactly what it always did
        all.extend(self.due_settings()?);
        all.extend(self.rm_settings()?);
        all.extend(self.sort_settings()?);
        all.extend(self.display_settings()?);
        all.extend(self.block_settings()?);
        all.extend(self.closing_settings()?);
        all.extend(self.verifier_settings()?);
        all.extend(self.link_settings()?);
        all.extend(self.rounds_settings()?);
        all.extend(self.kind_settings()?);
        all.extend(self.rules_settings()?);
        all.extend(self.access_settings()?);
        // hooks (store/gate.rs, crate::hooks): listed once the board asks for one, so a board
        // that asks for none lists exactly what it always did
        all.extend(self.hook_settings()?);
        // `file-mode` is listed only when there is something to say (a file other users can
        // open, or one kept shared on purpose): a private board's listing is unchanged
        all.extend(self.file_mode_setting()?.map(|v| ("file-mode".to_string(), v)));
        Ok(all)
    }

    /// (permission bits of the board file, kept shared on purpose?). The mode is None for an
    /// in-memory board and on a platform without unix permissions.
    pub fn file_mode(&self) -> Result<(Option<u32>, bool)> {
        let shared: Option<String> =
            self.conn.query_row("SELECT value FROM config WHERE key='file-mode'", [], |r| r.get(0)).optional()?;
        Ok((self.path().and_then(|p| crate::fsperm::mode_of(&p)), shared.as_deref() == Some("shared")))
    }

    /// The `file-mode` row of `tb config`: None while the file is private and nothing was set.
    fn file_mode_setting(&self) -> Result<Option<String>> {
        let (mode, shared) = self.file_mode()?;
        Ok(match (mode, shared) {
            (Some(m), true) => Some(format!("shared ({})", crate::fsperm::fmt_mode(m))),
            (None, true) => Some("shared".into()),
            (Some(m), false) if crate::fsperm::is_wide(m) => Some(format!("{} (open to other users)", crate::fsperm::fmt_mode(m))),
            _ => None,
        })
    }

    /// What `tb config file-mode` answers: `private (0600)`, `0644 (open to other users)`,
    /// `shared (0664)`, or `not applicable` where there is no file mode to speak of.
    pub fn file_mode_text(&self) -> Result<String> {
        Ok(match (self.file_mode_setting()?, self.file_mode()?.0) {
            (Some(s), _) => s,
            (None, Some(m)) => format!("private ({})", crate::fsperm::fmt_mode(m)),
            (None, None) => "not applicable".into(),
        })
    }

    /// `tb config file-mode private|shared`. `private` makes the board file and its live
    /// sidecars 0600 — the one place tb changes the mode of an existing file, because it was
    /// asked to, and it says what it did. `shared` records that other users are meant to reach
    /// this file, which ends the report. Both are logged on the board. Returns the line to print.
    pub fn set_file_mode(&self, value: &str, actor: &str) -> Result<String> {
        let log = |text: String| -> Result<()> { Self::log_board(&self.conn, actor, "file-mode", &text) };
        match value.trim().to_ascii_lowercase().as_str() {
            "private" => {
                let Some(path) = self.path() else {
                    return err("this board has no file yet — add a card first, e.g. 'tb add \"title\"'", Code::Unknown);
                };
                if crate::fsperm::mode_of(&path).is_none() {
                    return err("this platform has no file modes — there is nothing to tighten; see 'tb config'", Code::Unknown);
                }
                let done = crate::fsperm::make_private(&path).map_err(|e| {
                    BoardError(format!("cannot change the mode of {}: {e} — check that you own the file, then 'tb config file-mode private' again", path.display()), Code::IoError)
                })?;
                let base = |f: &std::path::PathBuf| f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                // a path that is not a regular file (a symbolic link someone planted) is never
                // re-moded: tb says so and leaves it
                let left = if done.skipped.is_empty() {
                    String::new()
                } else {
                    format!(" · left alone, not a regular file: {}", done.skipped.iter().map(base).collect::<Vec<_>>().join(", "))
                };
                self.conn.execute("DELETE FROM config WHERE key='file-mode'", [])?;
                if done.changed.is_empty() {
                    return Ok(format!("{} is already private (mode {}){left}", path.display(), crate::fsperm::fmt_mode(crate::fsperm::PRIVATE)));
                }
                let was: Vec<String> =
                    done.changed.iter().map(|(f, m)| format!("{} was {}", base(f), crate::fsperm::fmt_mode(*m))).collect();
                log(format!("private: {}", was.join(", ")))?;
                Ok(format!(
                    "{} is now private (mode {}): {}{left}",
                    path.display(),
                    crate::fsperm::fmt_mode(crate::fsperm::PRIVATE),
                    was.join(", ")
                ))
            }
            "shared" => {
                self.set_config("file-mode", "shared")?;
                let mode = self.file_mode()?.0.map(crate::fsperm::fmt_mode).unwrap_or_else(|| "unknown".into());
                log(format!("shared: mode {mode} kept"))?;
                Ok(format!("file-mode is now shared — tb leaves the mode ({mode}) alone and stops reporting it"))
            }
            other => err(format!("'{other}' is not private|shared — try 'tb config file-mode private'"), Code::InvalidValue),
        }
    }

    pub fn layout(&self) -> Result<String> {
        let v: Option<String> = self
            .conn
            .query_row("SELECT value FROM config WHERE key='layout'", [], |r| r.get(0))
            .optional()?;
        Ok(v.map(|l| layout_alias(&l).to_string()).filter(|l| LAYOUTS.contains(&l.as_str())).unwrap_or_else(|| "auto".into()))
    }

    pub fn set_layout(&self, layout: &str) -> Result<()> {
        let l = layout_alias(&layout.trim().to_ascii_lowercase()).to_string();
        if !LAYOUTS.contains(&l.as_str()) {
            return err(format!("unknown layout '{layout}' — use 'tb config layout auto|focus|third-h|third-v|half-h|half-v'"), Code::InvalidValue);
        }
        self.set_config("layout", &l)
    }

    /// Mark the board as set up (`tb setup` ran or was skipped).
    pub fn set_setup_done(&self) -> Result<()> {
        self.set_config(crate::setup::SETUP_DONE, "1")
    }

    /// Set up already, or configured by hand (any config key): bare `tb` skips the wizard.
    pub fn is_set_up(&self) -> Result<bool> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM config", [], |r| r.get(0))?;
        Ok(n > 0)
    }

    fn set_config(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO config(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn add(&self, raw_title: &str, desc: &str, checks: &[String], actor: &str) -> Result<i64> {
        self.add_tagged(raw_title, desc, checks, actor, None)
    }

    /// `add` with an explicit `--tag` (`store::closing`): the tag the user chose, and then
    /// the title is kept whole — tb guesses nothing from a `tag:` prefix it was not asked
    /// about, which is the point of the flag.
    pub fn add_tagged(&self, raw_title: &str, desc: &str, checks: &[String], actor: &str, tag: Option<Option<&str>>) -> Result<i64> {
        if raw_title.trim().is_empty() {
            return err("title is empty — try 'tb add \"tag: what to do\"'", Code::ArgRequired);
        }
        let (tag, gh, title) = match tag {
            None => parse_title(raw_title),
            Some(explicit) => {
                let (_, gh, title) = parse_title_keeping_prefix(raw_title);
                (explicit.map(str::to_string), gh, title)
            }
        };
        // `BEGIN IMMEDIATE`, not a deferred transaction (#85): this reads `bottom_of` and
        // then writes the INSERT. A deferred transaction takes its SHARED (read) lock on the
        // first statement and only asks to upgrade to a write lock on the INSERT — and SQLite
        // does not run the busy handler for that upgrade, so two concurrent adds return
        // SQLITE_BUSY ("database is locked") instantly instead of one of them waiting out the
        // 10s busy_timeout. Starting the transaction as a write from the first statement makes
        // the busy timeout apply, the way every other read-then-write path here now does.
        // `add` is `&self` (not `&mut self`), so `transaction_with_behavior` is not available
        // (it needs `&mut Connection`) — `unchecked_transaction` + an explicit ROLLBACK into a
        // fresh BEGIN IMMEDIATE is the same trick `note` and `block` already use below.
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("ROLLBACK; BEGIN IMMEDIATE")?;
        let t = now();
        let pos = bottom_of(&tx, "todo")?;
        tx.execute(
            r#"INSERT INTO cards(title, tag, description, "column", gh_ref, created_at, column_since, position)
               VALUES (?,?,?,'todo',?,?,?,?)"#,
            params![title, tag, desc.trim(), gh, t, t, pos],
        )?;
        let id = tx.last_insert_rowid();
        for (i, c) in checks.iter().enumerate() {
            tx.execute(
                "INSERT INTO checklist(card_id, idx, text, done) VALUES (?,?,?,0)",
                params![id, i as i64 + 1, c],
            )?;
        }
        Self::log(&tx, id, actor, "created", "")?;
        tx.commit()?;
        Ok(id)
    }

    /// Insert a card with explicit column/age (test fixtures).
    pub fn seed(&self, s: &Seed) -> Result<i64> {
        let id = self.add(
            s.title,
            s.desc,
            &s.checks.iter().map(|c| c.0.to_string()).collect::<Vec<_>>(),
            s.owner.unwrap_or("seed"),
        )?;
        let t = now();
        let since = t - s.age_secs;
        let pos = if s.column == "todo" { self.card(id)?.position } else { bottom_of(&self.conn, s.column)? };
        self.conn.execute(
            r#"UPDATE cards SET "column"=?, owner=?, due=?, created_at=?, column_since=?, position=? WHERE id=?"#,
            params![s.column, s.owner, s.due, since - 3600, since, pos, id],
        )?;
        self.conn
            .execute("UPDATE events SET ts=? WHERE card_id=?", params![since - 3600, id])?;
        for (i, c) in s.checks.iter().enumerate() {
            if c.1 {
                self.conn.execute(
                    "UPDATE checklist SET done=1 WHERE card_id=? AND idx=?",
                    params![id, i as i64 + 1],
                )?;
            }
        }
        if s.column != "todo" {
            let who = s.owner.unwrap_or("seed");
            let (kind, text) = match s.column {
                "doing" => ("taken", String::new()),
                col => ("moved", format!("todo -> {col}")),
            };
            self.conn.execute(
                "INSERT INTO events(card_id, ts, actor, kind, text) VALUES (?,?,?,?,?)",
                params![id, since, who, kind, text],
            )?;
        }
        let n = s.notes.len() as i64;
        for (i, (who, text)) in s.notes.iter().enumerate() {
            let ts = since + (s.age_secs * (i as i64 + 1)) / (n + 1);
            self.conn.execute(
                "INSERT INTO events(card_id, ts, actor, kind, text) VALUES (?,?,?,'note',?)",
                params![id, ts, who, text],
            )?;
        }
        Ok(id)
    }

    pub fn list(&self) -> Result<Vec<Card>> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {CARD_COLS} FROM cards ORDER BY id"))?;
        let v = st.query_map([], row_card)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn card(&self, id: i64) -> Result<Card> {
        get_card(&self.conn, id)
    }

    pub fn show(&self, id: i64) -> Result<CardDetail> {
        let card = self.card(id)?;
        let mut st = self
            .conn
            .prepare("SELECT idx, text, done FROM checklist WHERE card_id=? ORDER BY idx")?;
        let checklist = st
            .query_map([id], |r| {
                Ok(CheckItem { idx: r.get(0)?, text: r.get(1)?, done: r.get::<_, i64>(2)? != 0 })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut st = self.conn.prepare(
            "SELECT card_id, ts, actor, kind, text, actor_id FROM events WHERE card_id=? ORDER BY ts, id",
        )?;
        let events = st.query_map([id], row_event)?.collect::<rusqlite::Result<Vec<_>>>()?;
        let round = round_of(&events);
        let escalate = rounds::escalate_of(round, &card.column, self.max_rounds()?);
        let actors = self.actors_by_id(&events.iter().filter_map(|e| e.actor_id).collect::<Vec<_>>())?;
        let approved_by = closing::approved_by(&events);
        let links = self.links_of(id)?;
        Ok(CardDetail { card, checklist, events, round, escalate, approved_by, actors, links })
    }

    /// Every card's title, for an export that names a card without loading it again.
    pub fn card_titles(&self) -> Result<HashMap<i64, String>> {
        let mut st = self.conn.prepare("SELECT id, title FROM cards")?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<HashMap<i64, String>>>()?;
        Ok(v)
    }

    /// Every event at or after `from_ts`, oldest first, handed to `f` ONE AT A TIME. A board
    /// with a hundred thousand events must not be collected into a Vec before anything is
    /// written: this is what lets `tb export` and `tb log` stream.
    ///
    /// The filter is the TIME, not an id: `tb watch --since` may resume from an id because a
    /// live stream only ever moves forward, but a history can hold an event written with an
    /// earlier timestamp than the row before it (a clock that stepped, a fixture), and
    /// "everything since Tuesday" must still mean everything since Tuesday.
    pub fn for_each_event(&self, from_ts: i64, f: &mut dyn FnMut(&Event) -> Result<()>) -> Result<()> {
        let mut st = self
            .conn
            .prepare("SELECT card_id, ts, actor, kind, text, actor_id FROM events WHERE ts >= ? ORDER BY ts, id")?;
        let mut rows = st.query([from_ts])?;
        while let Some(r) = rows.next()? {
            f(&row_event(r)?)?;
        }
        Ok(())
    }

    /// Every event at or after `from_ts`, oldest first, card events interleaved with the
    /// board's own log (`board_events` — a move's `moved-out` on the board a card left, a WIP
    /// change, a file-mode change, a soft-delete, …), which otherwise has no command that
    /// reads it (#106: a card moved off a board leaves a trail on that board nothing prints).
    /// Two cursors, merged by timestamp, so this streams exactly like `for_each_event`.
    pub fn for_each_log_event(&self, from_ts: i64, f: &mut dyn FnMut(LogEvent) -> Result<()>) -> Result<()> {
        let mut cst = self
            .conn
            .prepare("SELECT card_id, ts, actor, kind, text, actor_id FROM events WHERE ts >= ? ORDER BY ts, id")?;
        let mut crows = cst.query([from_ts])?;
        let mut bst = self.conn.prepare("SELECT ts, actor, kind, text, actor_id FROM board_events WHERE ts >= ? ORDER BY ts, id")?;
        let mut brows = bst.query([from_ts])?;

        /// One `board_events` row, held between cursor advances (a named struct, not a
        /// 5-tuple, so the type stays readable).
        struct BoardRow {
            ts: i64,
            actor: String,
            kind: String,
            text: String,
            actor_id: Option<i64>,
        }

        fn next_card(rows: &mut rusqlite::Rows<'_>) -> Result<Option<Event>> {
            Ok(match rows.next()? {
                Some(r) => Some(row_event(r)?),
                None => None,
            })
        }
        fn next_board(rows: &mut rusqlite::Rows<'_>) -> Result<Option<BoardRow>> {
            Ok(match rows.next()? {
                Some(r) => Some(BoardRow { ts: r.get(0)?, actor: r.get(1)?, kind: r.get(2)?, text: r.get(3)?, actor_id: r.get(4)? }),
                None => None,
            })
        }

        let mut c_cur = next_card(&mut crows)?;
        let mut b_cur = next_board(&mut brows)?;
        loop {
            let card_first = match (&c_cur, &b_cur) {
                (None, None) => break,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (Some(c), Some(b)) => c.ts <= b.ts,
            };
            if card_first {
                f(LogEvent::Card(c_cur.take().unwrap()))?;
                c_cur = next_card(&mut crows)?;
            } else {
                let BoardRow { ts, actor, kind, text, actor_id } = b_cur.take().unwrap();
                f(LogEvent::Board { ts, actor, kind, text, actor_id })?;
                b_cur = next_board(&mut brows)?;
            }
        }
        Ok(())
    }

    /// Every event of one card, oldest first — the whole history an export carries, where
    /// `contract::card` carries only the last ten.
    pub fn all_events_of(&self, id: i64) -> Result<Vec<crate::contract::EventJ>> {
        let mut st = self
            .conn
            .prepare("SELECT card_id, ts, actor, kind, text, actor_id FROM events WHERE card_id=? ORDER BY ts, id")?;
        let v = st
            .query_map([id], row_event)?
            .map(|e| e.map(crate::contract::EventJ::of))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// Events with `id > after`, oldest first (for `tb watch --events`).
    pub fn events_since(&self, after: i64) -> Result<Vec<WatchEvent>> {
        let mut st = self
            .conn
            .prepare("SELECT id, card_id, ts, actor, kind, text, actor_id FROM events WHERE id > ? ORDER BY id")?;
        let v = st
            .query_map([after], |r| {
                Ok(WatchEvent {
                    id: r.get(0)?,
                    event: Event {
                        card_id: r.get(1)?,
                        ts: r.get(2)?,
                        actor: r.get(3)?,
                        kind: r.get(4)?,
                        text: r.get(5)?,
                        actor_id: r.get(6)?,
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// The id of the last event strictly before `ts` (0 when none) — `--since` resumption:
    /// streaming `id > cursor` yields exactly the events at/after `ts`.
    pub fn events_cursor_at(&self, ts: i64) -> Result<i64> {
        // a DB error must surface, not silently replay the whole history from 0
        Ok(self.conn.query_row("SELECT COALESCE(MAX(id), 0) FROM events WHERE ts < ?", [ts], |r| r.get(0))?)
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let cards = self.list()?;
        let wip = self.wip()?;
        let mut last_note = HashMap::new();
        let mut last_event_at: HashMap<i64, i64> = HashMap::new();
        let mut recent: HashMap<i64, Vec<Event>> = HashMap::new();
        let mut rounds: HashMap<i64, i64> = HashMap::new();
        let mut actor_last: HashMap<String, Event> = HashMap::new();
        let mut st = self.conn.prepare(
            "SELECT card_id, ts, actor, kind, text, actor_id FROM events ORDER BY card_id, ts, id",
        )?;
        for e in st.query_map([], row_event)? {
            let e = e?;
            if e.kind == "note" {
                last_note.insert(e.card_id, e.text.clone());
            }
            last_event_at.insert(e.card_id, e.ts);
            if e.kind == "returned" {
                *rounds.entry(e.card_id).or_insert(1) += 1;
            }
            let who = e.actor.trim().to_lowercase();
            if !actor_last.get(&who).is_some_and(|p: &Event| p.ts > e.ts) {
                actor_last.insert(who, e.clone());
            }
            let v = recent.entry(e.card_id).or_default();
            v.push(e);
            if v.len() > 2 {
                v.remove(0);
            }
        }
        let mut checks = HashMap::new();
        let mut st = self
            .conn
            .prepare("SELECT card_id, SUM(done), COUNT(*) FROM checklist GROUP BY card_id")?;
        for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))? {
            let (id, d, t) = r?;
            checks.insert(id, (d, t));
        }
        let theme = self.theme()?;
        let layout = self.layout()?;
        let github_panel_hidden = !self.panel("github-panel")?;
        let agents_panel_hidden = !self.panel("agents-panel")?;
        Ok(Snapshot {
            cards,
            wip,
            last_note,
            last_event_at,
            recent,
            checks,
            rounds,
            theme,
            board: self.name.clone(),
            layout,
            github_panel_hidden,
            agents_panel_hidden,
            now: now(),
            actor_last,
            sort: self.sort()?,
            display: self.display()?,
            blocks: self.block_ctx()?,
            waiting_lane: self.waiting_lane()?,
        })
    }

    /// Atomically take the oldest todo card.
    pub fn next(&mut self, actor: &str) -> Result<Card> {
        self.next_bg(actor, None)
    }

    /// `next` with `--break-glass "why"`: see `move_opts_bg`.
    pub fn next_bg(&mut self, actor: &str, break_glass: Option<&str>) -> Result<Card> {
        self.claim(None, actor, break_glass)
    }

    /// Atomically claim the top unclaimed, unblocked REVIEW card that `actor` did not author.
    /// Same `BEGIN IMMEDIATE` lock and compare-and-swap as `next`, so two reviewers never
    /// get the same card. Does not count against the WIP limit.
    pub fn next_review(&mut self, actor: &str) -> Result<Card> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cards: Vec<Card> = {
            let mut st = tx.prepare(&format!(
                r#"SELECT {CARD_COLS} FROM cards WHERE "column"='review' AND blocked IS NULL AND reviewer IS NULL"#
            ))?;
            let mut v = st.query_map([], row_card)?.collect::<rusqlite::Result<Vec<_>>>()?;
            // the one ordering (store/order.rs): the REVIEW column exactly as everyone sees it
            let sort = order::sort_of(&tx)?;
            v.sort_by(|a, b| order::cmp(sort, a, b));
            v
        };
        // an escalated card (`config max-rounds`, store/rounds.rs) is skipped by the automatic
        // claim here too, same reasoning as `tb next`'s TODO pick: it is not counted as "yours"
        // or as "waiting", just passed over, so it never inflates either message below.
        let max_rounds = rounds::max_rounds_of(&tx)?;
        let mut own = 0;
        let mut target = None;
        for c in &cards {
            if rounds::is_escalated(&tx, c.id, &c.column, max_rounds)? {
                continue;
            }
            if author_of(&tx, c)?.is_some_and(|a| a.eq_ignore_ascii_case(actor)) {
                own += 1;
            } else {
                target = Some(c.id);
                break;
            }
        }
        let Some(target) = target else {
            return err(if own > 0 {
                let waiting = if own == 1 { "the one waiting is".to_string() } else { format!("the {own} waiting are") };
                format!("no review cards for you — {waiting} your own work; ask another person or agent to review it, and take new work with 'tb next'")
            } else {
                "no review cards waiting — take new work with 'tb next'".to_string()
            }, Code::Unknown);
        };
        let changed = tx.execute(
            r#"UPDATE cards SET reviewer=? WHERE id=? AND "column"='review' AND reviewer IS NULL"#,
            params![actor, target],
        )?;
        if changed != 1 {
            return err(format!("card #{target} was claimed by someone else — try 'tb next --review'"), Code::Unknown);
        }
        Self::log(&tx, target, actor, "reviewing", "")?;
        let card = get_card(&tx, target)?;
        tx.commit()?;
        Ok(card)
    }

    /// Atomically take a specific todo card.
    pub fn take(&mut self, id: i64, actor: &str) -> Result<Card> {
        self.take_bg(id, actor, None)
    }

    /// `take` with `--break-glass "why"`: see `move_opts_bg`.
    pub fn take_bg(&mut self, id: i64, actor: &str, break_glass: Option<&str>) -> Result<Card> {
        self.claim(Some(id), actor, break_glass)
    }

    /// `tb assign ID NAME`: hand a specific TODO card straight to `owner`, without `actor`
    /// (the one running the command) becoming its holder — `take` done on someone else's
    /// behalf. Only a TODO card is a valid target (the same restriction `take` itself
    /// enforces, and `take` has no `--force` to override it either), so this can never pull a
    /// card away from whoever already holds it. The event log keeps the two facts separate:
    /// its `actor` is who assigned the card, its new `owner` is who now holds it.
    pub fn assign(&mut self, id: i64, owner: &str, actor: &str) -> Result<Card> {
        let owner = owner.trim();
        if owner.is_empty() {
            return err(format!("name is empty — try 'tb assign {id} bob'"), Code::ArgRequired);
        }
        self.transition(Change::Assign { id, owner }, actor, false, HookGate::Normal)
    }

    /// `BEGIN IMMEDIATE` + compare-and-swap on the column, so two callers can never
    /// both win the same card.
    fn claim(&mut self, id: Option<i64>, actor: &str, break_glass: Option<&str>) -> Result<Card> {
        self.transition(Change::Claim(id), actor, false, HookGate::of(break_glass))
    }

    /// The card `next` / `take` claims: the named one (it must be in TODO), or the top
    /// unblocked TODO card. Runs inside `transition`'s transaction. (The body is the selection
    /// exactly as it was written inside `claim`, `&tx` and all, so work on the ordering that is
    /// in flight elsewhere still merges line for line — hence the lint allowance.)
    #[allow(clippy::needless_borrow)]
    fn claim_target(tx: &Connection, id: Option<i64>) -> Result<i64> {
        let target = match id {
            Some(id) => {
                let c = get_card(&tx, id)?;
                if c.column != "todo" {
                    let who = c.owner.map(|o| format!(", owner {o}")).unwrap_or_default();
                    return err(format!(
                        "card #{id} is in {}{who}, not todo — take another with 'tb next'",
                        c.column
                    ), Code::Unknown);
                }
                id
            }
            None => {
                // the first unblocked card of the TODO column in the one ordering
                // (store/order.rs), read inside this transaction: what `tb next` hands out is
                // what `tb list` and the board show on top — nearest due date under `sort due`.
                // The queue is read WHOLE, blocks included: a row the order cannot read must
                // refuse the claim, never be silently skipped for a card that parses (an
                // agent runs `tb next` blind — what it hands out must be what the board shows
                // on top, or nothing).
                // an escalated card (`config max-rounds`, store/rounds.rs) is skipped here too:
                // this is the AUTOMATIC pick, and handing out a card that is already stuck in a
                // worker/reviewer loop would extend the loop unnoticed. `tb take ID` names a
                // card explicitly and is not filtered — an escalated card is skipped, not hidden.
                let max_rounds = rounds::max_rounds_of(tx)?;
                let found: Option<i64> = {
                    let mut st = tx.prepare(&format!(
                        r#"SELECT {CARD_COLS} FROM cards WHERE "column"='todo'"#
                    ))?;
                    let open = st.query_map([], row_card)?.collect::<rusqlite::Result<Vec<Card>>>()?;
                    let sort = order::sort_of(&tx)?;
                    let mut candidates = Vec::with_capacity(open.len());
                    for c in open {
                        if c.blocked.is_none() && !rounds::is_escalated(tx, c.id, &c.column, max_rounds)? {
                            candidates.push(c);
                        }
                    }
                    candidates.into_iter().min_by(|a, b| order::cmp(sort, a, b)).map(|c| c.id)
                };
                match found {
                    Some(i) => i,
                    None => {
                        return err("no todo cards — add one with 'tb add \"title\"'", Code::Unknown)
                    }
                }
            }
        };
        Ok(target)
    }

    /// Log an event of any kind on a card (e.g. `github` auto-moves).
    pub fn note_kind(&self, id: i64, actor: &str, text: &str, kind: &str) -> Result<()> {
        Self::log(&self.conn, id, actor, kind, text)
    }

    pub fn note(&self, id: i64, text: &str, actor: &str) -> Result<()> {
        if text.trim().is_empty() {
            return err(format!("note is empty — try 'tb note {id} \"what changed\"'"), Code::ArgRequired);
        }
        // The card is checked INSIDE the write transaction, not before it. Checking first
        // and writing after leaves a window: under WAL the check reads happily while another
        // process holds the write lock, and by the time the insert runs the card can be gone
        // — which surfaced as a raw `FOREIGN KEY constraint failed` instead of `no card #N`
        // (a `tb note` racing a `tb mv` of the same card). Same shape as `block` and `edit`.
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("ROLLBACK; BEGIN IMMEDIATE")?;
        get_card(&tx, id)?;
        Self::log(&tx, id, actor, "note", text.trim())?;
        tx.commit()?;
        Ok(())
    }

    /// Toggle checklist item `n` (1-based). Returns new state.
    pub fn check(&self, id: i64, n: i64, actor: &str) -> Result<bool> {
        let d = self.show(id)?;
        let Some(item) = d.checklist.iter().find(|c| c.idx == n) else {
            return err(format!(
                "card #{id} has no checklist item {n} (it has {}) — see 'tb show {id}'",
                d.checklist.len()
            ), Code::Unknown);
        };
        let new = !item.done;
        self.conn.execute(
            "UPDATE checklist SET done=? WHERE card_id=? AND idx=?",
            params![new as i64, id, n],
        )?;
        let mark = if new { "[x]" } else { "[ ]" };
        Self::log(&self.conn, id, actor, "check", &format!("{mark} {}", item.text))?;
        Ok(new)
    }

    /// Mark a card blocked (`reason` like `#7`), or clear it with None.
    pub fn block(&self, id: i64, reason: Option<&str>, actor: &str) -> Result<()> {
        let reason = reason.map(|r| r.trim().trim_start_matches("by ").trim().to_string());
        if reason.as_deref() == Some("") {
            return err(format!("say what blocks it — 'tb block {id} \"#7\"'"), Code::ArgRequired);
        }
        // the card is checked inside the transaction, for the reason given on `note`
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("ROLLBACK; BEGIN IMMEDIATE")?;
        get_card(&tx, id)?;
        tx.execute("UPDATE cards SET blocked=? WHERE id=?", params![reason, id])?;
        match &reason {
            Some(r) => Self::log(&tx, id, actor, "blocked", &format!("by {r}"))?,
            None => Self::log(&tx, id, actor, "unblocked", "")?,
        }
        tx.commit()?;
        Ok(())
    }

    /// Append a checklist item. Returns its number.
    pub fn add_check(&self, id: i64, text: &str, actor: &str) -> Result<i64> {
        self.card(id)?;
        let text = text.trim();
        if text.is_empty() {
            return err(format!("check item is empty — try 'tb check {id} --add \"write test\"'"), Code::ArgRequired);
        }
        // reads (the next idx) then writes: needs `BEGIN IMMEDIATE`, same reason as `add` (#85).
        // `&self`, so `unchecked_transaction` + the ROLLBACK/BEGIN IMMEDIATE trick, not
        // `transaction_with_behavior` (which needs `&mut Connection`) — see `add_tagged`.
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("ROLLBACK; BEGIN IMMEDIATE")?;
        let n: i64 = tx.query_row(
            "SELECT COALESCE(MAX(idx), 0) + 1 FROM checklist WHERE card_id=?",
            [id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO checklist(card_id, idx, text, done) VALUES (?,?,?,0)",
            params![id, n, text],
        )?;
        Self::log(&tx, id, actor, "check", &format!("+ {text}"))?;
        tx.commit()?;
        Ok(n)
    }

    /// Delete checklist item `n` and renumber the ones after it.
    pub fn remove_check(&self, id: i64, n: i64, actor: &str) -> Result<()> {
        let d = self.show(id)?;
        let Some(item) = d.checklist.iter().find(|c| c.idx == n) else {
            return err(format!(
                "card #{id} has no checklist item {n} (it has {}) — see 'tb show {id}'",
                d.checklist.len()
            ), Code::Unknown);
        };
        // a write transaction from the start, consistent with every other write path (#85)
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("ROLLBACK; BEGIN IMMEDIATE")?;
        tx.execute("DELETE FROM checklist WHERE card_id=? AND idx=?", params![id, n])?;
        // two steps so the (card_id, idx) key never collides mid-update
        tx.execute(
            "UPDATE checklist SET idx = -(idx - 1) WHERE card_id=? AND idx>?",
            params![id, n],
        )?;
        tx.execute("UPDATE checklist SET idx = -idx WHERE card_id=? AND idx<0", params![id])?;
        Self::log(&tx, id, actor, "check", &format!("- {}", item.text))?;
        tx.commit()?;
        Ok(())
    }

    /// The guards `actor` moving card `id` into DONE would fail right now — (rule, code), in the
    /// order `--force` would log them; empty when nothing stands in the way or the card is
    /// already done. What the full-screen board names before it offers to force a close.
    pub fn done_would_skip(&self, id: i64, actor: &str) -> Result<Vec<(&'static str, Code)>> {
        let c = get_card(&self.conn, id)?;
        if c.column == "done" {
            return Ok(Vec::new());
        }
        Ok(done_checks(&self.conn, &c, actor)?.into_iter().map(|d| (d.rule, d.err.1)).collect())
    }

    pub fn move_to(&mut self, id: i64, column: &str, actor: &str) -> Result<Card> {
        self.move_card(id, column, actor, false, None, None)
    }

    /// `move_to` that lets the author approve their own REVIEW card; logged as a `force` event.
    pub fn move_to_forced(&mut self, id: i64, column: &str, actor: &str) -> Result<Card> {
        self.move_card(id, column, actor, true, None, None)
    }

    /// `move_to`, exempt from the pre/post-change hook by an internal ORIGIN — never by the
    /// actor name (an actor asking `--as github` is refused before this, in `main.rs`). tb's
    /// own GitHub sync is the only caller: it writes down what a merged PR or a closed issue
    /// already says, not a person or agent proposing a change, so it is not who a hook exists
    /// to gate.
    pub fn move_sync(&mut self, id: i64, column: &str, actor: &str) -> Result<Card> {
        self.transition(Change::Move { id, column, reason: None }, actor, false, HookGate::Sync)
    }

    /// `move_to` with every option: `reason` is required (and only allowed) when a REVIEW
    /// card goes back to DOING.
    pub fn move_opts(&mut self, id: i64, column: &str, actor: &str, force: bool, reason: Option<&str>) -> Result<Card> {
        self.move_opts_bg(id, column, actor, force, reason, None)
    }

    /// `move_opts` with `--break-glass "why"`: skip the pre-change hook this board asks for,
    /// applying the change anyway — logged on the card and on the board (`store::gate`), never
    /// silently. `None`: behave exactly as `move_opts` (a hook, if any, decides as always).
    pub fn move_opts_bg(
        &mut self,
        id: i64,
        column: &str,
        actor: &str,
        force: bool,
        reason: Option<&str>,
        break_glass: Option<&str>,
    ) -> Result<Card> {
        self.move_card(id, column, actor, force, reason, break_glass)
    }

    /// Send a REVIEW card back to its owner in DOING with the reason (a `returned` event).
    pub fn send_back(&mut self, id: i64, reason: &str, actor: &str) -> Result<Card> {
        self.move_card(id, "doing", actor, false, Some(reason), None)
    }

    /// Last `returned` event time per card (GitHub sync leaves those cards alone until
    /// their PR is updated after it).
    pub fn returned_at(&self) -> Result<HashMap<i64, i64>> {
        let mut st = self.conn.prepare("SELECT card_id, MAX(ts) FROM events WHERE kind='returned' GROUP BY card_id")?;
        let v = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<HashMap<i64, i64>>>()?;
        Ok(v)
    }

    /// Who did the work on a card: its owner, or — only for a card that reached review with
    /// no owner — the actor of its last move into review (never the `github` sync).
    pub fn author(&self, id: i64) -> Result<Option<String>> {
        let c = get_card(&self.conn, id)?;
        author_of(&self.conn, &c)
    }

    fn move_card(&mut self, id: i64, column: &str, actor: &str, force: bool, reason: Option<&str>, break_glass: Option<&str>) -> Result<Card> {
        self.transition(Change::Move { id, column, reason }, actor, force, HookGate::of(break_glass))
    }

    /// EVERY column change goes through here — `next`/`take`, `move`/`done`/send-back (and the
    /// GitHub sync, which calls `move_sync`), and `drop` — from the CLI and the full-screen
    /// board alike, via [`transition`](Self::transition), which asks the board's pre/post-change
    /// hook (if any) around this. One transaction (`BEGIN IMMEDIATE`), and one fixed order:
    ///
    /// 1. what is asked: the card, the column it goes to, and the refusals that belong to the
    ///    request itself (unknown column, a send-back without its reason, nothing to take); a
    ///    change that changes nothing ends here. When `transition` asked a pre-change hook, its
    ///    answer was about a SPECIFIC card in a SPECIFIC state — `snapshot`, carried straight
    ///    through — so the freshly-read card here must still match it (id, column, owner) or
    ///    the change is refused as "changed while the hook ran": an approval about a card that
    ///    has since moved is never acted on.
    /// 2. the guards, always in this order: the holder (leaving DOING needs the card's owner,
    ///    or `--force`, logged) → review-first (entering DONE from anywhere but REVIEW, or
    ///    `--force`, logged — store/verifier.rs) → self-approval (entering DONE by the card's
    ///    author or last holder, or `--force`, logged) → the
    ///    verifier rule (REVIEW -> DONE needs a verifier role, a place on `config verifiers`,
    ///    or a person; or `--force`, logged — store/verifier.rs) → the same-session rule
    ///    (REVIEW -> DONE from a session that did the work, or `--force`, logged —
    ///    store/verifier.rs) → `done-by` (entering DONE needs to be one of the named closers,
    ///    or `--force`, logged) → `done-needs-note` (entering DONE needs a note written during
    ///    the stay being left, or `--force`, logged) → the WIP limit (entering DOING, except a
    ///    send-back). The first two are about WHO may touch the card; `done-by` and
    ///    `done-needs-note` are about closing it responsibly, so they come after — a person
    ///    blocked by ownership or self-approval never even reaches the closing checks. The
    ///    pre-change hook itself is NOT one of these guards — it already ran, in `transition`,
    ///    BEFORE any of them (see that function's doc comment for why) — the one thing this
    ///    function does on its behalf is the `snapshot` mismatch check in step 1: the hook
    ///    approved a specific (id, column, owner), and this function is what makes an approval
    ///    about a card that has since changed count for nothing.
    /// 3. the change, then its events.
    ///
    /// A new persona-independent guard (not a hook) belongs in step 2, after the ones there.
    fn transition_inner(&mut self, change: Change<'_>, actor: &str, force: bool, snapshot: Option<(i64, &str, Option<&str>)>) -> Result<Card> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // A pre-change hook (if `transition` asked one) approved a change to exactly this card
        // in exactly this state. Checked FIRST, before step 1's own refusals: a card someone
        // else took while the hook ran must read as "changed while the hook ran", not as
        // whatever step 1 would say about the card it is now.
        if let Some((exp_id, exp_col, exp_owner)) = snapshot {
            let same = get_card(&tx, exp_id).is_ok_and(|now| now.column == exp_col && now.owner.as_deref() == exp_owner);
            if !same {
                return Err(gate::race_err(exp_id));
            }
        }
        // 1. what is asked
        let (c, column, reason) = match change {
            Change::Claim(id) => {
                let target = Self::claim_target(&tx, id)?;
                (get_card(&tx, target)?, "doing".to_string(), None)
            }
            Change::Assign { id, .. } => {
                // the same "must be TODO" check `take ID` makes (claim_target's Some(id) arm);
                // reusing it keeps the refusal text identical, so an agent that has seen
                // take's error recognizes assign's
                let target = Self::claim_target(&tx, Some(id))?;
                (get_card(&tx, target)?, "doing".to_string(), None)
            }
            Change::Drop(id) => {
                let c = get_card(&tx, id)?;
                if c.column == "todo" && c.owner.is_none() {
                    return Ok(c);
                }
                (c, "todo".to_string(), None)
            }
            Change::Release { id, holder, .. } => {
                let c = get_card(&tx, id)?;
                if c.column != "doing" || !c.owner.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(holder)) {
                    return err(format!(
                        "#{id} changed while its holder {holder} was being checked — look again: 'tb show {id}'"
                    ), Code::Unknown);
                }
                (c, "todo".to_string(), None)
            }
            Change::Move { id, column, reason } => {
                let column = column.to_ascii_lowercase();
                if !COLUMNS.contains(&column.as_str()) {
                    return err(format!(
                        "unknown column '{column}' — use one of todo, doing, review, done: 'tb move {id} doing'"
                    ), Code::InvalidValue);
                }
                let reason = reason.map(str::trim);
                let c = get_card(&tx, id)?;
                let send_back = c.column == "review" && column == "doing";
                // a FAILed card goes back to TODO unowned, with the same `returned` event
                // the review->doing send-back writes: the FAIL reason travels on the card.
                let fail_to_todo = c.column == "review" && column == "todo";
                if send_back && !matches!(reason, Some(r) if !r.is_empty()) {
                    return err(format!(
                        "say why it goes back — 'tb move {id} doing \"what to fix\"'"
                    ), Code::ReasonRequired);
                }
                if !send_back && !fail_to_todo && reason.is_some() {
                    return err(format!(
                        "a reason only goes with sending a REVIEW card back to doing or todo — log it with 'tb note {id} \"...\"'"
                    ), Code::InvalidValue);
                }
                if c.column == column {
                    // `tb move ID review` on a claimed card releases the claim (a reviewer that stopped)
                    if column == "review" && c.reviewer.is_some() {
                        tx.execute("UPDATE cards SET reviewer=NULL WHERE id=?", [id])?;
                        Self::log(&tx, id, actor, "unclaimed", c.reviewer.as_deref().unwrap_or(""))?;
                        let c = get_card(&tx, id)?;
                        tx.commit()?;
                        return Ok(c);
                    }
                    return Ok(c);
                }
                (c, column, reason)
            }
        };
        // A pre-change hook (if `transition` asked one) approved a change to exactly this card
        // in exactly this state — never a blank check. `probe` (outside the transaction) and
        // the resolution above (inside it) can pick a DIFFERENT card for a bare `tb next` /
        // `assign` if the board changed in between (a different TODO card now sorts first, or
        // this one was just taken) — comparing the id catches that, not only the column/owner.
        if let Some((exp_id, exp_col, exp_owner)) = snapshot {
            if c.id != exp_id || c.column != exp_col || c.owner.as_deref() != exp_owner {
                return Err(gate::race_err(exp_id));
            }
        }
        let id = c.id;
        let kind = match change {
            Change::Claim(_) => Kind::Claim,
            Change::Drop(_) => Kind::Drop,
            Change::Move { .. } => Kind::Move,
            Change::Assign { .. } => Kind::Assign,
            Change::Release { .. } => Kind::Release,
        };
        // who `assign` hands the card to — read out of `change` here (not inside the "3. the
        // change" match below) because `column`/`c` are rebound by then; a WIP cap keyed on
        // the HOLDER (not the actor issuing the command) must see this name too — see the
        // note beside the WIP check below.
        let assignee = match change {
            Change::Assign { owner, .. } => Some(owner),
            _ => None,
        };
        let send_back = kind == Kind::Move && c.column == "review" && column == "doing";
        let fail_to_todo = kind == Kind::Move && c.column == "review" && column == "todo";
        // 2. the guards
        // Card ids are small shared integers: an off-by-one must not move someone else's
        // work. Leaving DOING requires the owner (or --force, logged as its own event).
        // The `github` automation is exempt: its moves are evidence-driven and logged.
        // `release` is exempt too: its permission is the dead-holder check (store/release.rs).
        if c.column == "doing" && actor != "github" && kind != Kind::Release {
            if let Some(owner) = c.owner.as_deref() {
                if !owner.eq_ignore_ascii_case(actor) {
                    if !force {
                        return Err(ownership_err(&tx, id, owner, actor, &format!("move it to {column}"))?);
                    }
                    let text = match kind {
                        Kind::Drop => format!("moved #{id} held by {owner} back to todo"),
                        _ => format!("moved #{id} held by {owner} to {column}"),
                    };
                    Self::log(&tx, id, actor, "force", &text)?;
                }
            }
        }
        // Every way into DONE is guarded, not just REVIEW -> DONE (#55, the LAUNDERING hole):
        // the owner of a REVIEW card could move it back to TODO first — which clears the
        // owner — and then close it with a plain `tb done`, which used to see `c.column ==
        // "todo"` and skip this check entirely. `column == "done" && c.column != "done"` is
        // the same shape `done-by` below already uses, for the same reason its comment gives:
        // moving the card out of REVIEW first must not be a way round the guard.
        //
        // A second question closes the DROPPED-WORK hole (#55): `author_of` only sees the
        // CURRENT owner, or — once unowned — whoever last moved the card into review. A drop
        // clears the owner, so an agent that held the card in DOING, dropped it, and then let
        // someone else move the now-unowned card into review stops being "the author" by that
        // reading, even though it did the work. `last_holder_of` answers "who most recently
        // HELD this card, by either `tb next`/`tb take` or `tb assign`" instead — a drop logs
        // a `dropped` event, not a `taken`/`assigned` one, so it does not erase this — and a
        // fresh claim or assignment to a DIFFERENT actor since (a genuinely new holder) still
        // supersedes it, so a real reassignment is never falsely refused.
        // Every guard on the way into DONE, in one fixed order, from ONE list (`done_checks`):
        // review-first → self-approval → the verifier rule → same-session → `done-by` →
        // `done-needs-note` →
        // `done-needs-link`. Each refuses unless `--force`, which logs one `force` event per
        // guard it gets past. The full-screen board asks the same list (`done_would_skip`)
        // before it offers to force a close, so its prompt can never skip a rule it did not name.
        if column == "done" && c.column != "done" {
            for check in done_checks(&tx, &c, actor)? {
                if !force {
                    return Err(check.err);
                }
                Self::log(&tx, id, actor, "force", &check.forced)?;
            }
        }
        // a returned card is its owner's existing work, not new work: WIP does not block it
        if column == "doing" && !send_back {
            let wip = wip_of(&tx)?;
            // `wip-counts-blocked no` (store/blocks.rs) discounts blocked DOING cards, up to
            // `wip` of them, so waiting for someone else does not stall the board — and
            // blocking everything can still never hand out unlimited work.
            //
            // The per-owner cap (`wip-per-owner`, store/access.rs) lives here too: `tb assign`
            // enters DOING through this exact check, on purpose, so the cap catches it for
            // free — PROVIDED it is keyed on the card's new HOLDER, never on `actor` alone.
            // `actor` is who is issuing the command (the assigner); for `Kind::Assign` that is
            // not who ends up holding the card, and a cap keyed on the wrong name would let an
            // orchestrator assign straight past it — exactly the hole a per-owner limit exists
            // to close. Hence `assignee.unwrap_or(actor)`: `assignee` is `Some` only for
            // `Kind::Assign`, and for `Kind::Claim` it is `None`, so this is `actor` there.
            // NOT `c.owner`, which is still `None` for an assign (the card is still TODO).
            //
            // Board-wide first, then per-owner: the board-wide message names every holder, so
            // a board that is simply full says so once, rather than telling one agent it is
            // personally over a cap that would not have mattered. Each gate discounts blocked
            // cards by its OWN number (store/access.rs), so they compose as independent limits.
            let (counted, doing) = blocks::doing_counts(&tx, wip)?;
            if counted >= wip {
                return Err(wip_full_err(&tx, doing, wip, actor));
            }
            access::room_for(&tx, assignee.unwrap_or(actor))?;
        }

        // 3. the change, then its events
        let owner = match (kind, column.as_str()) {
            // MUST come before the `Kind::Claim` arm below: assign sets the NAMED owner, not
            // the actor running the command — that distinction (who assigned it vs. who now
            // holds it) is the whole point of the command, and the event log carries both:
            // `owner` here is who holds it, `actor` on the "assigned" event is who assigned it.
            (Kind::Assign, _) => Some(assignee.expect("Kind::Assign always carries an owner").to_string()),
            (Kind::Claim, _) => Some(actor.to_string()),
            (_, "todo") => None,
            (_, "doing") => Some(c.owner.clone().unwrap_or_else(|| actor.to_string())),
            _ => c.owner.clone(),
        };
        // a block set while in REVIEW must not survive into DONE (or it renders as a live
        // problem on a finished card); reaching done clears it, logged as part of the move.
        // Any other move keeps the block untouched (QA: 'block 1' then 'done 1' from DOING
        // must NOT clear it — only the DONE transition does).
        let block_cleared = column == "done" && c.blocked.is_some();
        let pos = bottom_of(&tx, &column)?;
        // the reviewer stays on the card that reaches done (who approved it); any other move
        // ends the review, so the next round is claimed afresh
        let reviewer = if column == "done" { c.reviewer.clone() } else { None };
        // compare-and-swap on the column the card was read in: two callers never both win it
        let changed = tx.execute(
            r#"UPDATE cards SET "column"=?, owner=?, column_since=?, position=?, reviewer=?,
               blocked = CASE WHEN ?='done' THEN NULL ELSE blocked END WHERE id=? AND "column"=?"#,
            params![column, owner, now(), pos, reviewer, column, id, c.column],
        )?;
        if changed != 1 {
            return err(format!("card #{id} was taken by someone else — try 'tb next'"), Code::Unknown);
        }
        match kind {
            Kind::Claim => Self::log(&tx, id, actor, "taken", "")?,
            // logged under `actor` — who ran 'tb assign', i.e. who assigned it — never under
            // `owner` (who now holds it): that split is what lets `tb show`/`tb log` answer
            // "who assigned this" and "who holds this" as two different questions, the way
            // every other event's `actor` column already answers "who did this". `owner` here
            // is who the card was JUST assigned to (see the `(Kind::Assign, _)` arm above), so
            // it is always `Some` — `log_assign` writes it into its own structured column too.
            Kind::Assign => Self::log_assign(&tx, id, actor, owner.as_deref().unwrap_or(""))?,
            Kind::Drop => Self::log(&tx, id, actor, "dropped", &format!("{} -> todo", c.column))?,
            Kind::Release => {
                let text = match change {
                    Change::Release { text, .. } => text,
                    _ => "",
                };
                Self::log(&tx, id, actor, "released", text)?
            }
            Kind::Move => {
                if block_cleared {
                    Self::log(&tx, id, actor, "unblocked", "cleared on done")?;
                }
                Self::log(&tx, id, actor, "moved", &format!("{} -> {column}", c.column))?;
                if let (true, Some(r)) = (send_back || fail_to_todo, reason) {
                    Self::log(&tx, id, actor, "returned", r)?;
                }
            }
        }
        // a card that reached DONE lifts the blocks that named it (store/blocks.rs) — in
        // this transaction, so the move and the unblocks land together or not at all
        if column == "done" && c.column != "done" {
            blocks::on_done(&tx, id, actor)?;
        }
        let c = get_card(&tx, id)?;
        tx.commit()?;
        Ok(c)
    }

    /// A plain read of the card a `change` is ABOUT, and the column it would move to — worked
    /// out with the same selection logic `transition_inner` uses (`claim_target` takes any
    /// `&Connection`, so calling it with `&self.conn` here needs no transaction), but outside
    /// any transaction: this is what a pre-change hook is asked about, and asking it must never
    /// hold the board's write lock (see `crate::hooks`'s doc comment). What `transition_inner`
    /// resolves once it actually opens its transaction may differ if the board changed in
    /// between — that is exactly what `snapshot`'s re-check in `transition_inner` is for.
    fn probe(&self, change: Change<'_>) -> Result<(Card, String)> {
        Ok(match change {
            Change::Claim(id) => {
                let target = Self::claim_target(&self.conn, id)?;
                (get_card(&self.conn, target)?, "doing".to_string())
            }
            Change::Assign { id, .. } => {
                let target = Self::claim_target(&self.conn, Some(id))?;
                (get_card(&self.conn, target)?, "doing".to_string())
            }
            Change::Drop(id) | Change::Release { id, .. } => (get_card(&self.conn, id)?, "todo".to_string()),
            Change::Move { id, column, .. } => {
                // caught again, identically, once `transition_inner` opens its transaction —
                // duplicated here only so a hook is never asked about a column that does not
                // exist (nonsense input wastes a hook run and hands it a nonsense payload)
                let column = column.to_ascii_lowercase();
                if !COLUMNS.contains(&column.as_str()) {
                    return err(format!(
                        "unknown column '{column}' — use one of todo, doing, review, done: 'tb move {id} doing'"
                    ), Code::InvalidValue);
                }
                (get_card(&self.conn, id)?, column)
            }
        })
    }

    /// The public face of [`transition_inner`](Self::transition_inner): asks the board's
    /// pre-change hook (if any) BEFORE that function opens its write transaction, and its
    /// post-change hook (if any) AFTER that transaction commits. See `crate::hooks` for why the
    /// hook itself runs as a child process outside any lock, and `transition_inner`'s own doc
    /// comment for the `snapshot` re-check that keeps a hook's answer honest.
    ///
    /// **Where the hook sits, and why not "last"**: a hook that says no is checked FIRST, before
    /// `transition_inner`'s five guards (holder, self-approval, `done-by`, `done-needs-note`,
    /// `done-needs-link`, the WIP cap) ever run — the opposite of "only once everything else
    /// already allows it". The one thing that forces this is the deadlock rule: a hook is a
    /// child process, and the only way to guarantee it never waits on a lock THIS process holds
    /// is to ask it before that lock is taken at all — see `crate::hooks`'s doc comment. The
    /// five guards stay authoritative regardless of order: `transition_inner` re-runs every one
    /// of them for real, inside its own transaction, so a hook's "allowed" is necessary but
    /// never sufficient — a change a hook approved can still be refused a moment later by the
    /// WIP cap, the holder rule, or (via `snapshot`) by having simply changed underneath it. A
    /// hook can never be used to bypass a guard; the only cost of asking it first is that a
    /// change already doomed by an ordinary guard (an unknown column, a full board) sometimes
    /// asks the hook anyway before failing — `probe` catches the cheapest of these (an unknown
    /// column) so a hook is at least never handed nonsense, but it is not asked to re-derive
    /// every refusal `transition_inner` would reach on its own.
    ///
    /// `gate` decides three things: whether a hook is asked at all (`HookGate::Sync` — tb's own
    /// GitHub sync — is exempt by construction, never by actor name; a hook's own `tb` call on
    /// this board skips it only with a live run ticket, and is logged as `hook-nested`), whether the pre-change hook actually runs or is skipped with `--break-glass`
    /// (`HookGate::BreakGlass`, logged on the card and the board — `store::gate` — never
    /// silently; refused outright when this board asks for no hook to break), and nothing about
    /// the guards in `transition_inner`, which run exactly as they always did.
    fn transition(&mut self, change: Change<'_>, actor: &str, force: bool, gate: HookGate<'_>) -> Result<Card> {
        let sync = matches!(gate, HookGate::Sync);
        let board = self.hook_board();
        // a hook's own `tb` call on this very board, backed by a live run ticket — never by an
        // environment variable anyone can set (`hooks::nested`); recorded below, never silent
        let nested = if sync { None } else { hooks::nested(&board) };
        let live = !sync && nested.is_none();
        let asks = (self.hook(hooks::Event::PreChange)?, self.hook(hooks::Event::PostChange)?);
        let nested = nested.filter(|_| asks.0.is_some() || asks.1.is_some());
        let (pre_name, post_name) = if live { asks } else { (None, None) };
        if live && pre_name.is_none() && matches!(gate, HookGate::BreakGlass(_)) {
            return Err(gate::no_gate_err());
        }
        let before = if pre_name.is_some() || post_name.is_some() { Some(self.probe(change)?) } else { None };
        // (id, from-column, from-owner, the run to log once the change is committed) — kept
        // only when a pre-change hook actually ran and approved; `None` for break-glass, sync,
        // nested and "no hook configured" alike, so nothing extra is logged for any of them.
        let mut approved: Option<(i64, String, Option<String>, hooks::Run)> = None;
        // (hook name, why) — kept only for a break-glass that reaches a real change: logging it
        // against a change the OTHER guards (holder, self-approval, WIP, …) went on to refuse
        // anyway would be a false record that a gate was skipped when nothing moved at all.
        let mut break_glass: Option<(String, String)> = None;
        if let (Some(name), Some((card, to))) = (&pre_name, &before) {
            match gate {
                HookGate::BreakGlass(why) => break_glass = Some((name.clone(), why.to_string())),
                HookGate::Normal | HookGate::Sync => {
                    let payload = crate::contract::card_by_id(&*self, card.id)?;
                    let payload = serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null);
                    let text = hooks::payload(hooks::Event::PreChange, &self.name, &payload, &card.column, to, actor, now(), force, None);
                    let run = hooks::fire(hooks::Event::PreChange, name, &text, &board)?;
                    approved = Some((card.id, card.column.clone(), card.owner.clone(), run));
                }
            }
        }
        let snapshot = approved.as_ref().map(|(id, col, owner, _)| (*id, col.as_str(), owner.as_deref()));
        let result = self.transition_inner(change, actor, force, snapshot);
        if let (Ok(c), Some((name, why))) = (&result, &break_glass) {
            let _ = self.log_break_glass(c.id, actor, name, why);
        }
        if let (Ok(c), Some(n)) = (&result, &nested) {
            let _ = self.log_nested(c.id, actor, n);
        }
        if let (Ok(c), Some((_, _, _, run))) = (&result, &approved) {
            // best-effort: the change itself already succeeded and must not be undone by a
            // failure to write one more log line about it
            let _ = self.log_hook(c.id, actor, &run.line());
        }
        if let (Ok(c), Some(name)) = (&result, &post_name) {
            let card_v = crate::contract::card_by_id(&*self, c.id)
                .ok()
                .and_then(|p| serde_json::to_value(&p).ok())
                .unwrap_or(serde_json::Value::Null);
            let from = before.as_ref().map(|(card, _)| card.column.as_str()).unwrap_or(c.column.as_str());
            let text = hooks::payload(hooks::Event::PostChange, &self.name, &card_v, from, &c.column, actor, now(), force, None);
            // a post-change hook never decides anything: a failure is a line to report, not a
            // change to undo (`hooks::fire_after` already turns an Err into that line)
            let line = match hooks::fire_after(name, &text, &board) {
                Ok(run) => run.line(),
                Err(msg) => format!("{name} failed: {msg}"),
            };
            let _ = self.log_hook(c.id, actor, &line);
        }
        result
    }

    /// Hard-delete a card with its checklist, links and events; logged on the board.
    pub fn delete_card(&mut self, id: i64, actor: &str) -> Result<Card> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let c = get_card(&tx, id)?;
        tx.execute("DELETE FROM checklist WHERE card_id=?", [id])?;
        tx.execute("DELETE FROM links WHERE card_id=?", [id])?;
        tx.execute("DELETE FROM events WHERE card_id=?", [id])?;
        tx.execute("DELETE FROM cards WHERE id=?", [id])?;
        Self::log_board(&tx, actor, "delete", &format!("deleted #{id} \"{}\"", c.title))?;
        tx.commit()?;
        Ok(c)
    }

    /// Reorder a card within its column: `top`, `bottom`, `up`, `down`.
    pub fn reorder(&mut self, id: i64, how: &str, actor: &str) -> Result<Card> {
        // a reorder that moves nothing writes no event, so it would never reach the check in
        // `log`: ask here, so a name the board does not know is refused consistently rather
        // than being told a no-op succeeded
        access::guard_actor(&self.conn, actor)?;
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let c = get_card(&tx, id)?;
        // `prio` edits POSITION, so it walks the column in position order whatever the board
        // sorts by (under `sort due` position is the tie-break between equal dates)
        let mut ids: Vec<i64> = {
            let mut st = tx.prepare(&format!(r#"SELECT {CARD_COLS} FROM cards WHERE "column"=?"#))?;
            let mut v = st.query_map([&c.column], row_card)?.collect::<rusqlite::Result<Vec<Card>>>()?;
            v.sort_by(order::by_position);
            v.iter().map(|c| c.id).collect()
        };
        let i = ids.iter().position(|x| *x == id).unwrap_or(0);
        let j = match how {
            "top" => 0,
            "bottom" => ids.len() - 1,
            "up" => i.saturating_sub(1),
            "down" => (i + 1).min(ids.len() - 1),
            _ => {
                return err(format!("unknown '{how}' — use 'tb prio {id} top|bottom|up|down'"), Code::InvalidValue);
            }
        };
        let moved = ids.remove(i);
        ids.insert(j, moved);
        for (p, cid) in ids.iter().enumerate() {
            tx.execute("UPDATE cards SET position=? WHERE id=?", params![p as i64, cid])?;
        }
        if i != j {
            Self::log(&tx, id, actor, "prio", how)?;
        }
        let c = get_card(&tx, id)?;
        tx.commit()?;
        Ok(c)
    }

    /// Edit title (re-parsing `tag:` and `gh#N`; an absent gh#N keeps the old ref) and/or description.
    /// Edit a card. `raw_title`/`desc` of `None` leave that field alone (only the fields
    /// the caller changed are written). `baseline` = what the caller saw when they started
    /// (the TUI form): a field the caller changed that someone else changed since the form
    /// opened is refused instead of overwritten — the form never puts old values back.
    pub fn edit(
        &mut self,
        id: i64,
        raw_title: Option<&str>,
        desc: Option<&str>,
        actor: &str,
        baseline: Option<(&str, &str)>,
    ) -> Result<Card> {
        self.edit_tagged(id, raw_title, desc, actor, baseline, None)
    }

    /// `edit` with an explicit `--tag` (`store::closing`).
    #[allow(clippy::too_many_arguments)]
    pub fn edit_tagged(
        &mut self,
        id: i64,
        raw_title: Option<&str>,
        desc: Option<&str>,
        actor: &str,
        baseline: Option<(&str, &str)>,
        tag: Option<Option<&str>>,
    ) -> Result<Card> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let c = get_card(&tx, id)?;
        let (mut skip_title, mut skip_desc) = (false, false);
        if let Some((base_title, base_desc)) = baseline {
            // Only fields the caller CHANGED are written. A field typed back at its
            // open-time value is skipped (never written — no stale overwrite). A field
            // they changed that someone else changed since the form opened is refused.
            let conflict = |field: &str, base: &str, now: &str, typed: &str| -> Option<String> {
                (now != base && typed != now).then(|| {
                    format!("#{id} changed while you were editing — {field} has newer text; reopen with e")
                })
            };
            if let Some(t) = raw_title {
                // the baseline for titles is the raw `tag: gh#N title` string, not the
                // parsed title the card stores
                let now_raw = crate::store::raw_title(&c);
                if t == base_title {
                    skip_title = true;
                } else if let Some(e) = conflict("title", base_title, &now_raw, t) {
                    return err(e, Code::Unknown);
                }
            }
            if let Some(d) = desc {
                if d == base_desc {
                    skip_desc = true;
                } else if let Some(e) = conflict("description", base_desc, &c.description, d) {
                    return err(e, Code::Unknown);
                }
            }
        }
        let mut what = Vec::new();
        if let (Some(t), false) = (raw_title, skip_title) {
            if t.trim().is_empty() {
                return err(format!("title is empty — try 'tb edit {id} --title \"tag: new title\"'"), Code::ArgRequired);
            }
            let (guessed, gh, title) = match tag {
                Some(_) => parse_title_keeping_prefix(t),
                None => parse_title(t),
            };
            // A tag no title could carry (`00-key 2`) was set on purpose with `--tag`: a
            // later title edit must not quietly drop it. Every tag tb could have guessed
            // behaves exactly as before.
            let keep = c.tag.clone().filter(|t| !closing::from_a_title_prefix(t));
            let new_tag = match tag {
                Some(explicit) => explicit.map(str::to_string),
                None => guessed.or(keep),
            };
            tx.execute(
                "UPDATE cards SET title=?, tag=?, gh_ref=? WHERE id=?",
                params![title, new_tag, gh.or(c.gh_ref), id],
            )?;
            what.push("title");
        } else if let Some(explicit) = tag {
            tx.execute("UPDATE cards SET tag=? WHERE id=?", params![explicit, id])?;
            what.push("tag");
        }
        if let (Some(d), false) = (desc, skip_desc) {
            tx.execute("UPDATE cards SET description=? WHERE id=?", params![d.trim(), id])?;
            what.push("description");
        }
        if what.is_empty() {
            return err(format!("nothing to change — 'tb edit {id} --title T' and/or '--desc D'"), Code::Unknown);
        }
        Self::log(&tx, id, actor, "edit", &format!("{} edited", what.join(" and ")))?;
        let c = get_card(&tx, id)?;
        tx.commit()?;
        Ok(c)
    }

    /// doing -> review; review -> done (by a verifier, store/verifier.rs). A TODO card is
    /// refused by the same guard `tb move ID done` meets: nothing reaches DONE except from REVIEW.
    pub fn done(&mut self, id: i64, actor: &str) -> Result<Card> {
        self.done_opts(id, actor, false, None)
    }

    /// `done` with `--force`: see `move_to_forced`.
    pub fn done_forced(&mut self, id: i64, actor: &str) -> Result<Card> {
        self.done_opts(id, actor, true, None)
    }

    /// `done` (or `done --force`) with `--break-glass "why"`: see `move_opts_bg`.
    pub fn done_bg(&mut self, id: i64, actor: &str, force: bool, break_glass: Option<&str>) -> Result<Card> {
        self.done_opts(id, actor, force, break_glass)
    }

    fn done_opts(&mut self, id: i64, actor: &str, force: bool, break_glass: Option<&str>) -> Result<Card> {
        let c = self.card(id)?;
        match c.column.as_str() {
            "doing" => self.move_card(id, "review", actor, force, None, break_glass),
            "todo" | "review" => self.move_card(id, "done", actor, force, None, break_glass),
            _ => err(format!(
                "card #{id} is already done — reopen with 'tb move {id} todo'"
            ), Code::Unknown),
        }
    }

    /// `drop_card` with `--force`: the actor is not the owner but means it; logged.
    pub fn drop_card_forced(&mut self, id: i64, actor: &str) -> Result<Card> {
        self.drop_card_inner(id, actor, true, None)
    }

    pub fn drop_card(&mut self, id: i64, actor: &str) -> Result<Card> {
        self.drop_card_inner(id, actor, false, None)
    }

    /// `drop` (or `drop --force`) with `--break-glass "why"`: see `move_opts_bg`.
    pub fn drop_card_bg(&mut self, id: i64, actor: &str, force: bool, break_glass: Option<&str>) -> Result<Card> {
        self.drop_card_inner(id, actor, force, break_glass)
    }

    /// Back to todo, unowned. Someone else's DOING card is refused (the same hazard as moving
    /// it, see move_card) unless forced; the check, the `force` event and the drop are one
    /// transaction.
    fn drop_card_inner(&mut self, id: i64, actor: &str, force: bool, break_glass: Option<&str>) -> Result<Card> {
        self.transition(Change::Drop(id), actor, force, HookGate::of(break_glass))
    }

    /// The move behind `tb release` (store/release.rs, which runs its checks first): DOING →
    /// TODO, unowned, logged as one `released` event carrying `text`. No `--force`: refused
    /// unless the card is still in DOING held by `holder` when the transaction opens.
    pub(crate) fn release_card(&mut self, id: i64, holder: &str, text: &str, actor: &str) -> Result<Card> {
        self.transition(Change::Release { id, holder, text }, actor, false, HookGate::Normal)
    }
}

/// A column change, as asked for (see `Store::transition`).
#[derive(Clone, Copy)]
enum Change<'a> {
    /// `next` (None: the top unblocked TODO card) / `take ID`: TODO → DOING, owned by the actor.
    Claim(Option<i64>),
    /// `move` / `done` / send-back / GitHub sync; `reason` only with REVIEW → DOING.
    Move { id: i64, column: &'a str, reason: Option<&'a str> },
    /// `drop`: back to TODO, unowned.
    Drop(i64),
    /// `assign ID NAME`: TODO → DOING, owned by `owner` — never the actor running the
    /// command. Only reaches a TODO card (the same restriction `claim_target` gives `take`,
    /// and `take` itself has no `--force` to take a card away from its current holder
    /// either), so the holder guard never needs a separate check here: a card already held
    /// by someone is simply not a valid target, by construction.
    Assign { id: i64, owner: &'a str },
    /// `release`: a DOING card held by `holder` (checked dead by store/release.rs) back to
    /// TODO, unowned; `text` is the `released` event. Not a holder-rule bypass by `--force`:
    /// the liveness check is the permission, and it is refused if the holder changed since.
    Release { id: i64, holder: &'a str, text: &'a str },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Claim,
    Move,
    Drop,
    Assign,
    Release,
}

/// How a change relates to the pre/post-change hook (`config hook`, `crate::hooks`) — see
/// `Store::transition`. Every existing call site is `Normal`; the other two are opt-in and
/// mutually exclusive by construction (one `Change` call carries exactly one `HookGate`).
#[derive(Clone, Copy)]
enum HookGate<'a> {
    /// A person or agent, via the CLI or the full-screen board — a hook always runs.
    Normal,
    /// tb's own GitHub sync (`Store::move_sync`): an internal ORIGIN, never derived from the
    /// actor name (`--as github` is refused before this, in `main.rs`) — it writes down what a
    /// merged PR or a closed issue already says, not a person or agent proposing a change.
    Sync,
    /// `--break-glass "why"`: skip the pre-change hook this board asks for, and say why —
    /// refused outright when this board asks for no hook (`store::gate::no_gate_err`).
    BreakGlass(&'a str),
}

impl<'a> HookGate<'a> {
    fn of(break_glass: Option<&'a str>) -> HookGate<'a> {
        match break_glass {
            Some(why) => HookGate::BreakGlass(why),
            None => HookGate::Normal,
        }
    }
}

/// Rework round from a card's events: 1, plus one for every time it was sent back.
pub fn round_of(events: &[Event]) -> i64 {
    1 + events.iter().filter(|e| e.kind == "returned").count() as i64
}

/// Position for a card appended to the bottom of `column`.
fn bottom_of(conn: &Connection, column: &str) -> Result<i64> {
    Ok(conn.query_row(r#"SELECT COALESCE(MAX(position), -1) + 1 FROM cards WHERE "column"=?"#, [column], |r| r.get(0))?)
}

/// The card's `gh#N` when it is not already in the title text (a mid-title ref stays there),
/// i.e. the ref a display should put in front of the title.
pub fn shown_ref(c: &Card) -> Option<i64> {
    let n = c.gh_ref?;
    let token = format!("gh#{n}");
    (!c.title.split_whitespace().any(|w| w.eq_ignore_ascii_case(&token))).then_some(n)
}

/// The raw title as typed: `tag: gh#N title` (what `edit` pre-fills).
pub fn raw_title(c: &Card) -> String {
    let mut s = String::new();
    // only a tag a `tag:` prefix could produce is rebuilt into the raw title; an explicit
    // one (`--tag "00-key 2"`) stays on the card and out of the text being edited
    if let Some(t) = c.tag.as_deref().filter(|t| closing::from_a_title_prefix(t)) {
        s.push_str(&format!("{t}: "));
    }
    if let Some(n) = shown_ref(c) {
        s.push_str(&format!("gh#{n} "));
    }
    s.push_str(&c.title);
    s
}

fn wip_of(conn: &Connection) -> Result<i64> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM config WHERE key='wip'", [], |r| r.get(0))
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_WIP))
}

fn theme_of(conn: &Connection) -> Result<String> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM config WHERE key='theme'", [], |r| r.get(0))
        .optional()?;
    Ok(v.filter(|t| t == "light").unwrap_or_else(|| "dark".into()))
}

fn get_card(conn: &Connection, id: i64) -> Result<Card> {
    conn.query_row(&format!("SELECT {CARD_COLS} FROM cards WHERE id=?"), [id], row_card)
        .optional()?
        .ok_or_else(|| BoardError(format!("no card #{id} — see 'tb list' for ids"), Code::NoCard))
}

/// Compact age: 40m, 1h12m, 2d.
/// Coarse age for fixed-width columns (always <= 4 chars): 42m, 10h, 3d.
pub fn coarse_age(secs: i64) -> String {
    let s = secs.max(0);
    if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86400)
    }
}

pub fn fmt_age(secs: i64) -> String {
    let s = secs.max(0);
    if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        let (h, m) = (s / 3600, (s % 3600) / 60);
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h{m:02}m")
        }
    } else {
        format!("{}d", s / 86400)
    }
}

pub fn fmt_clock(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(ts, 0).single() {
        Some(t) => t.format("%H:%M").to_string(),
        None => "--:--".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #105: SQLITE_BUSY/SQLITE_LOCKED (another `tb` holds the write lock) reads nothing like
    /// a real path problem (cannot open, read-only, …), and only the latter names TB_DB.
    #[test]
    fn a_locked_database_and_a_path_problem_get_different_messages() {
        let locked = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error { code: rusqlite::ErrorCode::DatabaseBusy, extended_code: 5 },
            Some("database is locked".to_string()),
        );
        let path_problem = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error { code: rusqlite::ErrorCode::CannotOpen, extended_code: 14 },
            Some("unable to open database file".to_string()),
        );
        let locked_msg = BoardError::from(locked).0;
        let path_msg = BoardError::from(path_problem).0;
        assert_ne!(locked_msg, path_msg, "a lock and a path problem must not read the same");
        assert!(locked_msg.contains("another tb is writing this board"), "{locked_msg}");
        assert!(!locked_msg.contains("TB_DB"), "a lock is not a TB_DB problem: {locked_msg}");
        // whether the path message names TB_DB depends on whether it is actually set — that
        // exact rule is `the_tb_db_hint_names_it_only_when_set` below, via the pure function,
        // so this does not assert on ambient process environment here
        assert!(path_msg.starts_with("database error: unable to open database file"), "{path_msg}");
    }

    /// #105: TB_DB is named only when it is actually set — otherwise it was never the pin.
    /// Never says "the board file": that placeholder is reserved for when the real file is
    /// genuinely unknown (`position_guard.rs` pins it out of an actual refusal).
    #[test]
    fn the_tb_db_hint_names_it_only_when_set() {
        assert_eq!(db_error_hint(None), "check it is writable");
        assert!(!db_error_hint(None).contains("TB_DB") && !db_error_hint(None).contains("the board file"));
        assert_eq!(db_error_hint(Some("/tmp/some-board.db")), "check TB_DB (/tmp/some-board.db) points at a writable file");
    }

    /// #138: a constraint failure reads as the DATA being refused — it never says "writable",
    /// which sent the person who hit the mv FOREIGN KEY bug off to check file permissions.
    #[test]
    fn a_constraint_failure_does_not_hint_at_writability() {
        let fk = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error { code: rusqlite::ErrorCode::ConstraintViolation, extended_code: 787 },
            Some("FOREIGN KEY constraint failed".to_string()),
        );
        let msg = BoardError::from(fk).0;
        assert!(msg.contains("FOREIGN KEY constraint failed"), "{msg}");
        assert!(!msg.contains("writable"), "a constraint is a data problem, not a file problem: {msg}");
        assert!(!msg.contains("TB_DB"), "{msg}");
    }

    #[test]
    fn title_parsing() {
        assert_eq!(
            parse_title("widgets: gh#327 login form rejects"),
            (Some("widgets".into()), Some(327), "login form rejects".into())
        );
        assert_eq!(parse_title("renew domain"), (None, None, "renew domain".into()));
        assert_eq!(parse_title("Admin: quote"), (Some("admin".into()), None, "quote".into()));
        assert_eq!(parse_title("see http://x"), (None, None, "see http://x".into()));
    }

    #[test]
    fn coarse_ages_fit_four_chars() {
        assert_eq!(coarse_age(42 * 60), "42m");
        assert_eq!(coarse_age(3600 + 42 * 60), "1h");
        assert_eq!(coarse_age(10 * 3600 + 54 * 60), "10h");
        assert_eq!(coarse_age(3 * 86400 + 5), "3d");
        for s in [0, 59, 3599, 3600, 86399, 86400, 99 * 86400] {
            assert!(coarse_age(s).len() <= 4, "{s}");
        }
    }

    #[test]
    fn ages() {
        assert_eq!(fmt_age(40 * 60), "40m");
        assert_eq!(fmt_age(3600 + 12 * 60), "1h12m");
        assert_eq!(fmt_age(2 * 86400 + 5), "2d");
    }

    /// #111: `last_holder_of` must read the ASSIGN holder out of the structured `assignee`
    /// column, never out of the event's human-readable `text` — pinned by mutating only the
    /// write-site's WORDING (never touching `assignee`) and checking the guard is unmoved.
    ///
    /// Reproduces the exact shape a reviewer used to reopen the old text-parsing bypass:
    /// `b` is handed the card by an orchestrator (`assign`, not `take` — the vulnerable path,
    /// since `taken` events already carry the holder in a real column, `actor`), `b` drops it,
    /// and a third party moves the now-unowned card straight into review, the same shape as
    /// `case6` in tests/review.rs — so `author_of` alone would name the mover, not `b`, and
    /// only `last_holder_of` can still catch `b`.
    #[test]
    fn a_reworded_assign_event_does_not_change_who_last_held_the_card() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("b.db")).unwrap();
        let id = s.add("widgets: fix the thing", "", &[], "lead").unwrap();
        s.assign(id, "b", "orchestrator").unwrap();
        s.drop_card(id, "b").unwrap();
        assert_eq!(s.card(id).unwrap().column, "todo");
        assert_eq!(s.card(id).unwrap().owner, None);
        s.move_to(id, "review", "mover").unwrap();
        assert_eq!(s.author(id).unwrap().as_deref(), Some("mover"), "author_of alone would miss b");
        // simulate the write-site's wording changing — the exact mutation the reviewer used:
        // only the message a person reads changes; the structured `assignee` column (what the
        // guard now reads) is untouched
        s.conn
            .execute(
                "UPDATE events SET text = 'handed off to b, go' WHERE card_id = ?1 AND kind = 'assigned'",
                params![id],
            )
            .unwrap();
        assert_eq!(last_holder_of(&s.conn, id).unwrap().as_deref(), Some("b"), "reads assignee, not text");
        // end to end: b really held this card and must still be refused, whatever the assign
        // event's prose says now
        let r = s.done(id, "b");
        assert!(r.is_err(), "a reworded assign event must not reopen the self-approval bypass: {r:?}");
        assert!(r.unwrap_err().to_string().contains("you did this work"));
        assert_eq!(s.card(id).unwrap().column, "review", "the card did not reach done");
        // an uninvolved third party still approves fine, no --force needed
        assert_eq!(s.done(id, "rev").unwrap().column, "done");
    }

    /// #111 migration: an `assigned` event written before the `assignee` column existed has
    /// NULL there — its only record of the holder is the old "assigned to NAME" prose, and
    /// `last_holder_of` must still fall back to parsing it, or an upgraded board would silently
    /// forget every assignment made before the upgrade.
    #[test]
    fn a_pre_migration_assign_event_with_no_assignee_column_value_still_falls_back_to_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("b.db")).unwrap();
        let id = s.add("widgets: fix the thing", "", &[], "lead").unwrap();
        s.assign(id, "b", "orchestrator").unwrap();
        // simulate a row written before this migration: assignee NULL, only the old prose
        s.conn.execute("UPDATE events SET assignee = NULL WHERE card_id = ?1 AND kind = 'assigned'", params![id]).unwrap();
        assert_eq!(last_holder_of(&s.conn, id).unwrap().as_deref(), Some("b"), "falls back to the old prose");
    }
}

/// `conn_notice_key` must agree with itself across the input shapes that broke the earlier,
/// hand-rolled version (`Path::display()` on the pre-open path): a push during `Store::open`
/// and a later drain via `Store::notice_key()` both call it on the SAME open connection, so
/// they can never disagree — these tests prove that for the shapes that matter, not just the
/// plain absolute path every other test already uses.
///
/// `#[cfg(unix)]`: `report_wide_file` (what these trigger) is itself unix-only (`fsperm::mode_of`
/// returns `None` off unix, so it never fires there) — nothing here is testing something that
/// exists on other platforms. `cargo check --target x86_64-pc-windows-gnu` (the project's
/// windows gate) does not compile test code at all (no `--tests`/`--all-targets`), so this
/// module never needs to type-check there either.
///
/// No test here mutates process-global state (`std::env::set_current_dir` / `set_var`): a
/// "relative path" is built by walking up from `std::env::current_dir()` with `..` segments
/// back down into a fresh tempdir, never by changing the process's actual cwd — this test
/// binary runs `#[test]` fns on multiple threads at once, and a real chdir would be exactly
/// the class of cross-test hazard this whole fix exists to remove. `TB_DB` pointing at a
/// relative file is not tested separately: `boards::path_for` does zero transformation on it
/// (`PathBuf::from(the_raw_string)`, read from the source), so at the `Store::open` level it
/// is the identical scenario the relative-path test already covers.
#[cfg(all(test, unix))]
mod notice_key_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// Create `path` (and its parent directories) as a file mode 0644 — "already existed,
    /// world-readable" — the shape `report_wide_file` reports on, deterministically, without
    /// depending on the process umask the way a freshly-`Store::open`-ed file's mode would.
    fn seed_wide_file(path: &Path) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(path, b"").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// Open `path` and assert the wide-file notice it raises is found under exactly the key
    /// `notice_key()` reports for that same store — the property this whole module checks.
    fn assert_key_round_trips(path: &Path) {
        let store = Store::open(path).unwrap();
        let key = store.notice_key().expect("an on-disk board always has a key");
        let pending = crate::notice::take_unprinted_for(&key);
        assert!(
            pending.iter().any(|m| m.contains("is open to other users")),
            "push (inside Store::open) and drain (notice_key(), right after) must agree on the \
             key for {path:?} — got key {key:?}, pending {pending:?}"
        );
    }

    #[test]
    fn a_relative_board_path() {
        // walk up from cwd to `/` with `..`, then back down into a fresh tempdir — a genuine
        // relative Path (no leading `/`) that resolves to the same file as `dir.path()`,
        // without ever touching the process's actual current directory
        let cwd = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ups = "../".repeat(cwd.components().filter(|c| matches!(c, std::path::Component::Normal(_))).count());
        let target = dir.path().join("relative.db");
        let rel = PathBuf::from(format!("{ups}{}", target.strip_prefix("/").unwrap().display()));
        assert!(!rel.is_absolute(), "sanity: the input really is relative: {rel:?}");
        seed_wide_file(&target);
        assert_key_round_trips(&rel);
    }

    #[test]
    fn a_path_with_dotdot_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("board.db");
        seed_wide_file(&target);
        // absolute, but not the shortest form: a `..` segment that only cancels out once
        // resolved, the same shape a hand-typed `--db ../shared/../work/board.db` would take
        let messy = dir.path().join("sub").join("..").join("board.db");
        assert_key_round_trips(&messy);
    }

    #[test]
    fn a_path_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real_file = dir.path().join("real.db");
        let link = dir.path().join("link.db");
        seed_wide_file(&real_file);
        std::os::unix::fs::symlink(&real_file, &link).unwrap();
        assert_key_round_trips(&link);
    }

    #[test]
    fn the_same_board_opened_twice() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("board.db");
        seed_wide_file(&target);
        let store1 = Store::open(&target).unwrap();
        let key1 = store1.notice_key().unwrap();
        drop(store1);
        let store2 = Store::open(&target).unwrap();
        let key2 = store2.notice_key().unwrap();
        assert_eq!(key1, key2, "the same file opened twice must key identically");
        // the notice the FIRST open raised is still sitting there, findable under that same
        // key, exactly as a `reload()` on either store would find it
        let pending = crate::notice::take_unprinted_for(&key1);
        assert!(pending.iter().any(|m| m.contains("is open to other users")), "{pending:?}");
    }
}
