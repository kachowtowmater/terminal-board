//! SQLite store: cards, checklist, events, config. WAL mode, atomic claims.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

pub mod due;

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

impl From<rusqlite::Error> for BoardError {
    fn from(e: rusqlite::Error) -> Self {
        BoardError(format!("database error: {e} — check TB_DB points at a writable file"))
    }
}

pub type Result<T> = std::result::Result<T, BoardError>;

/// The refusal for moving someone else's DOING card: what it is held by, what the actor
/// holds, and the escape hatch.
fn ownership_err(tx: &Connection, id: i64, owner: &str, actor: &str, to: &str) -> Result<BoardError> {
    let mine: Vec<i64> = {
        let mut st =
            tx.prepare(r#"SELECT id FROM cards WHERE "column"='doing' AND owner=? COLLATE NOCASE ORDER BY id"#)?;
        let v = st.query_map([actor], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let yours = if mine.is_empty() { "none".to_string() } else { mine.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(", ") };
    Ok(BoardError(format!(
        "#{id} is held by {owner} — your cards: {yours} · to move it to {to} anyway use --force (logged)"
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

fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(BoardError(msg.into()))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
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
        if col == "done" {
            v.sort_by_key(|c| (std::cmp::Reverse(c.column_since), c.id));
        } else {
            v.sort_by_key(|c| (c.position, c.id));
        }
        v
    }
}

/// The clock. `TB_NOW` (unix seconds) pins it; that is for the test suite only, so that
/// fixtures such as `now - 3h` do not depend on the time of day. Unset or unparsable =
/// the real clock.
pub fn now() -> i64 {
    match crate::env("NOW") {
        Some(v) => v.trim().parse::<i64>().unwrap_or_else(|_| chrono::Utc::now().timestamp()),
        None => chrono::Utc::now().timestamp(),
    }
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
    r#"id, title, tag, description, "column", owner, due, gh_ref, created_at, column_since, blocked, position, reviewer"#;

fn row_card(r: &rusqlite::Row) -> rusqlite::Result<Card> {
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
        position: r.get(11)?,
        reviewer: r.get(12)?,
    })
}

fn row_event(r: &rusqlite::Row) -> rusqlite::Result<Event> {
    Ok(Event {
        card_id: r.get(0)?,
        ts: r.get(1)?,
        actor: r.get(2)?,
        kind: r.get(3)?,
        text: r.get(4)?,
    })
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
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(10))?;
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;")?;
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

    fn log(conn: &Connection, id: i64, actor: &str, kind: &str, text: &str) -> Result<()> {
        conn.execute(
            "INSERT INTO events(card_id, ts, actor, kind, text) VALUES (?,?,?,?,?)",
            params![id, now(), actor, kind, text],
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
            self.conn.execute(
                "INSERT INTO board_events(ts, actor, kind, text) VALUES (?,?,'wip',?)",
                params![now(), actor, format!("wip {old} -> {n}")],
            )?;
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
        Ok(all)
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
        if raw_title.trim().is_empty() {
            return err("title is empty — try 'tb add \"tag: what to do\"'");
        }
        let (tag, gh, title) = parse_title(raw_title);
        let tx = self.conn.unchecked_transaction()?;
        let t = now();
        let pos = bottom_of(&tx, "todo")?;
        tx.execute(
            r#"INSERT INTO cards(title, tag, description, "column", gh_ref, created_at, column_since, position)
               VALUES (?,?,?,'todo',?,?,?,?)"#,
            params![title, tag, desc, gh, t, t, pos],
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
            "SELECT card_id, ts, actor, kind, text FROM events WHERE card_id=? ORDER BY ts, id",
        )?;
        let events = st.query_map([id], row_event)?.collect::<rusqlite::Result<Vec<_>>>()?;
        let round = round_of(&events);
        Ok(CardDetail { card, checklist, events, round })
    }

    /// Events with `id > after`, oldest first (for `tb watch --events`).
    pub fn events_since(&self, after: i64) -> Result<Vec<WatchEvent>> {
        let mut st = self
            .conn
            .prepare("SELECT id, card_id, ts, actor, kind, text FROM events WHERE id > ? ORDER BY id")?;
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
        let mut st = self.conn.prepare(
            "SELECT card_id, ts, actor, kind, text FROM events ORDER BY card_id, ts, id",
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
                r#"SELECT {CARD_COLS} FROM cards WHERE "column"='review' AND blocked IS NULL AND reviewer IS NULL ORDER BY position, id"#
            ))?;
            let v = st.query_map([], row_card)?.collect::<rusqlite::Result<Vec<_>>>()?;
            v
        };
        let mut own = 0;
        let mut target = None;
        for c in &cards {
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
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let wip = wip_of(&tx)?;
        let doing: i64 =
            tx.query_row(r#"SELECT COUNT(*) FROM cards WHERE "column"='doing'"#, [], |r| r.get(0))?;
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
                let found: Option<i64> = tx
                    .query_row(
                        r#"SELECT id FROM cards WHERE "column"='todo' AND blocked IS NULL ORDER BY position, id LIMIT 1"#,
                        [],
                        |r| r.get(0),
                    )
                    .optional()?;
                match found {
                    Some(i) => i,
                    None => {
                        return err("no todo cards — add one with 'tb add \"title\"'")
                    }
                }
            }
        };
        if doing >= wip {
            return Err(wip_full_err(&tx, doing, wip, actor));
        }
        let pos = bottom_of(&tx, "doing")?;
        let changed = tx.execute(
            r#"UPDATE cards SET "column"='doing', owner=?, column_since=?, position=?, reviewer=NULL WHERE id=? AND "column"='todo'"#,
            params![actor, now(), pos, target],
        )?;
        if changed != 1 {
            return err(format!(
                "card #{target} was taken by someone else — try 'tb next'"
            ));
        }
        Self::log(&tx, target, actor, "taken", "")?;
        let card = get_card(&tx, target)?;
        tx.commit()?;
        Ok(card)
    }

    /// Log an event of any kind on a card (e.g. `github` auto-moves).
    pub fn note_kind(&self, id: i64, actor: &str, text: &str, kind: &str) -> Result<()> {
        Self::log(&self.conn, id, actor, kind, text)
    }

    pub fn note(&self, id: i64, text: &str, actor: &str) -> Result<()> {
        self.card(id)?;
        if text.trim().is_empty() {
            return err(format!("note is empty — try 'tb note {id} \"what changed\"'"));
        }
        Self::log(&self.conn, id, actor, "note", text.trim())
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
        self.card(id)?;
        let reason = reason.map(|r| r.trim().trim_start_matches("by ").trim().to_string());
        if reason.as_deref() == Some("") {
            return err(format!("say what blocks it — 'tb block {id} \"#7\"'"));
        }
        let tx = self.conn.unchecked_transaction()?;
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
        let column = column.to_ascii_lowercase();
        if !COLUMNS.contains(&column.as_str()) {
            return err(format!(
                "unknown column '{column}' — use one of todo, doing, review, done: 'tb move {id} doing'"
            ));
        }
        let reason = reason.map(str::trim);
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
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
        // Card ids are small shared integers: an off-by-one must not move someone else's
        // work. Leaving DOING requires the owner (or --force, logged as its own event).
        // The `github` automation is exempt: its moves are evidence-driven and logged.
        if c.column == "doing" && actor != "github" {
            if let Some(owner) = c.owner.as_deref() {
                if !owner.eq_ignore_ascii_case(actor) {
                    if !force {
                        return Err(ownership_err(&tx, id, owner, actor, &column)?);
                    }
                    Self::log(&tx, id, actor, "force", &format!("moved #{id} held by {owner} to {column}"))?;
                }
            }
        }
        if column == "done" && c.column == "review" {
            if let Some(author) = author_of(&tx, &c)? {
                if author.eq_ignore_ascii_case(actor) {
                    if !force {
                        return err("you did this work — ask another person or agent to review it");
                    }
                    Self::log(&tx, id, actor, "force", "approved own work")?;
                }
            }
        }
        // a returned card is its owner's existing work, not new work: WIP does not block it
        if column == "doing" && !send_back {
            let wip = wip_of(&tx)?;
            let doing: i64 = tx.query_row(
                r#"SELECT COUNT(*) FROM cards WHERE "column"='doing'"#,
                [],
                |r| r.get(0),
            )?;
            if doing >= wip {
                return Err(wip_full_err(&tx, doing, wip, actor));
            }
        }
        let owner = match column.as_str() {
            "todo" => None,
            "doing" => Some(c.owner.clone().unwrap_or_else(|| actor.to_string())),
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
        tx.execute(
            r#"UPDATE cards SET "column"=?, owner=?, column_since=?, position=?, reviewer=?,
               blocked = CASE WHEN ?='done' THEN NULL ELSE blocked END WHERE id=?"#,
            params![column, owner, now(), pos, reviewer, column, id],
        )?;
        if block_cleared {
            Self::log(&tx, id, actor, "unblocked", "cleared on done")?;
        }
        Self::log(&tx, id, actor, "moved", &format!("{} -> {column}", c.column))?;
        if let (true, Some(r)) = (send_back, reason) {
            Self::log(&tx, id, actor, "returned", r)?;
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
        tx.execute(
            "INSERT INTO board_events(ts, actor, kind, text) VALUES (?,?,'delete',?)",
            params![now(), actor, format!("deleted #{id} \"{}\"", c.title)],
        )?;
        tx.commit()?;
        Ok(c)
    }

    /// Reorder a card within its column: `top`, `bottom`, `up`, `down`.
    pub fn reorder(&mut self, id: i64, how: &str, actor: &str) -> Result<Card> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let c = get_card(&tx, id)?;
        let mut ids: Vec<i64> = {
            let mut st = tx.prepare(r#"SELECT id FROM cards WHERE "column"=? ORDER BY position, id"#)?;
            let v = st.query_map([&c.column], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
            v
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
            let (tag, gh, title) = parse_title(t);
            tx.execute(
                "UPDATE cards SET title=?, tag=?, gh_ref=? WHERE id=?",
                params![title, tag, gh.or(c.gh_ref), id],
            )?;
            what.push("title");
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
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let c = get_card(&tx, id)?;
        if c.column == "todo" && c.owner.is_none() {
            return Ok(c);
        }
        if c.column == "doing" && actor != "github" {
            if let Some(owner) = c.owner.as_deref() {
                if !owner.eq_ignore_ascii_case(actor) {
                    if !force {
                        return Err(ownership_err(&tx, id, owner, actor, "todo")?);
                    }
                    Self::log(&tx, id, actor, "force", &format!("moved #{id} held by {owner} back to todo"))?;
                }
            }
        }
        let pos = bottom_of(&tx, "todo")?;
        tx.execute(
            r#"UPDATE cards SET "column"='todo', owner=NULL, column_since=?, position=?, reviewer=NULL WHERE id=?"#,
            params![now(), pos, id],
        )?;
        Self::log(&tx, id, actor, "dropped", &format!("{} -> todo", c.column))?;
        let c = get_card(&tx, id)?;
        tx.commit()?;
        Ok(c)
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
    if let Some(t) = &c.tag {
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
