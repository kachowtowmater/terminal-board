//! Named boards: `~/.local/state/ttyboard/boards/<name>.db`, legacy migration, selection.

use crate::store::{BoardError, Result, Store, COLUMNS};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_BOARD: &str = "default";

/// Subcommand names: never valid board names.
pub const COMMANDS: [&str; 24] = [
    "add", "list", "show", "next", "take", "note", "check", "move", "done", "block", "drop",
    "config", "boards", "github", "help", "rm", "prio", "edit", "sync", "board", "watch",
    "agents", "guide", "setup",
];

/// `[a-z0-9_-]{1,32}` and not a subcommand.
pub fn validate(name: &str) -> Result<()> {
    if COMMANDS.contains(&name) {
        return Err(BoardError(format!(
            "'{name}' is a command, so it can't be a board name — pick another, e.g. 'tb -b {name}s' or 'tb home'"
        )));
    }
    let ok = (1..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !ok {
        return Err(BoardError(format!(
            "'{name}' is not a command or a valid board name (a-z 0-9 _ -, up to 32) — see 'tb --help' or try 'tb home'"
        )));
    }
    Ok(())
}

/// Precedence: positional > `-b/--board` > `TTYBOARD_BOARD` > `default`.
pub fn select(positional: Option<&str>, flag: Option<&str>, env: Option<&str>) -> Result<String> {
    let name = positional
        .or(flag)
        .or(env.filter(|e| !e.trim().is_empty()))
        .unwrap_or(DEFAULT_BOARD)
        .trim()
        .to_string();
    validate(&name)?;
    Ok(name)
}

/// The board that bare `ttyboard` uses: `TTYBOARD_BOARD` or `default`.
pub fn default_name() -> String {
    crate::env("BOARD")
        .unwrap_or_else(|| DEFAULT_BOARD.into())
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// `~/.local/state/terminal-board`
pub fn state_dir() -> PathBuf {
    home().join(".local/state/terminal-board")
}

/// Where boards lived before the rename: `~/.local/state/ttyboard`.
pub fn old_state_dir() -> PathBuf {
    home().join(".local/state/ttyboard")
}

pub fn boards_dir() -> PathBuf {
    state_dir().join("boards")
}

/// Where `tb boards archive` puts a retired board: `~/.local/state/terminal-board/archive`.
/// Nothing in tb deletes a board — an archived board is a file the user can restore or
/// remove themselves.
pub fn archive_dir() -> PathBuf {
    state_dir().join("archive")
}

/// DB file for a board. `TB_DB` (or the old `TTYBOARD_DB`) overrides everything.
pub fn path_for(name: &str) -> PathBuf {
    match crate::env("DB") {
        Some(p) => PathBuf::from(p),
        _ => boards_dir().join(format!("{name}.db")),
    }
}

fn move_with_sidecars(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)?;
    for ext in ["-wal", "-shm"] {
        let f = PathBuf::from(format!("{}{ext}", from.display()));
        if f.exists() {
            std::fs::rename(&f, format!("{}{ext}", to.display()))?;
        }
    }
    Ok(())
}

/// An archived board file: `archive/<name>@<stamp>.db`. `@` is not a legal board-name
/// character, so the split back into (name, stamp) is unambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveRow {
    pub name: String,
    /// `YYYYmmdd-HHMMSS`, when it was archived.
    pub stamp: String,
    pub path: PathBuf,
    /// Card counts in column order, or `None` when the file cannot be read.
    pub counts: Option<[usize; 4]>,
}

impl ArchiveRow {
    /// The stamp as `2026-09-21 10:15`, or the raw stamp if it is not the expected shape.
    pub fn archived_at(&self) -> String {
        let s = &self.stamp;
        if s.len() == 15 && s.is_char_boundary(8) && s.as_bytes()[8] == b'-' {
            return format!("{}-{}-{} {}:{}", &s[0..4], &s[4..6], &s[6..8], &s[9..11], &s[11..13]);
        }
        s.clone()
    }
}

/// Card counts of a board file, read WITHOUT writing to it: an archived board must come back
/// byte-for-byte, and the normal open runs migrations. `None` when it cannot be read.
fn counts_read_only(path: &Path) -> Option<[usize; 4]> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let mut st = conn.prepare(r#"SELECT "column", COUNT(*) FROM cards GROUP BY "column""#).ok()?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).ok()?;
    let mut counts = [0usize; 4];
    for (col, n) in rows.flatten() {
        if let Some(i) = COLUMNS.iter().position(|c| *c == col) {
            counts[i] = n.max(0) as usize;
        }
    }
    Some(counts)
}

