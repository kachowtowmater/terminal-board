//! Who did the work: the structured identity behind an event's short `actor` name.
//!
//! `events.actor` stays the short name people read on the board (`--as`, `TB_AS`, the herdr
//! pane, the login name — `resolve_actor`). Next to it an event may carry an `actor_id` into
//! the `actors` table: which harness, which model, in which role, in which session and on
//! which machine that name was working. A bad batch of work can then be traced back to the
//! session that wrote it, which a nickname reused across runs cannot do.
//!
//! The rules, each of them tested:
//! - **One row per distinct identity.** The key is the WHOLE tuple `(actor, harness, model,
//!   role, session, host)`, NULLs included, and it is defined in exactly one place — the
//!   unique index `actors_identity`. Nothing that changes between two commands of one session
//!   (a time, a process id) is part of it, so a session writes one row however many commands
//!   it runs, and two processes that arrive at once still make one row.
//! - **Nothing is guessed.** `harness` and `session` are read from what the harness exports
//!   (or, inside a herdr pane, from herdr's record of that pane); `role` is only ever what
//!   `TB_ROLE` says, because no harness exports it; `model` is only ever `TB_MODEL`, except for
//!   `pi`, the one harness found to export its own live model name (`$PI_MODEL`). Unknown is NULL.
//! - **No identity, no row.** A person in a plain terminal exports none of this: their events
//!   keep `actor_id` NULL and read exactly as they always did — and their machine's name is
//!   not written into a board file that gets shared.
//! - **Values are data from outside.** Every field is cleaned of control characters and
//!   escape sequences, cut to `FIELD_CAP` characters, and never stored as a path: a board is a
//!   file people copy, and a path carries a home directory.
//! - **It is a claim, not proof.** Like the name itself, the record is self-reported.

use super::{Result, Store};
use crate::herdr::PaneRecord;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// The longest value any identity field may have, in characters.
pub const FIELD_CAP: usize = 64;

/// The table, its key, and the nullable `actor_id` — plus the `ancestry` text column on
/// both event tables, where a write of consequence records the kernel's parent chain
/// (`store::proc`). Idempotent, additive (`CREATE … IF NOT EXISTS`, `ALTER TABLE … ADD
/// COLUMN`), safe inside a transaction: existing rows keep `actor_id` NULL and no ancestry,
/// and nothing is back-filled.
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS actors (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            actor TEXT NOT NULL,
            harness TEXT,
            model TEXT,
            role TEXT,
            session TEXT,
            host TEXT,
            first_seen INTEGER NOT NULL,
            last_seen INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS actors_identity ON actors(
            actor, IFNULL(harness, ''), IFNULL(model, ''), IFNULL(role, ''), IFNULL(session, ''), IFNULL(host, '')
        );",
    )?;
    for table in ["events", "board_events"] {
        let has: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name='actor_id'"),
            [],
            |r| r.get(0),
        )?;
        if has == 0 {
            match conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN actor_id INTEGER REFERENCES actors(id)")) {
                Err(e) if !e.to_string().contains("duplicate column") => return Err(e.into()),
                _ => {}
            }
        }
    }
    for table in ["events", "board_events"] {
        let has: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name='ancestry'"),
            [],
            |r| r.get(0),
        )?;
        if has == 0 {
            match conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN ancestry TEXT")) {
                Err(e) if !e.to_string().contains("duplicate column") => return Err(e.into()),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Everything about a writer except its name. Every field is already clean (`Identity::resolve`
/// is the only constructor that reads the outside world).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub harness: Option<String>,
    pub model: Option<String>,
    pub role: Option<String>,
    pub session: Option<String>,
    pub host: Option<String>,
}

/// Where an identity is read from. The real ones are the environment, herdr and the machine
/// (`Identity::from_env`); tests pass their own.
pub struct Sources<'a> {
    /// An environment variable by its full name.
    pub var: &'a dyn Fn(&str) -> Option<String>,
    /// herdr's record of a pane, by pane id.
    pub pane: &'a dyn Fn(&str) -> Option<PaneRecord>,
    /// This machine's name.
    pub host: &'a dyn Fn() -> Option<String>,
}

