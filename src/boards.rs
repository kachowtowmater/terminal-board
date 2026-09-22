//! Named boards: `~/.local/state/ttyboard/boards/<name>.db`, legacy migration, selection.

use crate::store::{BoardError, Result, Store, COLUMNS};
use std::path::{Path, PathBuf};

pub const DEFAULT_BOARD: &str = "default";

/// Subcommand names: never valid board names.
pub const COMMANDS: [&str; 26] = [
    "add", "list", "show", "next", "take", "note", "check", "move", "done", "block", "drop",
    "config", "boards", "github", "help", "rm", "prio", "edit", "sync", "board", "watch",
    "agents", "guide", "setup", "restore", "import",
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

/// The board a command acts on: `select`, with the saved default board between `TB_BOARD` and
/// `default`. The settings are read only when nothing above them decides, so a settings file
/// that cannot be used never blocks a command that names its board (and never one under
/// `TB_DB`). A saved default whose board is gone is refused — never re-created as an empty
/// phantom, never silently swapped for `default`.
pub fn resolve(positional: Option<&str>, flag: Option<&str>, env: Option<&str>) -> Result<String> {
    let ambient = env.filter(|e| !e.trim().is_empty());
    if positional.is_none() && flag.is_none() && ambient.is_none() {
        if let Some(saved) = saved_default_for_read()? {
            if !path_for(&saved).exists() {
                let names = list();
                let all = if names.is_empty() { "none yet".to_string() } else { names.join(", ") };
                return Err(BoardError(format!(
                    "the saved default board is '{saved}', but there is no board '{saved}' — boards: {all} · choose another with 'tb boards --default NAME' or go back with 'tb boards --default --clear'"
                )));
            }
            return Ok(saved);
        }
    }
    select(positional, flag, env)
}

/// The board that bare `tb` uses: `TB_BOARD`, else the saved default board, else `default`.
/// (A settings file that cannot be read counts as "nothing saved" HERE — this only marks a row;
/// the command that actually opens a board refuses instead, see `saved_default`.)
pub fn default_name() -> String {
    env_board()
        .0
        .or_else(|| saved_default_for_read().ok().flatten())
        .unwrap_or_else(|| DEFAULT_BOARD.into())
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

/// The key of the saved default board in the machine-local settings (`crate::machine`).
pub const DEFAULT_BOARD_KEY: &str = "default_board";

/// The saved default board — the board plain `tb` opens when neither a name on the command
/// line nor `TB_BOARD` says otherwise — or `None` when nothing is saved. It is a per-user
/// choice, so it lives in the machine-local settings, never in a board file. `TB_DB` pins one
/// file: there is nothing to choose, and the settings are not even read.
/// Errors: the settings file cannot be used, or holds something that is not a board name.
pub fn saved_default() -> Result<Option<String>> {
    saved_from(crate::machine::load()?)
}

/// The saved default board for a command that did NOT ask about it: a settings file tb cannot
/// read is "nothing is saved" (said once on stderr), so a machine that never saved anything
/// behaves exactly as it did before this file existed. A file that reads fine but is not a
/// settings object is still an error — the setting may be in there, and opening the wrong
/// board without a word is worse than refusing.
pub fn saved_default_for_read() -> Result<Option<String>> {
    if db_pinned() {
        return Ok(None);
    }
    saved_from(crate::machine::load_for_read()?)
}

fn saved_from(settings: serde_json::Map<String, serde_json::Value>) -> Result<Option<String>> {
    if db_pinned() {
        return Ok(None);
    }
    let fix = "set it again with 'tb boards --default NAME' or go back with 'tb boards --default --clear'";
    match settings.get(DEFAULT_BOARD_KEY) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(name)) if validate(name).is_ok() => Ok(Some(name.clone())),
        Some(other) => Err(BoardError(format!(
            "the saved default board in {} is {other}, which is not a board name — {fix}",
            crate::machine::path().display()
        ))),
    }
}

/// Why plain `tb` opens the board it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultSource {
    /// `TB_DB` pins one file: no boards to choose from.
    Pinned,
    /// `TB_BOARD` in this environment.
    Env,
    /// The saved default board.
    Setting,
    /// Nothing set: the built-in `default`.
    Builtin,
}

impl DefaultSource {
    pub fn as_str(self) -> &'static str {
        match self {
            DefaultSource::Pinned => "TB_DB",
            DefaultSource::Env => "TB_BOARD",
            DefaultSource::Setting => "setting",
            DefaultSource::Builtin => "builtin",
        }
    }
}

/// The board plain `tb` opens here, and why. THE precedence, stated once:
/// **`TB_DB` > `TB_BOARD` > the saved default board > `default`** (a board named on the
/// command line — `tb NAME …`, `-b NAME` — beats all of them, and is refused under `TB_DB`).
pub fn plain_board() -> Result<(String, DefaultSource)> {
    if db_pinned() {
        return Ok((DEFAULT_BOARD.into(), DefaultSource::Pinned));
    }
    // through env_board(), so the TB_DB rule lives in exactly one place
    if let Some(name) = env_board().0 {
        return Ok((name, DefaultSource::Env));
    }
    Ok(match saved_default()? {
        Some(name) => (name, DefaultSource::Setting),
        None => (DEFAULT_BOARD.into(), DefaultSource::Builtin),
    })
}

/// Save `name` as the default board (`None`, or the built-in `default`, clears it). Refused
/// for a board that does not exist — an archived board is not in the boards directory, so it
/// is refused the same way — and under `TB_DB`, where there is nothing to choose.
pub fn set_default(name: Option<&str>) -> Result<()> {
    if db_pinned() {
        return Err(BoardError(
            "TB_DB pins one board file, so there is no default board to choose — unset TB_DB, then 'tb boards --default NAME'".into(),
        ));
    }
    let name = name.map(str::trim).filter(|n| *n != DEFAULT_BOARD);
    if let Some(n) = name {
        validate(n)?;
        let names = list();
        if !names.iter().any(|b| b == n) {
            let all = if names.is_empty() { "none yet".to_string() } else { names.join(", ") };
            return Err(BoardError(format!(
                "no board '{n}' — boards: {all} · choose one that exists: 'tb boards --default NAME' (create it first with 'tb {n} add \"…\"')"
            )));
        }
    }
    crate::machine::update(|m| match name {
        Some(n) => {
            m.insert(DEFAULT_BOARD_KEY.into(), serde_json::Value::String(n.to_string()));
        }
        None => {
            m.remove(DEFAULT_BOARD_KEY);
        }
    })
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
    fn default_source_names() {
        let all = [DefaultSource::Pinned, DefaultSource::Env, DefaultSource::Setting, DefaultSource::Builtin];
        assert_eq!(all.map(DefaultSource::as_str), ["TB_DB", "TB_BOARD", "setting", "builtin"]);
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
