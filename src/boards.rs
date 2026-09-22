//! Named boards: `~/.local/state/ttyboard/boards/<name>.db`, legacy migration, selection.

use crate::store::{BoardError, Result, Store, COLUMNS};
use std::path::{Path, PathBuf};

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
    env_board().0.unwrap_or_else(|| DEFAULT_BOARD.into())
}

/// `TB_BOARD` — unless `TB_DB` pins one file. A pinned file has no boards to choose from, so
/// `TB_DB` wins and the name is dropped: `(None, Some(name))`, for the caller to say so once.
/// (`TB_BOARD=default` names the board a pinned file already is: nothing dropped, nothing to
/// say.) A name TYPED on the command line is a different matter and is still refused.
pub fn env_board() -> (Option<String>, Option<String>) {
    let name = crate::env("BOARD");
    if !db_pinned() {
        return (name, None);
    }
    (None, name.filter(|n| n.trim() != DEFAULT_BOARD))
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
