//! Machine-local settings: `~/.config/terminal-board/config.json` (or the file `TB_CONFIG` names).
//!
//! A board is a file people copy, so anything that belongs to a PERSON on a MACHINE — which
//! board plain `tb` opens, and later which commands this machine trusts — lives here, never
//! inside a board file, and never next to the boards either (it is not `install.conf`, the
//! installer's own record, which `install.sh --uninstall` deletes while keeping boards).
//!
//! One JSON object, one top-level key per owner (`default_board`, …). The API is four
//! functions over a generic map, so two features never edit one type:
//! - [`path`] — where the file is;
//! - [`load`] — the whole object, every problem an error. For asking about a setting;
//! - [`load_for_read`] — the same, but a file that cannot be READ (no permission, a path that
//!   is not a directory) is "nothing is set", warned once on stderr, so a machine that never
//!   saved anything works exactly as it did before this file existed. A file that IS readable
//!   and holds something that is not a settings object is still an error on every path: tb
//!   would otherwise open the wrong board without a word;
//! - [`update`] — read, change, write back, under a lock (below). Keys the caller does not
//!   touch keep their exact text, down to the digits of a number.
//!
//! **One writer at a time.** `update` holds `crate::lock`'s advisory EXCLUSIVE lock on a
//! sibling `.lock` file across read → change → write → rename, and RE-READS the file inside
//! the lock: what a caller changed is applied to whatever the file says at that moment, so a
//! write by someone else between the two is kept, not reverted. This matters beyond tidiness —
//! a trust store lives here, and a silently reverted revocation fails OPEN. The lock is the
//! KERNEL's, held by an open file, so it dies with the process: there is no stale lock to
//! reap, and the `.lock` file's mere existence never blocks anyone. The wait is bounded; on
//! timeout the command refuses and says what to look for. `crate::lock` is shared with
//! `crate::store::Store::open`, which takes the SHARED half of the same mechanism for a
//! board's own lifetime — see that module's doc comment for why one file now serves both.
//! Where there is no `flock` (Windows) this compiles to no lock at all — the write is still
//! atomic, but two writers at the same instant are last-writer-wins, as they were before.
//!
//! Writes are atomic (a temp file beside the real file, then a rename), the file is created
//! private (mode 0600, its directory 0700 when tb has to create it), and a file that is not a
//! JSON object is REFUSED by name and never overwritten: tb does not guess what a person meant.
//!
//! Symbolic links, as `tb`'s board files treat them: the settings are the file the chain of
//! links ends at. tb follows the chain itself and writes THERE, so a dangling link (the
//! natural dotfiles setup for a new tool) is filled in instead of being replaced by a regular
//! file. A link into a directory that does not exist, and a chain that never ends, are
//! refused: tb creates the settings file through a link, never directories.

use crate::lock;
use crate::store::{BoardError, Code, Result};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The file's top-level keys with their text exactly as written. `serde_json::Map` is always
/// `Map<String, Value>`, so this is a plain `BTreeMap` — which iterates in key order, the same
/// order `serde_json::Map` uses.
type Raw = BTreeMap<String, Box<RawValue>>;

/// The most a settings file may hold. Bounded like every other file tb reads: a named pipe or
/// a device (`/dev/zero`) is refused before it is opened, so nothing can hang or grow forever.
pub const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// How long `update` waits for another `tb` to finish writing before it gives up.
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// `TB_CONFIG` (so a test harness, or a `TB_DB` run, can isolate it), else
/// `~/.config/terminal-board/config.json`.
pub fn path() -> PathBuf {
    match crate::env("CONFIG") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
            .join(".config/terminal-board/config.json"),
    }
}

/// The settings object. A missing or empty file is an empty object (nothing is set); every
/// other problem is an error. Use this when the user ASKED about a setting.
pub fn load() -> Result<Map<String, Value>> {
    load_at(&path())
}

fn load_at(path: &Path) -> Result<Map<String, Value>> {
    Ok(values(&load_raw(path)?))
}

/// The settings object for a command that did not ask about a setting: a file tb cannot READ
/// (no permission, a path component that is not a directory, a pipe or a device) is "nothing
/// is set", said once on stderr. A file that reads fine but is not a settings object is still
/// an error — tb knows a setting may be in there and will not guess.
pub fn load_for_read() -> Result<Map<String, Value>> {
    load_for_read_at(&path())
}