impl Identity {
    /// Does it say anything a name does not? (The machine alone does not: see the module doc.)
    pub fn says_something(&self) -> bool {
        self.harness.is_some() || self.model.is_some() || self.role.is_some() || self.session.is_some()
    }

    /// Read the identity. Explicit `TB_*` values win (legacy `TTYBOARD_*` prefix included);
    /// then what the harness exports; then, inside a herdr pane and only for what is still
    /// missing, herdr's record of the pane. The machine is looked up only when there is
    /// something for it to qualify.
    pub fn resolve(src: &Sources) -> Identity {
        let var = |k: &str| (src.var)(k).filter(|v| !v.trim().is_empty());
        let tb = |name: &str| var(&format!("TB_{name}")).or_else(|| var(&format!("TTYBOARD_{name}")));
        // `OMPCODE` before `CLAUDECODE`: omp sets BOTH on its own child processes (`CLAUDECODE`
        // is a compatibility flag omp raises for tools that only know to look for Claude Code,
        // not an omp session id — card #90's fact-finding) — checking `OMPCODE` first is what
        // keeps an omp session from being recorded as `claude-code`. Neither a session id nor a
        // model name is exported by omp today (also #90); when one is, it slots into `session`
        // /`model` the same way `CLAUDE_CODE_SESSION_ID` does.
        let mut harness = tb("HARNESS")
            .or_else(|| var("AI_AGENT").and_then(|v| harness_of(&v)))
            .or_else(|| var("OMPCODE").map(|_| "omp".to_string()))
            // codex exports none of the above to its shell, only its own `CODEX_*` names
            // (`CODEX_SESSION_ID`, `CODEX_THREAD_ID`, `CODEX_SANDBOX`, `CODEX_CI`) — any one of
            // them names the harness. Before `CLAUDECODE`, for the same reason as `OMPCODE`.
            .or_else(|| CODEX_MARKERS.iter().find_map(|k| var(k)).map(|_| "codex".to_string()))
            .or_else(|| var("CLAUDECODE").map(|_| "claude-code".to_string()));
        // pi exports its session id and live model name (`PI_SESSION_ID`, `PI_MODEL`) next to
        // `AI_AGENT=pi`. They are only trusted once the harness is pi: another tool could set
        // the same plain-looking names, and nothing is attributed to pi unless pi is running.
        let is_pi = harness.as_deref() == Some("pi");
        // Under pi, the session is pi's own: pi hands its whole environment to its children, so
        // a pi started from a Claude Code shell still carries `CLAUDE_CODE_SESSION_ID`, and that
        // id belongs to the Claude session, not to this one.
        let exported_session = if is_pi {
            var("PI_SESSION_ID")
        } else if harness.as_deref() == Some("codex") {
            // the same reasoning as pi: only codex's own ids, only once codex is the harness
            var("CODEX_SESSION_ID").or_else(|| var("CODEX_THREAD_ID"))
        } else {
            var("CLAUDE_CODE_SESSION_ID")
        };
        let mut session = tb("SESSION").or(exported_session);
        if harness.is_none() || session.is_none() {
            if let Some(rec) = var("HERDR_PANE_ID").and_then(|p| (src.pane)(p.trim())) {
                harness = harness.or(rec.harness);
                session = session.or(rec.session);
            }
        }
        let model = tb("MODEL").or_else(|| is_pi.then(|| var("PI_MODEL")).flatten());
        let mut who = Identity {
            harness: harness.and_then(|v| field(&v)),
            model: model.and_then(|v| field(&v)),
            role: tb("ROLE").and_then(|v| field(&v)),
            session: session.and_then(|v| session_token(&v)),
            host: None,
        };
        if who.says_something() {
            who.host = tb("HOST").or_else(|| (src.host)()).and_then(|v| field(&v)).map(|h| host_label(&h));
        }
        who
    }

    /// The identity of this process: its environment, herdr, this machine.
    pub fn from_env() -> Identity {
        Identity::resolve(&Sources {
            var: &|k| std::env::var(k).ok(),
            pane: &crate::herdr::pane_record,
            host: &os_hostname,
        })
    }
}