/// Every archived board, oldest first within a name (so the last one for a name is newest).
pub fn archived() -> Vec<ArchiveRow> {
    let mut v: Vec<ArchiveRow> = std::fs::read_dir(archive_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let f = e.file_name().to_string_lossy().to_string();
                    let stem = f.strip_suffix(".db")?;
                    let (name, stamp) = stem.rsplit_once('@')?;
                    if validate(name).is_err() {
                        return None;
                    }
                    let path = e.path();
                    let counts = counts_read_only(&path);
                    Some(ArchiveRow { name: name.to_string(), stamp: stamp.to_string(), path, counts })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| (&a.name, &a.stamp).cmp(&(&b.name, &b.stamp)));
    v
}

/// Refused because another process has the board's SQLite file open.
struct Busy;

/// Take the board's file exclusively, or report that someone else holds it.
///
/// The sidecar files (`-wal`/`-shm`) are NOT a usable busy signal: they survive a crash, so
/// testing for them refuses forever after one. The only honest test is taking the lock —
/// `busy_timeout` 0 (the store itself waits 10s, which would hide the conflict),
/// `locking_mode=EXCLUSIVE` so the lock covers the whole connection rather than one
/// statement, then `BEGIN IMMEDIATE`. SQLITE_BUSY means another connection (a running `tb`
/// window keeps one open) has it. The returned connection keeps the lock until it is
/// dropped, so the caller can move the file with nobody able to open it.
fn lock_exclusive(path: &Path) -> Result<std::result::Result<Connection, Busy>> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(0))?;
    let _mode: String = conn.query_row("PRAGMA locking_mode=EXCLUSIVE", [], |r| r.get(0))?;
    match conn.execute_batch("BEGIN IMMEDIATE") {
        Ok(()) => {
            conn.execute_batch("ROLLBACK")?;
            Ok(Ok(conn))
        }
        Err(e) if is_busy(&e) => Ok(Err(Busy)),
        Err(e) => Err(e.into()),
    }
}

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _)
        if matches!(f.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
}

/// With `TB_DB` set there is one pinned file and no boards directory, so there is nothing to
/// move: the same refusal `tb boards` and the picker already give.
fn not_pinned(verb: &str) -> Result<()> {
    if db_pinned() {
        return Err(BoardError(format!("TB_DB pins one board file — unset TB_DB to {verb} a board")));
    }
    Ok(())
}

/// Move a board out of the boards directory into `archive/<name>@<stamp>.db`, holding the
/// board's SQLite lock across the move. Returns where it went. Nothing is deleted.
pub fn archive(name: &str) -> Result<PathBuf> {
    validate(name)?;
    not_pinned("archive")?;
    // there is no session state in the CLI: the board you are "in" is the one a bare `tb`
    // opens, which is TB_BOARD or `default`. Moving that one out from under it is the one
    // archive nobody can mean.
    if name == default_name() {
        return Err(BoardError(format!(
            "'{name}' is the board a bare 'tb' opens — archive another board, or point TB_BOARD at a different one first"
        )));
    }
    let src = boards_dir().join(format!("{name}.db"));
    if !src.is_file() {
        let names = list();
        let all = if names.is_empty() { "none yet".to_string() } else { names.join(", ") };
        return Err(BoardError(format!("no board '{name}' — boards: {all} · see 'tb boards'")));
    }
    let conn = match lock_exclusive(&src)? {
        Ok(c) => c,
        Err(Busy) => {
            return Err(BoardError(format!(
                "board '{name}' is open in another process — close it (quit any 'tb {name}' window or 'tb {name} watch') and archive again"
            )))
        }
    };
    let dir = archive_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| BoardError(format!("cannot create {}: {e} — check the state directory is writable", dir.display())))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let dst = dir.join(format!("{name}@{stamp}.db"));
    if dst.exists() {
        return Err(BoardError(format!("{} already exists — wait a second and archive again", dst.display())));
    }
    // fold the WAL back into the file, so what moves is the whole board
    let _: std::result::Result<i64, _> = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0));
    std::fs::rename(&src, &dst)
        .map_err(|e| BoardError(format!("cannot move {} to {}: {e} — check the state directory is writable", src.display(), dst.display())))?;
    drop(conn); // releases the lock; SQLite drops the now-empty sidecars
    for ext in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", src.display()));
    }
    Ok(dst)
}

/// Move the newest archive of `name` back into the boards directory. Returns (from, to).
pub fn restore(name: &str) -> Result<(PathBuf, PathBuf)> {
    validate(name)?;
    // restoring the default board is fine — there is no live file to pull away
    not_pinned("restore")?;
    let dst = boards_dir().join(format!("{name}.db"));
    if dst.exists() {
        return Err(BoardError(format!(
            "board '{name}' already exists — archive or move {} aside before restoring",
            dst.display()
        )));
    }
    let all = archived();
    let src = all.iter().rfind(|a| a.name == name).ok_or_else(|| {
        let mut names: Vec<&str> = all.iter().map(|a| a.name.as_str()).collect();
        names.dedup();
        let have = if names.is_empty() { "none".to_string() } else { names.join(", ") };
        BoardError(format!("no archived board '{name}' — archived: {have} · see 'tb boards --archived'"))
    })?;
    std::fs::create_dir_all(boards_dir())
        .map_err(|e| BoardError(format!("cannot create {}: {e} — check the state directory is writable", boards_dir().display())))?;
    move_with_sidecars(&src.path, &dst)
        .map_err(|e| BoardError(format!("cannot move {} to {}: {e} — check the state directory is writable", src.path.display(), dst.display())))?;
    Ok((src.path.clone(), dst))
}