fn load_for_read_at(path: &Path) -> Result<Map<String, Value>> {
    match load_raw(path) {
        Ok(raw) => Ok(values(&raw)),
        Err(Unusable::Unreadable(why)) => {
            warn_once(&why);
            Ok(Map::new())
        }
        Err(e) => Err(e.into()),
    }
}

/// Change the settings: `change` edits the object, which is then written back whole. When
/// `change` leaves it as it was, nothing is written and no file is created.
pub fn update(change: impl FnOnce(&mut Map<String, Value>)) -> Result<()> {
    update_at(&path(), change)
}

fn update_at(settings: &Path, change: impl FnOnce(&mut Map<String, Value>)) -> Result<()> {
    let target = resolve_target(settings)?;
    // What did the caller change? Read once without the lock and run the closure: a call that
    // changes nothing must not create a file, a lock file or contention.
    let before_raw = load_raw(&target)?;
    let before = values(&before_raw);
    let mut after = before.clone();
    change(&mut after);
    let ops = delta(&before, &after);
    if ops.is_empty() {
        return Ok(());
    }
    // From here the file will change. Everything below happens under the lock, and the file is
    // read AGAIN inside it: the caller's changes are applied to what the file says NOW, so a
    // write that landed in between is kept rather than reverted.
    let dir = parent_of(&target);
    if !dir.is_dir() {
        if target != settings {
            // through a link tb writes the settings file, never directories
            return Err(BoardError(format!(
                "{} is a symbolic link into a directory that does not exist ({}) — create that directory, or point TB_CONFIG somewhere else",
                settings.display(),
                dir.display()
            ), Code::IoError));
        }
        create_private_dir(&dir).map_err(|e| cannot("create the directory for", &target, &dir, e))?;
    }
    let _guard = lock_settings(&target, LOCK_WAIT)?;
    let fresh_raw = load_raw(&target)?;
    let mut out: Raw = fresh_raw.clone();
    for op in &ops {
        match op {
            Op::Set(k, v) => {
                out.insert(k.clone(), raw_of(v));
            }
            Op::Remove(k) => {
                out.remove(k);
            }
        }
    }
    let text = render(&out);
    if !fresh_raw.is_empty() && text == render(&fresh_raw) && target.exists() {
        return Ok(()); // somebody else already wrote exactly this
    }
    write_atomically(&target, &dir, &text)?;
    // The rename is what other processes see, and it happened under the lock. Making the
    // rename itself survive a crash (syncing the directory) does not need the lock, so it is
    // done after letting go: under a busy disk that sync is a large share of the time a
    // writer would otherwise make every other writer wait.
    drop(_guard);
    let _ = std::fs::File::open(&dir).map(|d| d.sync_all());
    Ok(())
}

/// One change to one top-level key.
enum Op {
    Set(String, Value),
    Remove(String),
}

fn delta(before: &Map<String, Value>, after: &Map<String, Value>) -> Vec<Op> {
    let mut ops = Vec::new();
    for (k, v) in after {
        if before.get(k) != Some(v) {
            ops.push(Op::Set(k.clone(), v.clone()));
        }
    }
    for k in before.keys() {
        if !after.contains_key(k) {
            ops.push(Op::Remove(k.clone()));
        }
    }
    ops
}

/// Why a settings file cannot be used.
enum Unusable {
    /// tb could not read it at all: no permission, not a directory on the way, not a regular
    /// file, bigger than the limit. Nothing is known about what it holds.
    Unreadable(String),
    /// tb read it and it is not a settings object.
    NotSettings(String),
}

impl From<Unusable> for BoardError {
    fn from(u: Unusable) -> BoardError {
        BoardError(
            match u {
                Unusable::Unreadable(m) | Unusable::NotSettings(m) => m,
            },
            Code::IoError,
        )
    }
}

/// Say once, on stderr, that the settings could not be read. Every later command in this
/// process stays quiet: one line is a warning, a line per read is noise.
fn warn_once(why: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SAID: AtomicBool = AtomicBool::new(false);
    if !SAID.swap(true, Ordering::Relaxed) {
        eprintln!("{}", crate::text::sanitize_lines(&format!("tb: {why}")));
    }
}