/// The variables codex hands its shell; any one of them means codex is the harness.
const CODEX_MARKERS: [&str; 4] = ["CODEX_SESSION_ID", "CODEX_THREAD_ID", "CODEX_SANDBOX", "CODEX_CI"];

/// The harness named by an `AI_AGENT` value: `claude-code_2-1-278_agent` -> `claude-code`.
/// The version is left out on purpose: the key would otherwise change with every update.
fn harness_of(ai_agent: &str) -> Option<String> {
    let v = ai_agent.trim();
    let v = v.strip_suffix("_agent").unwrap_or(v);
    let name = v.split('_').next().unwrap_or("").trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The first label of a host name (`box.lan` -> `box`): the rest changes with the network a
/// laptop is on, and would split one session over several rows. A numeric address stays whole.
fn host_label(host: &str) -> String {
    let h = host.trim();
    match h.split_once('.') {
        Some((first, _)) if !first.is_empty() && !first.chars().all(|c| c.is_ascii_digit()) => first.to_string(),
        _ => h.to_string(),
    }
}

/// This machine's name, without a new dependency: the kernel's own file where there is one,
/// else the `hostname` command (1s limit, never fatal).
fn os_hostname() -> Option<String> {
    let read = |p: &str| std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    read("/proc/sys/kernel/hostname")
        .or_else(|| crate::herdr::run_quick("hostname", &[]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// Does `s` name a place on a disk? Anchored forms always (`/…`, `~…`, `./…`, `../…`, `C:\…`,
/// anything with a backslash); with `any_separator`, every string with a `/` in it.
fn is_path(s: &str, any_separator: bool) -> bool {
    let b = s.as_bytes();
    let drive = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\');
    drive
        || s.starts_with('/')
        || s.starts_with('~')
        || s.starts_with("./")
        || s.starts_with("../")
        || s.contains('\\')
        || (any_separator && s.contains('/'))
}

/// The last non-empty component of a path, with either separator.
fn last_component(s: &str) -> &str {
    s.rsplit(['/', '\\']).find(|c| !c.trim().is_empty()).unwrap_or("").trim()
}

/// Cut to `FIELD_CAP` characters; empty = unknown.
fn capped(s: &str) -> Option<String> {
    let s: String = s.trim().chars().take(FIELD_CAP).collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// One identity field, clean: no control characters or escape sequences (the display
/// sanitizer), never a path (a value given as one keeps its last component only, so
/// `TB_MODEL=/somewhere/private/model.gguf` stores `model.gguf`), at most `FIELD_CAP`
/// characters. A name with a slash that is not anchored (`org/model`) is not a path.
pub fn field(raw: &str) -> Option<String> {
    let s = crate::text::sanitize(raw);
    let s = s.trim();
    capped(if is_path(s, false) { last_component(s) } else { s })
}

/// The session as stored: an id as given, or — when the harness reports the PATH of a session
/// file, as several do — the identifier inside the file's name, else a short hash of the path
/// (`path-` and 12 hex digits). A path is never stored, whole or in part: its directories
/// spell out a home directory. The hash only tells two sessions apart; it is not a secret.
pub fn session_token(raw: &str) -> Option<String> {
    let s = crate::text::sanitize(raw);
    let s = s.trim();
    if !is_path(s, true) {
        return capped(s);
    }
    match uuid_in(last_component(s)) {
        Some(id) => Some(id),
        None => Some(format!("path-{:012x}", fnv1a(s) & 0xffff_ffff_ffff)),
    }
}

/// The first `8-4-4-4-12` hex identifier inside `s`, lower-cased.
fn uuid_in(s: &str) -> Option<String> {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(35)).find_map(|i| {
        let w = &b[i..i + 36];
        let shaped = w.iter().enumerate().all(|(j, c)| if matches!(j, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() });
        shaped.then(|| String::from_utf8_lossy(w).to_ascii_lowercase())
    })
}

/// FNV-1a, 64 bit: a fixed, documented function, so the same path gives the same token on
/// every machine and in every tb version (a hasher from the standard library promises neither).
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// A row of `actors`, and the JSON object of an identity (docs/JSON.md).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Actor {
    pub id: i64,
    pub actor: String,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub role: Option<String>,
    pub session: Option<String>,
    pub host: Option<String>,
    pub first_seen: i64,
    pub last_seen: i64,
}

impl Actor {
    /// `<name> — <harness> <model> <role> session <id> on <host>`, unknown parts left out.
    pub fn line(&self) -> String {
        let mut parts: Vec<String> = [&self.harness, &self.model, &self.role].into_iter().flatten().cloned().collect();
        if let Some(s) = &self.session {
            parts.push(format!("session {s}"));
        }
        if let Some(h) = &self.host {
            parts.push(format!("on {h}"));
        }
        format!("{} — {}", self.actor, parts.join(" "))
    }
}

const ACTOR_COLS: &str = "id, actor, harness, model, role, session, host, first_seen, last_seen";

fn row_actor(r: &rusqlite::Row) -> rusqlite::Result<Actor> {
    Ok(Actor {
        id: r.get(0)?,
        actor: r.get(1)?,
        harness: r.get(2)?,
        model: r.get(3)?,
        role: r.get(4)?,
        session: r.get(5)?,
        host: r.get(6)?,
        first_seen: r.get(7)?,
        last_seen: r.get(8)?,
    })
}

/// The id of the row for `(actor, who)`, made if it is new. The lookup is written exactly as
/// the unique index is, so "the same identity" means one thing. A row that exists is found
/// and nothing is inserted (the usual case: every command of a session after its first). A
/// new one goes in with `INSERT OR IGNORE`, so when two processes get here at the same moment
/// the index lets one of them win and both read the same id back.
pub fn upsert(conn: &Connection, actor: &str, who: &Identity, ts: i64) -> Result<i64> {
    let find = || {
        conn.query_row(
            "SELECT id FROM actors WHERE actor=? AND IFNULL(harness, '')=IFNULL(?, '') AND IFNULL(model, '')=IFNULL(?, '')
               AND IFNULL(role, '')=IFNULL(?, '') AND IFNULL(session, '')=IFNULL(?, '') AND IFNULL(host, '')=IFNULL(?, '')",
            params![actor, who.harness, who.model, who.role, who.session, who.host],
            |r| r.get::<_, i64>(0),
        )
    };
    let id = match find().optional()? {
        Some(id) => id,
        None => {
            conn.execute(
                "INSERT OR IGNORE INTO actors(actor, harness, model, role, session, host, first_seen, last_seen)
                 VALUES (?,?,?,?,?,?,?,?)",
                params![actor, who.harness, who.model, who.role, who.session, who.host, ts, ts],
            )?;
            find()?
        }
    };
    conn.execute("UPDATE actors SET last_seen=? WHERE id=? AND last_seen<?", params![ts, id, ts])?;
    Ok(id)
}

/// Off until the `tb` binary turns it on: a program that links this crate records an
/// identity only if it asks to.
static FROM_ENVIRONMENT: AtomicBool = AtomicBool::new(false);
static WHO: OnceLock<Identity> = OnceLock::new();

/// Record this process's identity with every event it writes. Reading it (the environment,
/// at most one `herdr agent list`, the machine's name) waits for the first write, so a
/// command that only reads never pays for it.
pub fn use_environment() {
    FROM_ENVIRONMENT.store(true, Ordering::Relaxed);
}

/// This process's identity as the guards see it: exactly what `stamp` records with every
/// event, or nothing at all when identity recording is off (a program linking the crate that
/// never called `use_environment`, which then reads as a person).
pub(super) fn current() -> Identity {
    if !FROM_ENVIRONMENT.load(Ordering::Relaxed) {
        return Identity::default();
    }
    WHO.get_or_init(Identity::from_env).clone()
}

/// The `actor_id` for an event `actor` writes now: the ONE place an identity reaches the
/// board — both event inserts (`Store::log`, `Store::log_board`) call it, and every write goes
/// through those. None when nothing is known beyond the name.
pub(super) fn stamp(conn: &Connection, actor: &str, ts: i64) -> Result<Option<i64>> {
    if !FROM_ENVIRONMENT.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let who = WHO.get_or_init(Identity::from_env);
    if !who.says_something() {
        return Ok(None);
    }
    upsert(conn, actor, who, ts).map(Some)
}

impl Store {
    /// The identities with these ids, in id order (unknown ids are skipped).
    pub fn actors_by_id(&self, ids: &[i64]) -> Result<Vec<Actor>> {
        let mut ids: Vec<i64> = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let mut st = self.conn.prepare(&format!("SELECT {ACTOR_COLS} FROM actors WHERE id=?"))?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(a) = st.query_row([id], row_actor).optional()? {
                out.push(a);
            }
        }
        Ok(out)
    }

    /// One identity (None for an id the board does not have).
    pub fn actor_by_id(&self, id: i64) -> Result<Option<Actor>> {
        Ok(self.actors_by_id(&[id])?.into_iter().next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resolve(vars: &[(&str, &str)], pane: Option<PaneRecord>) -> Identity {
        let map: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Identity::resolve(&Sources {
            var: &|k| map.get(k).cloned(),
            pane: &|p| if p == "w:p2" { pane.clone() } else { None },
            host: &|| Some("box.lan".to_string()),
        })
    }

    const UUID: &str = "0b9f6a52-7c1d-4e0a-9f3b-2a6c1d8e4f70";

    #[test]
    fn nothing_exported_is_nothing_recorded() {
        let who = resolve(&[("USER", "pat"), ("HOME", "/somewhere")], None);
        assert_eq!(who, Identity::default());
        assert!(!who.says_something(), "a plain terminal: no row, and the machine's name is not read");
        // blank values are unset values
        assert_eq!(resolve(&[("TB_MODEL", "  "), ("TB_ROLE", ""), ("CLAUDE_CODE_SESSION_ID", " ")], None), Identity::default());
    }

    #[test]
    fn a_harness_that_exports_its_name_and_session_is_read_from_the_environment() {
        let who = resolve(&[("CLAUDECODE", "1"), ("AI_AGENT", "claude-code_2-1-278_agent"), ("CLAUDE_CODE_SESSION_ID", UUID)], None);
        assert_eq!(who.harness.as_deref(), Some("claude-code"), "the version is not part of the key");
        assert_eq!(who.session.as_deref(), Some(UUID));
        assert_eq!((who.model, who.role), (None, None), "no harness exports these: never guessed");
        assert_eq!(who.host.as_deref(), Some("box"), "first label only");
        // without AI_AGENT the marker variable still names the harness
        assert_eq!(resolve(&[("CLAUDECODE", "1")], None).harness.as_deref(), Some("claude-code"));
        assert_eq!(harness_of("some-tool"), Some("some-tool".into()));
        assert_eq!(harness_of("_agent"), None);
    }

    #[test]
    fn codex_is_read_from_its_own_variables_with_its_session() {
        // what `codex exec` hands its shell: none of AI_AGENT, CLAUDECODE or OMPCODE
        let who = resolve(&[("CODEX_SESSION_ID", UUID), ("CODEX_THREAD_ID", "thread-9"), ("CODEX_SANDBOX", "seatbelt"), ("CODEX_CI", "1")], None);
        assert_eq!((who.harness.as_deref(), who.session.as_deref()), (Some("codex"), Some(UUID)));
        assert_eq!(resolve(&[("CODEX_THREAD_ID", "thread-9")], None).session.as_deref(), Some("thread-9"));
        // a marker alone still names the harness, even with no id to record
        let who = resolve(&[("CODEX_SANDBOX", "seatbelt")], None);
        assert_eq!((who.harness.as_deref(), who.session), (Some("codex"), None));
        // an explicit value still wins, and a Claude Code session id is not codex's
        assert_eq!(resolve(&[("CODEX_CI", "1"), ("TB_HARNESS", "mine")], None).harness.as_deref(), Some("mine"));
        assert_eq!(resolve(&[("CODEX_CI", "1"), ("CLAUDE_CODE_SESSION_ID", UUID)], None).session, None);
    }

    #[test]
    fn omp_is_read_from_ompcode_even_though_omp_also_sets_claudecode() {
        // OMPCODE alone names the harness
        assert_eq!(resolve(&[("OMPCODE", "1")], None).harness.as_deref(), Some("omp"));
        // omp sets CLAUDECODE too (its own compatibility flag, not a Claude Code session) —
        // OMPCODE must still win, or an omp session is recorded as claude-code
        assert_eq!(resolve(&[("OMPCODE", "1"), ("CLAUDECODE", "1")], None).harness.as_deref(), Some("omp"));
        // without OMPCODE, CLAUDECODE is unchanged
        assert_eq!(resolve(&[("CLAUDECODE", "1")], None).harness.as_deref(), Some("claude-code"));
    }

    #[test]
    fn pi_session_and_model_are_read_from_what_pi_exports() {
        let who = resolve(&[("AI_AGENT", "pi"), ("PI_SESSION_ID", UUID), ("PI_MODEL", "model-y")], None);
        assert_eq!(who.harness.as_deref(), Some("pi"));
        assert_eq!(who.session.as_deref(), Some(UUID));
        assert_eq!(who.model.as_deref(), Some("model-y"));
    }

    #[test]
    fn explicit_tb_session_and_model_win_over_what_pi_exports() {
        let who = resolve(
            &[
                ("AI_AGENT", "pi"),
                ("PI_SESSION_ID", UUID),
                ("PI_MODEL", "model-y"),
                ("TB_SESSION", "run-7"),
                ("TB_MODEL", "model-x"),
            ],
            None,
        );
        assert_eq!(who.harness.as_deref(), Some("pi"));
        assert_eq!(who.session.as_deref(), Some("run-7"));
        assert_eq!(who.model.as_deref(), Some("model-x"));
    }

    #[test]
    fn a_pi_started_from_a_claude_code_shell_records_pis_session_not_claudes() {
        let inherited = [
            ("AI_AGENT", "pi"),
            ("PI_SESSION_ID", UUID),
            ("PI_MODEL", "model-y"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", "cc-session-1"),
        ];
        let who = resolve(&inherited, None);
        assert_eq!(who.harness.as_deref(), Some("pi"));
        assert_eq!(who.session.as_deref(), Some(UUID), "Claude's session id is not pi's");
        assert_eq!(who.model.as_deref(), Some("model-y"));
        // without pi's own id, Claude's is still not borrowed
        let who = resolve(&inherited[..1].iter().chain(&inherited[3..]).copied().collect::<Vec<_>>(), None);
        assert_eq!(who.session, None, "Claude's session id is not pi's");
        // TB_SESSION still wins over both
        let mut explicit = inherited.to_vec();
        explicit.push(("TB_SESSION", "run-7"));
        assert_eq!(resolve(&explicit, None).session.as_deref(), Some("run-7"));
    }

    #[test]
    fn a_harness_that_is_not_pi_never_takes_a_stray_pi_export() {
        for harness in [("CLAUDECODE", "1"), ("OMPCODE", "1"), ("TB_HARNESS", "codex")] {
            let who = resolve(&[harness, ("PI_MODEL", "model-y"), ("PI_SESSION_ID", UUID)], None);
            assert_eq!(who.model, None, "{harness:?}: PI_MODEL is pi's own");
            assert_eq!(who.session, None, "{harness:?}: PI_SESSION_ID is pi's own");
        }
        // no harness at all: still nothing guessed
        let who = resolve(&[("PI_MODEL", "model-y"), ("PI_SESSION_ID", UUID)], None);
        assert_eq!((who.harness, who.model, who.session), (None, None, None));
    }

    #[test]
    fn model_and_role_are_explicit_and_explicit_values_win() {
        let who = resolve(
            &[("TB_MODEL", "model-x"), ("TB_ROLE", "reviewer"), ("TB_HARNESS", "mine"), ("TB_SESSION", "run-7"), ("TB_HOST", "lab.example.com"),
              ("AI_AGENT", "claude-code_1_agent"), ("CLAUDE_CODE_SESSION_ID", UUID)],
            None,
        );
        assert_eq!(
            who,
            Identity { harness: Some("mine".into()), model: Some("model-x".into()), role: Some("reviewer".into()), session: Some("run-7".into()), host: Some("lab".into()) }
        );
        // the pre-rename prefix still works, and the new one wins
        assert_eq!(resolve(&[("TTYBOARD_ROLE", "old")], None).role.as_deref(), Some("old"));
        assert_eq!(resolve(&[("TTYBOARD_ROLE", "old"), ("TB_ROLE", "new")], None).role.as_deref(), Some("new"));
    }

    #[test]
    fn a_herdr_pane_fills_only_what_the_environment_left_out() {
        let pane = PaneRecord { harness: Some("aider".into()), session: Some(format!("/srv/agent/sessions/-work-/2026-01-02T03-04-05Z_{UUID}.jsonl")) };
        let who = resolve(&[("HERDR_PANE_ID", "w:p2")], Some(pane.clone()));
        assert_eq!((who.harness.as_deref(), who.session.as_deref()), (Some("aider"), Some(UUID)), "the id inside the file name, never the path");
        // the environment wins, herdr fills the gap
        let who = resolve(&[("HERDR_PANE_ID", "w:p2"), ("CLAUDECODE", "1")], Some(pane.clone()));
        assert_eq!((who.harness.as_deref(), who.session.as_deref()), (Some("claude-code"), Some(UUID)));
        // another pane, or no pane: herdr says nothing
        assert_eq!(resolve(&[("HERDR_PANE_ID", "w:p9")], Some(pane.clone())), Identity::default());
        assert_eq!(resolve(&[], Some(pane)), Identity::default());
    }

    #[test]
    fn herdr_is_not_asked_when_the_environment_already_answers() {
        let asked = std::cell::Cell::new(0);
        let vars: HashMap<&str, &str> =
            [("HERDR_PANE_ID", "w:p2"), ("AI_AGENT", "claude-code_1_agent"), ("CLAUDE_CODE_SESSION_ID", UUID)].into_iter().collect();
        Identity::resolve(&Sources {
            var: &|k| vars.get(k).map(|v| v.to_string()),
            pane: &|_| {
                asked.set(asked.get() + 1);
                None
            },
            host: &|| None,
        });
        assert_eq!(asked.get(), 0);
    }

    #[test]
    fn a_session_is_never_stored_as_a_path() {
        assert_eq!(session_token(UUID).as_deref(), Some(UUID));
        assert_eq!(session_token("run-7").as_deref(), Some("run-7"));
        let with_id = format!("/srv/people/pat/.agent/sessions/--srv-people-pat--/2026-01-02T03-04-05-678Z_{}.jsonl", UUID.to_ascii_uppercase());
        assert_eq!(session_token(&with_id).as_deref(), Some(UUID), "the identifier in the file name, lower-cased");
        for p in ["/srv/people/pat/notes/today.log", "~/sessions/today", "sessions/today.jsonl", "C:\\people\\pat\\s.jsonl", "..\\s"] {
            let t = session_token(p).unwrap();
            assert!(t.starts_with("path-") && t.len() == 17 && t[5..].chars().all(|c| c.is_ascii_hexdigit()), "{p} -> {t}");
            assert!(!t.contains("people") && !t.contains('/') && !t.contains('\\'), "{p} -> {t}");
            assert_eq!(session_token(p).unwrap(), t, "the same path is the same session");
        }
        assert_ne!(session_token("/a/one.log"), session_token("/a/two.log"));
        // the token is a fixed function of the path: pinned, so an upgrade never splits a session
        assert_eq!(session_token("/a/one.log").as_deref(), Some(format!("path-{:012x}", fnv1a("/a/one.log") & 0xffff_ffff_ffff).as_str()));
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn fields_are_cleaned_capped_and_never_a_path() {
        assert_eq!(field("model\x1b[31m-x\x07\u{9b}2J").as_deref(), Some("model-x"));
        assert_eq!(field("two\nlines\tand a tab").as_deref(), Some("two lines and a tab"));
        assert_eq!(field(" \x1b]0;only a sequence\x07 "), None, "nothing left = unknown");
        assert_eq!(field(&"x".repeat(500)).map(|s| s.chars().count()), Some(FIELD_CAP));
        assert_eq!(field(&"é".repeat(500)).map(|s| s.chars().count()), Some(FIELD_CAP), "characters, not bytes");
        assert_eq!(session_token(&"s".repeat(500)).map(|s| s.len()), Some(FIELD_CAP));
        assert_eq!(field("/srv/people/pat/models/model-x.gguf").as_deref(), Some("model-x.gguf"));
        assert_eq!(field("~/models/model-x.gguf").as_deref(), Some("model-x.gguf"));
        assert_eq!(field("C:\\people\\pat\\model-x.gguf").as_deref(), Some("model-x.gguf"));
        assert_eq!(field("/srv/people/pat/").as_deref(), Some("pat"), "a trailing separator is not a component");
        assert_eq!(field("org/model-x").as_deref(), Some("org/model-x"), "a name with a slash is not a path");
        assert_eq!(host_label("box.lan"), "box");
        assert_eq!(host_label("box"), "box");
        assert_eq!(host_label("127.0.0.1"), "127.0.0.1", "an address is not cut at its first dot");
    }

    fn board() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate_fresh(&conn);
        conn
    }

    fn migrate_fresh(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, actor TEXT);
             CREATE TABLE IF NOT EXISTS board_events (id INTEGER PRIMARY KEY, actor TEXT);",
        )
        .unwrap();
        migrate(conn).unwrap();
    }

    fn rows(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM actors", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn the_key_is_the_whole_tuple_nulls_included() {
        let conn = board();
        let who = Identity { harness: Some("h".into()), session: Some("s".into()), ..Default::default() };
        let id = upsert(&conn, "bot", &who, 100).unwrap();
        for ts in 101..600 {
            assert_eq!(upsert(&conn, "bot", &who, ts).unwrap(), id);
        }
        assert_eq!(rows(&conn), 1, "one identity, one row — NULL fields do not make a new one");
        let (first, last): (i64, i64) = conn.query_row("SELECT first_seen, last_seen FROM actors", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((first, last), (100, 599));
        upsert(&conn, "bot", &who, 50).unwrap();
        assert_eq!(conn.query_row("SELECT last_seen FROM actors", [], |r| r.get::<_, i64>(0)).unwrap(), 599, "a clock that steps back never rewinds it");
        // every field is part of the key, and so is the name
        let mut ids = vec![id];
        ids.push(upsert(&conn, "other", &who, 1).unwrap());
        for change in [
            Identity { harness: Some("h2".into()), ..who.clone() },
            Identity { model: Some("m".into()), ..who.clone() },
            Identity { role: Some("r".into()), ..who.clone() },
            Identity { session: Some("s2".into()), ..who.clone() },
            Identity { session: None, ..who.clone() },
            Identity { host: Some("box".into()), ..who.clone() },
        ] {
            ids.push(upsert(&conn, "bot", &change, 1).unwrap());
            assert_eq!(upsert(&conn, "bot", &change, 2).unwrap(), *ids.last().unwrap());
        }
        ids.sort_unstable();
        ids.dedup();
        assert_eq!((ids.len() as i64, rows(&conn)), (8, 8));
        assert_eq!(ids, (1..=8).collect::<Vec<i64>>(), "finding a row never uses up an id");
    }

    #[test]
    fn the_migration_can_run_again_and_inside_a_transaction() {
        let conn = board();
        migrate(&conn).unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        migrate(&tx).unwrap();
        tx.rollback().unwrap();
        for t in ["events", "board_events"] {
            let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('{t}') WHERE name='actor_id'"), [], |r| r.get(0)).unwrap();
            assert_eq!(n, 1, "{t}");
            let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('{t}') WHERE name='ancestry'"), [], |r| r.get(0)).unwrap();
            assert_eq!(n, 1, "{t} ancestry column");
        }
    }

    #[test]
    fn the_line_leaves_out_what_is_unknown() {
        let mut a = Actor {
            id: 1,
            actor: "lead".into(),
            harness: Some("claude-code".into()),
            model: Some("model-x".into()),
            role: Some("orchestrator".into()),
            session: Some(UUID.into()),
            host: Some("box".into()),
            first_seen: 0,
            last_seen: 0,
        };
        assert_eq!(a.line(), format!("lead — claude-code model-x orchestrator session {UUID} on box"));
        a.model = None;
        a.session = None;
        a.host = None;
        assert_eq!(a.line(), "lead — claude-code orchestrator");
    }
}
