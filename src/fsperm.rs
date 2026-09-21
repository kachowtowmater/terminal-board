//! Board files are private. A board can hold confidential text, so every file tb creates —
//! a board, a backup — is born mode 0600, whatever the umask (a umask can only narrow it).
//! SQLite gives the `-wal` and `-shm` sidecars the mode of the main file, so the main file
//! decides all three.
//!
//! tb never changes the mode of a file it did not just create: a board may be shared with a
//! group on purpose. A wider existing file is reported (`Store::open`), and tightened only by
//! `tb config file-mode private`, which says what it did.
//!
//! On a platform without unix permissions all of this compiles to "create the file" and
//! "nothing to report".

use std::path::{Path, PathBuf};

/// The mode of every file tb creates.
pub const PRIVATE: u32 = 0o600;

/// `path` with the SQLite sidecars that may sit next to it.
pub fn with_sidecars(path: &Path) -> [PathBuf; 3] {
    let side = |ext: &str| PathBuf::from(format!("{}{ext}", path.display()));
    [path.to_path_buf(), side("-wal"), side("-shm")]
}

/// Create `path` as an empty private file. `Ok(true)`: created it. `Ok(false)`: it was
/// already there, and its mode is left alone. SQLite opens an empty file as a new database.
pub fn create_private(path: &Path) -> std::io::Result<bool> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(PRIVATE);
    }
    match o.open(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        // some systems answer "permission denied" for an existing file in a read-only directory
        Err(_) if path.exists() => Ok(false),
        Err(e) => Err(e),
    }
}

/// The permission bits of `path`; None when it is missing or the platform has none.
pub fn mode_of(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).ok().map(|m| m.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Does this mode let anyone but the owner in?
pub fn is_wide(mode: u32) -> bool {
    mode & 0o077 != 0
}

/// `0644`
pub fn fmt_mode(mode: u32) -> String {
    format!("{mode:04o}")
}

/// Make `path` and whichever of its sidecars exist private. Returns each file that was wider,
/// with the mode it had.
pub fn make_private(path: &Path) -> std::io::Result<Vec<(PathBuf, u32)>> {
    let mut changed = Vec::new();
    for f in with_sidecars(path) {
        let Some(mode) = mode_of(&f) else { continue };
        if !is_wide(mode) {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(PRIVATE))?;
        }
        changed.push((f, mode));
    }
    Ok(changed)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_created_file_is_private_and_an_existing_one_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let new = dir.path().join("new.db");
        assert!(create_private(&new).unwrap());
        assert_eq!(mode_of(&new), Some(0o600));
        assert_eq!(std::fs::metadata(&new).unwrap().len(), 0, "empty: SQLite treats it as a new database");

        let old = dir.path().join("old.db");
        std::fs::write(&old, b"x").unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!create_private(&old).unwrap(), "already there");
        assert_eq!(mode_of(&old), Some(0o644), "never re-moded on the quiet");
        assert_eq!(std::fs::read(&old).unwrap(), b"x", "never truncated");
    }

    #[test]
    fn make_private_tightens_the_file_and_its_sidecars_and_reports_each() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("b.db");
        let [_, wal, shm] = with_sidecars(&db);
        for (f, m) in [(&db, 0o644), (&wal, 0o664)] {
            std::fs::write(f, b"").unwrap();
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(m)).unwrap();
        }
        let changed = make_private(&db).unwrap();
        assert_eq!(changed, vec![(db.clone(), 0o644), (wal.clone(), 0o664)]);
        assert_eq!((mode_of(&db), mode_of(&wal), mode_of(&shm)), (Some(0o600), Some(0o600), None));
        assert!(make_private(&db).unwrap().is_empty(), "nothing left to tighten");
        assert!(is_wide(0o640) && is_wide(0o604) && !is_wide(0o600) && !is_wide(0o400));
        assert_eq!(fmt_mode(0o644), "0644");
    }
}