/// The file's top-level keys, each with its text EXACTLY as written (`RawValue`), so a number
/// tb does not understand — a 30-digit integer, a float at the edge of the exponent range —
/// is written back digit for digit instead of through a `f64`.
fn load_raw(path: &Path) -> std::result::Result<Raw, Unusable> {
    let Some(text) = read_bounded(path)? else {
        return Ok(Raw::new());
    };
    // an empty file is what a crashed editor or `touch` leaves: nothing set, not an error
    if text.trim().is_empty() {
        return Ok(Raw::new());
    }
    let broken = |why: String| {
        Unusable::NotSettings(format!(
            "the settings file {p} is not usable ({why}) — fix it, or move it aside with 'mv {p} {p}.bad' and set things again (tb never overwrites a file it cannot read)",
            p = path.display()
        ))
    };
    match serde_json::from_str::<Raw>(&text) {
        Ok(map) => Ok(map),
        Err(e) if e.is_syntax() || e.is_eof() => Err(broken(format!("not valid JSON: {e}"))),
        Err(_) => Err(broken("it must be a JSON object: { … }".to_string())),
    }
}

/// The file's text, or `None` when there is no file. Only a regular file is ever opened: a
/// named pipe would block for ever and a device would never end, and neither is a settings
/// file. What is opened is read with a hard bound, so nothing can grow without limit.
fn read_bounded(path: &Path) -> std::result::Result<Option<String>, Unusable> {
    let unreadable = |what: String| Unusable::Unreadable(format!("cannot read the settings file {}: {what}", path.display()));
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(unreadable(format!(
                "{e} — tb carried on as if nothing were set; make it readable, or point TB_CONFIG at another file"
            )))
        }
    };
    if !meta.is_file() {
        let what = if meta.is_dir() { "it is a directory" } else { "it is not a regular file (a pipe or a device cannot be settings)" };
        return Err(unreadable(format!(
            "{what} — tb carried on as if nothing were set; point TB_CONFIG at a JSON file"
        )));
    }
    if meta.len() > MAX_SETTINGS_BYTES {
        return Err(unreadable(format!(
            "it is {} bytes, over the limit of {MAX_SETTINGS_BYTES} — tb carried on as if nothing were set; point TB_CONFIG at a settings file",
            meta.len()
        )));
    }
    let mut text = String::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(MAX_SETTINGS_BYTES + 1).read_to_string(&mut text));
    match read {
        Ok(n) if n as u64 > MAX_SETTINGS_BYTES => Err(unreadable(format!(
            "it is over the limit of {MAX_SETTINGS_BYTES} bytes — tb carried on as if nothing were set; point TB_CONFIG at a settings file"
        ))),
        Ok(_) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Err(unreadable(
            "it is not UTF-8 text — tb carried on as if nothing were set; fix it, or point TB_CONFIG at another file".to_string(),
        )),
        Err(e) => Err(unreadable(format!(
            "{e} — tb carried on as if nothing were set; make it readable, or point TB_CONFIG at another file"
        ))),
    }
}

/// Every key as an ordinary `Value`, for the caller.
fn values(raw: &Raw) -> Map<String, Value> {
    raw.iter()
        .map(|(k, v)| (k.clone(), serde_json::from_str(v.get()).unwrap_or(Value::Null)))
        .collect()
}

fn raw_of(v: &Value) -> Box<RawValue> {
    RawValue::from_string(serde_json::to_string(v).unwrap_or_else(|_| "null".into()))
        .unwrap_or_else(|_| RawValue::from_string("null".into()).expect("null is JSON"))
}