/// Bring old data into `new` (the terminal-board state dir), returning one-line notes:
/// 1. `old/boards/*` (the ttyboard era) moves over when `new` does not exist yet;
/// 2. single-board files `board.db` -> `boards/default.db`, `demo.db` -> `boards/demo.db`,
///    found in `old` or `new`, never overwriting an existing board.
pub fn migrate(old: &Path, new: &Path) -> std::io::Result<Vec<String>> {
    let mut notes = Vec::new();
    let new_boards = new.join("boards");
    if old.join("boards").is_dir() && !new.exists() {
        std::fs::create_dir_all(&new_boards)?;
        let mut n = 0;
        for e in std::fs::read_dir(old.join("boards"))? {
            let e = e?;
            std::fs::rename(e.path(), new_boards.join(e.file_name()))?;
            if e.file_name().to_string_lossy().ends_with(".db") {
                n += 1;
            }
        }
        notes.push(format!("moved {n} board(s) from {} to {}", old.join("boards").display(), new_boards.display()));
    }
    for dir in [old, new] {
        for (from, to) in [("board.db", "default.db"), ("demo.db", "demo.db")] {
            let src = dir.join(from);
            let dst = new_boards.join(to);
            if !src.is_file() || dst.exists() {
                continue;
            }
            std::fs::create_dir_all(&new_boards)?;
            move_with_sidecars(&src, &dst)?;
            notes.push(format!("moved legacy {} into {}", src.display(), dst.display()));
        }
    }
    Ok(notes)
}

/// Board names that exist on disk, sorted.
pub fn list() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(boards_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.strip_suffix(".db").map(str::to_string)
                })
                .filter(|n| validate(n).is_ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// One row of `tb boards`: the board, whether it is the default one, its card counts in
/// column order (todo, doing, review, done) and the file it lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardRow {
    pub name: String,
    pub is_default: bool,
    pub counts: [usize; 4],
    pub path: PathBuf,
}

/// Is `TB_DB` pinning one file? Then there is no boards directory to list: every name would
/// open the same file, which is why the CLI refuses board names in this mode.
pub fn db_pinned() -> bool {
    crate::env("DB").is_some()
}

/// The rows `tb boards` prints: every board on disk, counted. `TB_DB` pins one file, so it
/// reports the one board it is.
pub fn rows() -> Result<Vec<BoardRow>> {
    let def = default_name();
    let names = if db_pinned() { vec![def.clone()] } else { list() };
    names
        .iter()
        .map(|n| {
            let path = path_for(n);
            let snap = Store::open(&path)?.named(n).snapshot()?;
            let mut counts = [0usize; 4];
            for (i, c) in COLUMNS.iter().enumerate() {
                counts[i] = snap.in_column(c).len();
            }
            Ok(BoardRow { name: n.clone(), is_default: *n == def, counts, path })
        })
        .collect()
}

/// The board picker's rows, or the reason it cannot offer a choice. With `TB_DB` set there
/// is exactly one file and board names are refused (`open_board`), so switching is off.
pub fn picker_rows() -> std::result::Result<Vec<BoardRow>, String> {
    if db_pinned() {
        return Err("TB_DB pins one board file — unset TB_DB to switch boards".into());
    }
    rows().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(validate("home").is_ok());
        assert!(validate("work-2_x").is_ok());
        assert!(validate(&"a".repeat(32)).is_ok());
        assert!(validate(&"a".repeat(33)).is_err());
        assert!(validate("Home").is_err());
        assert!(validate("a b").is_err());
        assert!(validate("").is_err());
        let e = validate("add").unwrap_err().to_string();
        assert!(e.contains("is a command") && e.contains("tb "), "{e}");
    }

    #[test]
    fn precedence() {
        assert_eq!(select(Some("home"), Some("work"), Some("env")).unwrap(), "home");
        assert_eq!(select(None, Some("work"), Some("env")).unwrap(), "work");
        assert_eq!(select(None, None, Some("env")).unwrap(), "env");
        assert_eq!(select(None, None, Some("")).unwrap(), "default");
        assert_eq!(select(None, None, None).unwrap(), "default");
        assert!(select(None, Some("next"), None).is_err());
        assert!(select(None, None, Some("list")).is_err());
    }
}
