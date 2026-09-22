//! SQLite store: cards, checklist, events, config. WAL mode, atomic claims.

use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

pub mod actors;
pub mod archive;
pub mod blocks;
pub mod closing;
pub mod bulk;
pub mod display;
pub mod due;
pub mod kinds;
pub mod order;
pub mod rounds;
pub mod transfer;

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

#[derive(Debug)]
pub struct BoardError(pub String);

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
        if is_contended(&e) {
            // the file is fine — another `tb` is mid-write and holds the lock; TB_DB is not
            // the problem here, so it is not named (#105)
            return BoardError("database is locked — another tb is writing this board right now: wait a moment and try again".to_string());
        }
        BoardError(format!("database error: {e} — {}", db_error_hint(crate::env("DB").as_deref())))
    }
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
    )))
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
    BoardError(format!("doing is full ({doing}/{wip}: {}) — {tail}", holders.join(", ")))
}

/// Who did the work on a card: its OWNER — the agent that held it in DOING — whenever it has
/// one. Only a card that reached REVIEW with no owner falls back to whoever moved it there,
/// and never to the `github` sync (its moves are automation, not work).
///
/// Keying on the owner and not on the last mover is what makes the never-self-approve rule
/// point at the right agent: a reviewer who pushes a stuck card into REVIEW does not inherit
/// the work, and the worker who held the card cannot escape the rule by letting someone else
/// move it.
fn author_of(conn: &Connection, c: &Card) -> Result<Option<String>> {
    if c.owner.is_some() {
        return Ok(c.owner.clone());
    }
    let mover: Option<String> = conn
        .query_row(
            "SELECT actor FROM events WHERE card_id=? AND kind='moved' AND text LIKE '% -> review' ORDER BY id DESC LIMIT 1",
            [c.id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match mover {
        Some(a) if a != "github" => Some(a),
        _ => None,
    })
}

pub(crate) fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(BoardError(msg.into()))
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
        ))),
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
    text TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS events_card ON events(card_id, id);
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
        Box::new(crate::store::BoardError(position_error(id, value))),
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
    // migration: `blocked_on` / `blocked_until` (v2, `tb block --on … --until …`)
    blocks::migrate(conn)?;
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
        crate::notice::push(format!(
            "{} was written by an older tb: it was backed up to {} before its schema was upgraded — to go back, see \"Going back to an older tb\" in UPGRADING.md",
            path.display(),
            backup.display()
        ));
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
        ))
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
        reader.busy_timeout(Duration::from_secs(10)).map_err(|e| e.to_string())?;
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
    crate::notice::push(format!(
        "{} is open to other users (mode {}) — make it private with {}, or keep it that way with {}",
        real.display(),
        crate::fsperm::fmt_mode(mode),
        config_cmd(path, "file-mode private"),
        config_cmd(path, "file-mode shared"),
    ));
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
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| {
                    BoardError(format!(
                        "cannot create {}: {e} — set TB_DB to a writable path",
                        dir.display()
                    ))
                })?;
            }
        }
        // A board file is born private (0600): SQLite then opens the empty file as a new
        // database and gives the -wal/-shm sidecars the same mode. An existing file is never
        // re-moded here; `report_wide_file` says so instead. `real` is where the board lives:
        // `path` itself, or the end of its chain of symbolic links — tb creates THAT file
        // (SQLite would create a link's missing target 0644) and opens the database there.
        let (real, created) = if on_disk {
            crate::fsperm::create_board(path).map_err(BoardError)?
        } else {
            (path.to_path_buf(), false)
        };
        // the file every later refusal names (`position_error`): where the board really is,
        // whether it was named by `TB_DB`, by `-b NAME`, by `TB_BOARD` or by the saved default
        remember_board_file(on_disk.then_some(real.as_path()));
        let mut conn = Connection::open(&real)?;
        conn.busy_timeout(Duration::from_secs(10))?;
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;")?;
        upgrade(&mut conn, &real, on_disk)?;
        if on_disk && !created {
            report_wide_file(&conn, path, &real);
        }
        // migration: who did the work (`actors`, `events.actor_id`, `board_events.actor_id`)
        actors::migrate(&conn)?;
        Ok(Store { conn, name: crate::boards::DEFAULT_BOARD.into() })
    }

    /// The database file (None for an in-memory board).
    pub fn path(&self) -> Option<std::path::PathBuf> {
        self.conn.path().filter(|p| !p.is_empty()).map(std::path::PathBuf::from)
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

    /// Delete every card, checklist item and event (config is kept).
    pub fn wipe(&self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM checklist; DELETE FROM events; DELETE FROM cards; DELETE FROM board_events;
             DELETE FROM sqlite_sequence WHERE name IN ('cards','events','board_events');",
        )?;
        Ok(())
    }

    /// Every card event is written here, so this is where it gets its identity (`actor_id`).
    fn log(conn: &Connection, id: i64, actor: &str, kind: &str, text: &str) -> Result<()> {
        let ts = now();
        let actor_id = actors::stamp(conn, actor, ts)?;
        conn.execute(
            "INSERT INTO events(card_id, ts, actor, kind, text, actor_id) VALUES (?,?,?,?,?,?)",
            params![id, ts, actor, kind, text, actor_id],
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
            return err(format!("wip must be 1-{MAX_WIP} — try 'tb config wip 3'"));
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
            ));
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
                ))
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

    pub fn set_panel(&self, key: &str, value: &str) -> Result<()> {
        let v = value.trim().to_ascii_lowercase();
        let v = match v.as_str() {
            "shown" | "show" | "on" => "shown",
            "hidden" | "hide" | "off" => "hidden",
            _ => return err(format!("'{value}' is not shown|hidden — try 'tb config {key} hidden'")),
        };
        if key != "github-panel" && key != "agents-panel" {
            return err(format!("unknown panel '{key}' — use github-panel or agents-panel"));
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
        all.extend(self.rounds_settings()?);
        all.extend(self.kind_settings()?);
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
                    return err("this board has no file yet — add a card first, e.g. 'tb add \"title\"'");
                };
                if crate::fsperm::mode_of(&path).is_none() {
                    return err("this platform has no file modes — there is nothing to tighten; see 'tb config'");
                }
                let done = crate::fsperm::make_private(&path).map_err(|e| {
                    BoardError(format!("cannot change the mode of {}: {e} — check that you own the file, then 'tb config file-mode private' again", path.display()))
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
            other => err(format!("'{other}' is not private|shared — try 'tb config file-mode private'")),
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
            return err(format!("unknown layout '{layout}' — use 'tb config layout auto|focus|third-h|third-v|half-h|half-v'"));
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
            return err("title is empty — try 'tb add \"tag: what to do\"'");
        }
        let (tag, gh, title) = match tag {
            None => parse_title(raw_title),
            Some(explicit) => {
                let (_, gh, title) = parse_title_keeping_prefix(raw_title);
                (explicit.map(str::to_string), gh, title)
            }
        };
        let tx = self.conn.unchecked_transaction()?;
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
        Ok(CardDetail { card, checklist, events, round, escalate, approved_by, actors })
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
        self.claim(None, actor)
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
            });
        };
        let changed = tx.execute(
            r#"UPDATE cards SET reviewer=? WHERE id=? AND "column"='review' AND reviewer IS NULL"#,
            params![actor, target],
        )?;
        if changed != 1 {
            return err(format!("card #{target} was claimed by someone else — try 'tb next --review'"));
        }
        Self::log(&tx, target, actor, "reviewing", "")?;
        let card = get_card(&tx, target)?;
        tx.commit()?;
        Ok(card)
    }

    /// Atomically take a specific todo card.
    pub fn take(&mut self, id: i64, actor: &str) -> Result<Card> {
        self.claim(Some(id), actor)
    }

    /// `BEGIN IMMEDIATE` + compare-and-swap on the column, so two callers can never
    /// both win the same card.
    fn claim(&mut self, id: Option<i64>, actor: &str) -> Result<Card> {
        self.transition(Change::Claim(id), actor, false)
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
                    ));
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
                        return err("no todo cards — add one with 'tb add \"title\"'")
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
            return err(format!("note is empty — try 'tb note {id} \"what changed\"'"));
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
            ));
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
            return err(format!("say what blocks it — 'tb block {id} \"#7\"'"));
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
            return err(format!("check item is empty — try 'tb check {id} --add \"write test\"'"));
        }
        let tx = self.conn.unchecked_transaction()?;
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
            ));
        };
        let tx = self.conn.unchecked_transaction()?;
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

    pub fn move_to(&mut self, id: i64, column: &str, actor: &str) -> Result<Card> {
        self.move_card(id, column, actor, false, None)
    }

    /// `move_to` that lets the author approve their own REVIEW card; logged as a `force` event.
    pub fn move_to_forced(&mut self, id: i64, column: &str, actor: &str) -> Result<Card> {
        self.move_card(id, column, actor, true, None)
    }

    /// `move_to` with every option: `reason` is required (and only allowed) when a REVIEW
    /// card goes back to DOING.
    pub fn move_opts(&mut self, id: i64, column: &str, actor: &str, force: bool, reason: Option<&str>) -> Result<Card> {
        self.move_card(id, column, actor, force, reason)
    }

    /// Send a REVIEW card back to its owner in DOING with the reason (a `returned` event).
    pub fn send_back(&mut self, id: i64, reason: &str, actor: &str) -> Result<Card> {
        self.move_card(id, "doing", actor, false, Some(reason))
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

    fn move_card(&mut self, id: i64, column: &str, actor: &str, force: bool, reason: Option<&str>) -> Result<Card> {
        self.transition(Change::Move { id, column, reason }, actor, force)
    }

    /// EVERY column change goes through here — `next`/`take`, `move`/`done`/send-back (and the
    /// GitHub sync, which calls `move_to`), and `drop` — from the CLI and the full-screen
    /// board alike. One transaction (`BEGIN IMMEDIATE`), and one fixed order:
    ///
    /// 1. what is asked: the card, the column it goes to, and the refusals that belong to the
    ///    request itself (unknown column, a send-back without its reason, nothing to take);
    ///    a change that changes nothing ends here;
    /// 2. the guards, always in this order: the holder (leaving DOING needs the card's owner,
    ///    or `--force`, logged) → self-approval (REVIEW → DONE by the card's author, or
    ///    `--force`, logged) → `done-by` (entering DONE needs to be one of the named closers,
    ///    or `--force`, logged) → `done-needs-note` (entering DONE needs a note written during
    ///    the stay being left, or `--force`, logged) → the WIP limit (entering DOING, except a
    ///    send-back). The first two are about WHO may touch the card; `done-by` and
    ///    `done-needs-note` are about closing it responsibly, so they come after — a person
    ///    blocked by ownership or self-approval never even reaches the closing checks.
    /// 3. the change, then its events.
    ///
    /// A new guard — and a hook on a change — belongs in step 2, after the ones that are there.
    fn transition(&mut self, change: Change<'_>, actor: &str, force: bool) -> Result<Card> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // 1. what is asked
        let (c, column, reason) = match change {
            Change::Claim(id) => {
                let target = Self::claim_target(&tx, id)?;
                (get_card(&tx, target)?, "doing".to_string(), None)
            }
            Change::Drop(id) => {
                let c = get_card(&tx, id)?;
                if c.column == "todo" && c.owner.is_none() {
                    return Ok(c);
                }
                (c, "todo".to_string(), None)
            }
            Change::Move { id, column, reason } => {
                let column = column.to_ascii_lowercase();
                if !COLUMNS.contains(&column.as_str()) {
                    return err(format!(
                        "unknown column '{column}' — use one of todo, doing, review, done: 'tb move {id} doing'"
                    ));
                }
                let reason = reason.map(str::trim);
                let c = get_card(&tx, id)?;
                let send_back = c.column == "review" && column == "doing";
                if send_back && !matches!(reason, Some(r) if !r.is_empty()) {
                    return err(format!(
                        "say why it goes back — 'tb move {id} doing \"what to fix\"'"
                    ));
                }
                if !send_back && reason.is_some() {
                    return err(format!(
                        "a reason only goes with sending a REVIEW card back to doing — log it with 'tb note {id} \"...\"'"
                    ));
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
        let id = c.id;
        let kind = match change {
            Change::Claim(_) => Kind::Claim,
            Change::Drop(_) => Kind::Drop,
            Change::Move { .. } => Kind::Move,
        };
        let send_back = kind == Kind::Move && c.column == "review" && column == "doing";

        // 2. the guards
        // Card ids are small shared integers: an off-by-one must not move someone else's
        // work. Leaving DOING requires the owner (or --force, logged as its own event).
        // The `github` automation is exempt: its moves are evidence-driven and logged.
        if c.column == "doing" && actor != "github" {
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
        if kind == Kind::Move && column == "done" && c.column == "review" {
            if let Some(author) = author_of(&tx, &c)? {
                if author.eq_ignore_ascii_case(actor) {
                    if !force {
                        return err("you did this work — ask another person or agent to review it");
                    }
                    Self::log(&tx, id, actor, "force", "approved own work")?;
                }
            }
        }
        // who may close a card (`config done-by`, store/closing.rs) — an honest-mistake stop,
        // never security: names are self-asserted, and `--force` is open to everyone (logged).
        // It guards EVERY way into DONE, so moving a card out of review first is not a way
        // round it. `github` is exempt, as it is for the holder rule.
        if column == "done" && c.column != "done" {
            if let Some(names) = closing::may_close(&tx, actor)? {
                if !force {
                    return Err(closing::not_allowed(id, actor, &names));
                }
                Self::log(&tx, id, actor, "force", &format!("closed #{id}, not on the done-by list"))?;
            }
        }
        // a closing note (`config done-needs-note`, store/closing.rs) — off by default (a
        // board that sets nothing is unchanged). "A note" means one written during the stay
        // being left, not one from an earlier round: a note from round 1 must not silently
        // satisfy round 3's close. `--force` is open to everyone and logged, exactly like the
        // guards above; `github` is exempt — a merged PR is its own trace, the same reasoning
        // as the holder and `done-by` exemptions.
        if column == "done" && c.column != "done" && actor != "github" && closing::needs_note(&tx, id)? {
            if !force {
                return Err(closing::no_note_err(id));
            }
            Self::log(&tx, id, actor, "force", &format!("closed #{id} with no note since it entered {}", c.column))?;
        }
        // a returned card is its owner's existing work, not new work: WIP does not block it
        if column == "doing" && !send_back {
            let wip = wip_of(&tx)?;
            // `wip-counts-blocked no` (store/blocks.rs) discounts blocked DOING cards, up to
            // `wip` of them, so waiting for someone else does not stall the board — and
            // blocking everything can still never hand out unlimited work
            let (counted, doing) = blocks::doing_counts(&tx, wip)?;
            if counted >= wip {
                return Err(wip_full_err(&tx, doing, wip, actor));
            }
        }

        // 3. the change, then its events
        let owner = match (kind, column.as_str()) {
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
            return err(format!("card #{id} was taken by someone else — try 'tb next'"));
        }
        match kind {
            Kind::Claim => Self::log(&tx, id, actor, "taken", "")?,
            Kind::Drop => Self::log(&tx, id, actor, "dropped", &format!("{} -> todo", c.column))?,
            Kind::Move => {
                if block_cleared {
                    Self::log(&tx, id, actor, "unblocked", "cleared on done")?;
                }
                Self::log(&tx, id, actor, "moved", &format!("{} -> {column}", c.column))?;
                if let (true, Some(r)) = (send_back, reason) {
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

    /// Hard-delete a card with its checklist and events; logged on the board.
    pub fn delete_card(&mut self, id: i64, actor: &str) -> Result<Card> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let c = get_card(&tx, id)?;
        tx.execute("DELETE FROM checklist WHERE card_id=?", [id])?;
        tx.execute("DELETE FROM events WHERE card_id=?", [id])?;
        tx.execute("DELETE FROM cards WHERE id=?", [id])?;
        Self::log_board(&tx, actor, "delete", &format!("deleted #{id} \"{}\"", c.title))?;
        tx.commit()?;
        Ok(c)
    }

    /// Reorder a card within its column: `top`, `bottom`, `up`, `down`.
    pub fn reorder(&mut self, id: i64, how: &str, actor: &str) -> Result<Card> {
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
                return err(format!("unknown '{how}' — use 'tb prio {id} top|bottom|up|down'"));
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
                    return err(e);
                }
            }
            if let Some(d) = desc {
                if d == base_desc {
                    skip_desc = true;
                } else if let Some(e) = conflict("description", base_desc, &c.description, d) {
                    return err(e);
                }
            }
        }
        let mut what = Vec::new();
        if let (Some(t), false) = (raw_title, skip_title) {
            if t.trim().is_empty() {
                return err(format!("title is empty — try 'tb edit {id} --title \"tag: new title\"'"));
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
            return err(format!("nothing to change — 'tb edit {id} --title T' and/or '--desc D'"));
        }
        Self::log(&tx, id, actor, "edit", &format!("{} edited", what.join(" and ")))?;
        let c = get_card(&tx, id)?;
        tx.commit()?;
        Ok(c)
    }

    /// doing -> review; todo/review -> done.
    pub fn done(&mut self, id: i64, actor: &str) -> Result<Card> {
        self.done_opts(id, actor, false)
    }

    /// `done` with `--force`: see `move_to_forced`.
    pub fn done_forced(&mut self, id: i64, actor: &str) -> Result<Card> {
        self.done_opts(id, actor, true)
    }

    fn done_opts(&mut self, id: i64, actor: &str, force: bool) -> Result<Card> {
        let c = self.card(id)?;
        match c.column.as_str() {
            "doing" => self.move_card(id, "review", actor, force, None),
            "todo" | "review" => self.move_card(id, "done", actor, force, None),
            _ => err(format!(
                "card #{id} is already done — reopen with 'tb move {id} todo'"
            )),
        }
    }

    /// `drop_card` with `--force`: the actor is not the owner but means it; logged.
    /// `drop_card` with `--force`: the actor is not the owner but means it; logged.
    pub fn drop_card_forced(&mut self, id: i64, actor: &str) -> Result<Card> {
        self.drop_card_inner(id, actor, true)
    }

    pub fn drop_card(&mut self, id: i64, actor: &str) -> Result<Card> {
        self.drop_card_inner(id, actor, false)
    }

    /// Back to todo, unowned. Someone else's DOING card is refused (the same hazard as moving
    /// it, see move_card) unless forced; the check, the `force` event and the drop are one
    /// transaction.
    fn drop_card_inner(&mut self, id: i64, actor: &str, force: bool) -> Result<Card> {
        self.transition(Change::Drop(id), actor, force)
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Claim,
    Move,
    Drop,
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
        .ok_or_else(|| BoardError(format!("no card #{id} — see 'tb list' for ids")))
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
}