/// The object as text: one key per line, sorted (`serde_json::Map` is ordered), each value
/// exactly as it came in unless this write changed it.
fn render(map: &Raw) -> String {
    let mut out = String::from("{\n");
    for (i, (k, v)) in map.iter().enumerate() {
        out.push_str("  ");
        out.push_str(&serde_json::to_string(k).unwrap_or_else(|_| "\"\"".into()));
        out.push_str(": ");
        out.push_str(v.get().trim());
        if i + 1 < map.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("}\n");
    out
}

fn parent_of(target: &Path) -> PathBuf {
    target.parent().filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

fn cannot(what: &str, target: &Path, dir: &Path, e: std::io::Error) -> BoardError {
    BoardError(
        format!(
            "cannot {what} {}: {e} — check that {} is writable, or point TB_CONFIG at another file",
            target.display(),
            dir.display()
        ),
        Code::IoError,
    )
}

/// Is `path` itself a symbolic link (dangling or not)?
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// `path` followed through any chain of symbolic links to the path the chain ends at — which
/// need not exist yet, so a dangling link is FILLED IN rather than replaced by a regular file.
/// (The same rule board files follow; only the last component is followed by hand, a symlinked
/// directory on the way is the operating system's business.) A chain that never ends is refused.
fn resolve_target(path: &Path) -> Result<PathBuf> {
    let mut p = path.to_path_buf();
    for _ in 0..40 {
        if !is_symlink(&p) {
            return Ok(p);
        }
        let to = std::fs::read_link(&p).map_err(|e| {
            BoardError(format!("cannot follow the symbolic link {}: {e} — fix the link, or point TB_CONFIG at the real file", p.display()), Code::IoError)
        })?;
        p = if to.is_absolute() { to } else { parent_of(&p).join(to) };
    }
    Err(BoardError(format!(
        "{} is a symbolic link that never ends (a loop, or more than 40 links) — fix the link, or point TB_CONFIG at the real file",
        path.display()
    ), Code::IoError))
}

fn write_atomically(target: &Path, dir: &Path, text: &str) -> Result<()> {
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        target.file_name().and_then(|n| n.to_str()).unwrap_or("config.json"),
        std::process::id()
    ));
    let written = (|| -> std::io::Result<()> {
        let mut f = create_private_file(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, target)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(cannot("write", target, dir, e));
    }
    // the rename itself is durable only once the directory is synced: the caller does that
    // (best effort, never fatal) after it lets go of the lock
    Ok(())
}

/// A new file only its owner can read or write (0600 from creation; the umask can only
/// narrow it). `O_EXCL`, so it is never a symbolic link somebody left in the way.
fn create_private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    // a temp file left by a crashed writer with the same pid: replace it
    match o.open(path) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(path)?;
            o.open(path)
        }
        other => other,
    }
}

/// Create the settings directory (and parents). Only a directory tb creates is made private;
/// one that already exists is the user's and is left alone.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

