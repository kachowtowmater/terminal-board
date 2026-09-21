//! Machine-local settings: `~/.config/terminal-board/config.json` (or the file `TB_CONFIG` names).
//!
//! A board is a file people copy, so anything that belongs to a PERSON on a MACHINE — which
//! board plain `tb` opens, and later which commands this machine trusts — lives here, never
//! inside a board file, and never next to the boards either (it is not `install.conf`, the
//! installer's own record, which `install.sh --uninstall` deletes while keeping boards).
//!
//! One JSON object, one top-level key per owner (`default_board`, …). The API is three
//! functions over a generic map, so two features never edit one type:
//! - [`path`] — where the file is;
//! - [`load`] — the whole object; a missing file is an empty object;
//! - [`update`] — read, change, write back. Keys the caller does not touch survive exactly
//!   (values are kept as parsed; key order is normalised to alphabetical).
//!
//! Writes are atomic (a temp file beside it, then a rename), the file is created private
//! (mode 0600, its directory 0700 when tb has to create it), and a file that is not a JSON
//! object is REFUSED by name and never overwritten: tb does not guess what a person meant.
//! `update` re-reads immediately before writing; two writers at the same instant are
//! last-writer-wins, never a torn file.

use crate::store::{BoardError, Result};
use serde_json::{Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// `TB_CONFIG` (so a test harness, or a `TB_DB` run, can isolate it), else
/// `~/.config/terminal-board/config.json`.
pub fn path() -> PathBuf {
    match crate::env("CONFIG") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
            .join(".config/terminal-board/config.json"),
    }
}

/// The settings object. A missing file is an empty object (every setting at its default).
pub fn load() -> Result<Map<String, Value>> {
    load_from(&path())
}

/// Change the settings: `change` edits the object, which is then written back whole. When
/// `change` leaves it as it was, nothing is written (a no-op never creates the file).
pub fn update(change: impl FnOnce(&mut Map<String, Value>)) -> Result<()> {
    update_at(&path(), change)
}

fn load_from(path: &Path) -> Result<Map<String, Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => {
            return Err(BoardError(format!(
                "cannot read the settings file {}: {e} — make it readable, or point TB_CONFIG at another file",
                path.display()
            )))
        }
    };
    // an empty file is what a crashed editor or `touch` leaves: nothing set, not an error
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    let broken = |why: String| {
        BoardError(format!(
            "the settings file {p} is not usable ({why}) — fix it, or move it aside with 'mv {p} {p}.bad' and set things again (tb never overwrites a file it cannot read)",
            p = path.display()
        ))
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(broken("it must be a JSON object: { … }".to_string())),
        Err(e) => Err(broken(format!("not valid JSON: {e}"))),
    }
}

fn update_at(path: &Path, change: impl FnOnce(&mut Map<String, Value>)) -> Result<()> {
    // a malformed file stops here: it is never replaced by what tb would have written
    let before = load_from(path)?;
    let mut after = before.clone();
    change(&mut after);
    if after == before {
        return Ok(());
    }
    // a settings file managed as a symlink (dotfiles) keeps being one: write through it
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let cannot = |what: &str, e: std::io::Error| {
        BoardError(format!(
            "cannot {what} {}: {e} — check that {} is writable, or point TB_CONFIG at another file",
            target.display(),
            dir.display()
        ))
    };
    if !dir.is_dir() {
        create_private_dir(&dir).map_err(|e| cannot("create the directory for", e))?;
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(after)).unwrap_or_else(|_| "{}".into());
    text.push('\n');
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        target.file_name().and_then(|n| n.to_str()).unwrap_or("config.json"),
        std::process::id()
    ));
    let written = (|| -> std::io::Result<()> {
        let mut f = create_private_file(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &target)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(cannot("write", e));
    }
    Ok(())
}

/// A new file only its owner can read or write (0600; the umask can only narrow it).
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_and_empty_files_are_an_empty_object() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nested/dir/config.json");
        assert!(load_from(&p).unwrap().is_empty());
        // a no-op update creates nothing — not even the directory
        update_at(&p, |_| {}).unwrap();
        update_at(&p, |m| {
            m.remove("default_board");
        })
        .unwrap();
        assert!(!p.exists() && !p.parent().unwrap().exists());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, " \n").unwrap();
        assert!(load_from(&p).unwrap().is_empty());
    }

    #[test]
    fn another_owners_keys_survive_a_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.json");
        let theirs = json!({"argv": ["/usr/bin/true", "--flag"], "sha256": "ab12", "nested": {"n": 1.5, "null": null, "list": [1, "two", false]}});
        std::fs::write(&p, serde_json::to_string(&json!({"zeta": "last", "hooks": {"lint": theirs}, "v": 1})).unwrap()).unwrap();
        update_at(&p, |m| {
            m.insert("default_board".into(), json!("work"));
        })
        .unwrap();
        let got = load_from(&p).unwrap();
        assert_eq!(got["default_board"], "work");
        assert_eq!(got["hooks"]["lint"], theirs, "a key this writer knows nothing about is kept exactly");
        assert_eq!((&got["zeta"], &got["v"]), (&json!("last"), &json!(1)));
        update_at(&p, |m| {
            m.remove("default_board");
        })
        .unwrap();
        let got = load_from(&p).unwrap();
        assert!(!got.contains_key("default_board") && got["hooks"]["lint"] == theirs);
        // no temp file is left behind
        let names: Vec<String> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["config.json"]);
    }

    #[test]
    fn a_file_that_is_not_a_json_object_is_refused_and_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.json");
        for bad in ["{ \"default_board\": \"work\", ", "[1, 2]", "\"work\"", "not json at all"] {
            std::fs::write(&p, bad).unwrap();
            let e = load_from(&p).unwrap_err().0;
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

    #[cfg(unix)]
    #[test]
    fn the_file_is_created_private_and_a_symlink_stays_a_symlink() {
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
        // a wider file someone made by hand is private after the next write
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        update_at(&p, |m| {
            m.insert("default_board".into(), json!("home"));
        })
        .unwrap();
        assert_eq!(mode(&p), 0o600);
        // dotfiles: config.json is a symlink into another directory
        let real = d.path().join("dotfiles/tb.json");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "{}").unwrap();
        let link = d.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        update_at(&link, |m| {
            m.insert("default_board".into(), json!("work"));
        })
        .unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(load_from(&real).unwrap()["default_board"], "work");
    }
}