/// One writer at a time, for as long as a process lives: `crate::lock::take` on a `.lock` file
/// beside the settings (`crate::lock::sibling`), EXCLUSIVE — the same mechanism `Store::open`
/// now uses SHARED for a board's own lifetime (`crate::store`), lifted out of this file so the
/// two never drift apart. It is released when the process ends however it ends, so a killed
/// `tb` can never wedge the file; the `.lock` file's EXISTENCE means nothing, so there is no
/// stale lock to detect or reap. Only a process holding it right now blocks anyone. Messages
/// here stay settings-specific; `crate::lock` itself knows nothing about what a caller locks.
fn lock_settings(path: &Path, wait: Duration) -> Result<lock::Guard> {
    lock::take(&lock::sibling(path), lock::Mode::Exclusive, wait).map_err(|e| match e {
        lock::Error::Io(e) => BoardError(format!(
            "cannot lock the settings file: {e} at {} — check that folder is writable, or point TB_CONFIG at another file",
            path.display()
        ), Code::IoError),
        lock::Error::Busy(_) => BoardError(format!(
            "another tb has been writing the settings for {}s — wait for it to finish and try again (if nothing is running, a process is stuck holding {})",
            wait.as_secs(),
            path.display()
        ), Code::IoError),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_and_empty_files_are_an_empty_object() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nested/dir/config.json");
        assert!(load_raw(&p).ok().unwrap().is_empty());
        // a no-op update creates nothing — not even the directory or a lock file
        update_at(&p, |_| {}).unwrap();
        update_at(&p, |m| {
            m.remove("default_board");
        })
        .unwrap();
        assert!(!p.exists() && !p.parent().unwrap().exists());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, " \n").unwrap();
        assert!(load_raw(&p).ok().unwrap().is_empty());
    }

    #[test]
    fn a_file_that_is_not_a_json_object_is_refused_and_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.json");
        for bad in ["{ \"default_board\": \"work\", ", "[1, 2]", "\"work\"", "not json at all"] {
            std::fs::write(&p, bad).unwrap();
            let e = load_at(&p).unwrap_err().0;
            assert!(e.contains("is not usable") && e.contains(" — fix it, or move it aside with 'mv "), "{e}");
            let e = update_at(&p, |m| {
                m.insert("default_board".into(), json!("other"));
            })
            .unwrap_err()
            .0;
            assert!(e.contains("never overwrites"), "{e}");
            assert_eq!(std::fs::read_to_string(&p).unwrap(), bad, "the user's file is untouched");
        }
    }

    #[test]
    fn a_value_this_writer_does_not_understand_is_kept_digit_for_digit() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.json");
        // an integer no f64 can hold, and a float at the edge of the exponent range
        let theirs = r#"{"big":123456789012345678901234567890,"exp":1.2345678901234567e-300,"esc":"é🚀","deep":{"a":[{"b":[1.0,2e0]}]}}"#;
        std::fs::write(&p, theirs).unwrap();
        for i in 0..6 {
            update_at(&p, |m| {
                m.insert("default_board".into(), json!(format!("board{i}")));
            })
            .unwrap();
        }
        let text = std::fs::read_to_string(&p).unwrap();
        for exact in ["123456789012345678901234567890", "1.2345678901234567e-300", r#""é🚀""#, r#"{"a":[{"b":[1.0,2e0]}]}"#] {
            assert!(text.contains(exact), "{exact} did not survive six writes:\n{text}");
        }
        assert!(text.contains("\"default_board\": \"board5\""));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_created_private_and_a_symlink_is_followed_not_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("fresh/config.json");
        update_at(&p, |m| {
            m.insert("default_board".into(), json!("work"));
        })
        .unwrap();
        let mode = |q: &Path| std::fs::metadata(q).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&p), 0o600);
        assert_eq!(mode(p.parent().unwrap()), 0o700);

        // a DANGLING link (the dotfiles setup for a new tool): the link stays a link and its
        // target is created, rather than the link being replaced by a regular file
        let real = d.path().join("dotfiles/tb.json");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        let link = d.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        update_at(&link, |m| {
            m.insert("default_board".into(), json!("home"));
        })
        .unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link was replaced");
        assert_eq!(load_at(&link).unwrap()["default_board"], "home");
        assert_eq!(mode(&real), 0o600);

        // a chain of links ends at the real file
        let mid = d.path().join("mid.json");
        let top = d.path().join("top.json");
        std::os::unix::fs::symlink(d.path().join("dotfiles/chain.json"), &mid).unwrap();
        std::os::unix::fs::symlink(&mid, &top).unwrap();
        update_at(&top, |m| {
            m.insert("default_board".into(), json!("chained"));
        })
        .unwrap();
        for l in [&mid, &top] {
            assert!(std::fs::symlink_metadata(l).unwrap().file_type().is_symlink(), "{}", l.display());
        }
        assert!(d.path().join("dotfiles/chain.json").is_file());

        // a loop, and a link into a directory that does not exist: refused, nothing created
        let loop_a = d.path().join("a.json");
        let loop_b = d.path().join("b.json");
        std::os::unix::fs::symlink(&loop_b, &loop_a).unwrap();
        std::os::unix::fs::symlink(&loop_a, &loop_b).unwrap();
        let e = update_at(&loop_a, |m| {
            m.insert("default_board".into(), json!("x"));
        })
        .unwrap_err()
        .0;
        assert!(e.contains("never ends") || e.contains("cannot follow"), "{e}");
        let into_nowhere = d.path().join("nowhere.json");
        std::os::unix::fs::symlink(d.path().join("no/such/dir/c.json"), &into_nowhere).unwrap();
        let e = update_at(&into_nowhere, |m| {
            m.insert("default_board".into(), json!("x"));
        })
        .unwrap_err()
        .0;
        assert!(e.contains("into a directory that does not exist"), "{e}");
        assert!(!d.path().join("no").exists(), "tb created directories through a link");
        assert!(std::fs::symlink_metadata(&into_nowhere).unwrap().file_type().is_symlink());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_nothing_set_for_reads_and_an_error_for_asking() {
        let d = tempfile::tempdir().unwrap();
        // not a regular file: a directory stands in for a pipe or a device (both are refused
        // by the same check, and neither can be created portably in a unit test)
        let p = d.path().join("adir");
        std::fs::create_dir(&p).unwrap();
        let strict = load_at(&p).unwrap_err().0;
        assert!(strict.contains("it is a directory") && strict.contains("point TB_CONFIG at a JSON file"), "{strict}");
        assert!(load_for_read_at(&p).unwrap().is_empty(), "a read that needs no setting must carry on");
    }

    #[test]
    fn a_file_over_the_limit_is_refused_without_reading_it_all() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("huge.json");
        std::fs::write(&p, vec![b'x'; MAX_SETTINGS_BYTES as usize + 1]).unwrap();
        let e = load_at(&p).unwrap_err().0;
        assert!(e.contains("over the limit of 1048576") && e.contains("carried on as if nothing were set"), "{e}");
    }
}
