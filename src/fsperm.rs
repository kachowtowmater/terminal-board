//! Board files are private. A board can hold confidential text, so every file tb creates —
//! a board, a backup — is born mode 0600, whatever the umask (a umask can only narrow it).
//! SQLite gives the `-wal` and `-shm` sidecars the mode of the main file, so the main file
//! decides all three.
//!
//! tb never changes the mode of a file it did not just create: a board may be shared with a
//! group on purpose. A wider existing file is reported (`Store::open`), and tightened only by
//! `tb config file-mode private`, which says what it did.
//!
//! Symbolic links. A board path may be a link (`boards/work.db -> /mnt/secure/work.db`): the board
//! is the file the chain of links ends at, exactly as SQLite sees it. But tb — not SQLite —
//! creates that file: `create_board` follows the chain itself and creates the TARGET with
//! `O_EXCL` and 0600 (`O_EXCL` never follows a link, so "it exists" on a dangling link must
//! not be mistaken for "the board exists" — SQLite would then create the target 0644). The
//! database is opened at the resolved path. A link into a directory that does not exist, and
//! a chain that never ends, are refused: tb creates a board file through a link, never
//! directories. And tb never changes the mode of a path that is a link: `make_private` works
//! on the open file (`fchmod`) after checking it opened the very file it looked at.
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

/// Create `path` as an empty private file. `Ok(true)`: created it. `Ok(false)`: a file (or
/// directory) was already there, and its mode is left alone. A symbolic link at `path` is an
/// ERROR, never "already there": `O_EXCL` does not follow links, so nothing here says the
/// link's target exists. SQLite opens an empty file as a new database.
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
        Err(e) if is_symlink(path) => Err(std::io::Error::new(e.kind(), "a symbolic link is in the way")),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        // some systems answer "permission denied" for an existing file in a read-only directory
        Err(_) if path.exists() => Ok(false),
        Err(e) => Err(e),
    }
}

/// Is `path` itself a symbolic link (dangling or not)?
pub fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// `path` followed through any chain of symbolic links to the path the chain ends at — which
/// need not exist yet. Only the last component is followed by hand (a symlinked directory on
/// the way is the operating system's business). An endless chain is an error.
pub fn resolve(path: &Path) -> std::io::Result<PathBuf> {
    let mut p = path.to_path_buf();
    for _ in 0..40 {
        if !is_symlink(&p) {
            return Ok(p);
        }
        let to = std::fs::read_link(&p)?;
        p = if to.is_absolute() { to } else { p.parent().unwrap_or(Path::new("")).join(to) };
    }
    Err(std::io::Error::other("too many levels of symbolic links"))
}

/// Make sure the board file `path` names exists, creating it private when it does not.
/// Returns the path the board really lives at (see the module notes on symbolic links) and
/// whether this call created it. The error is a whole refusal: what happened — what to do.
pub fn create_board(path: &Path) -> Result<(PathBuf, bool), String> {
    let mut real = path.to_path_buf();
    // a few rounds: a link that appears at the resolved path meanwhile is followed too
    for _ in 0..4 {
        real = resolve(&real).map_err(|_| {
            format!(
                "{} is a symbolic link that never ends (a loop, or more than 40 links) — fix the link, or name the real file",
                path.display()
            )
        })?;
        if real != path {
            // through a link tb creates the board file, never directories
            if let Some(dir) = real.parent().filter(|d| !d.as_os_str().is_empty() && !d.is_dir()) {
                return Err(format!(
                    "{} is a symbolic link into a directory that does not exist ({}) — create that directory, or fix the link",
                    path.display(),
                    dir.display()
                ));
            }
        }
        match create_private(&real) {
            Ok(created) => return Ok((real, created)),
            Err(_) if is_symlink(&real) => continue,
            Err(e) => return Err(format!("cannot create {}: {e} — set TB_DB to a writable path", real.display())),
        }
    }
    Err(format!("{} keeps turning into another symbolic link — check who else writes to that folder, then try again", path.display()))
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

/// What `make_private` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tightened {
    /// Each file that was wider, with the mode it had.
    pub changed: Vec<(PathBuf, u32)>,
    /// Paths left alone because they are not regular files (a symbolic link someone planted).
    pub skipped: Vec<PathBuf>,
}

/// Make `path` and whichever of its sidecars exist private. Never through a symbolic link:
/// the mode is changed on the OPEN file, after checking that what opened is the very file
/// that was looked at, so a link swapped in at any moment cannot redirect it.
pub fn make_private(path: &Path) -> std::io::Result<Tightened> {
    let mut done = Tightened::default();
    for f in with_sidecars(path) {
        let Ok(seen) = std::fs::symlink_metadata(&f) else { continue };
        if !seen.file_type().is_file() {
            done.skipped.push(f);
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let mode = seen.permissions().mode() & 0o7777;
            if !is_wide(mode) {
                continue;
            }
            let file = std::fs::File::open(&f)?;
            let opened = file.metadata()?;
            if (opened.dev(), opened.ino()) != (seen.dev(), seen.ino()) {
                done.skipped.push(f);
                continue;
            }
            file.set_permissions(std::fs::Permissions::from_mode(PRIVATE))?;
            done.changed.push((f, mode));
        }
    }
    Ok(done)
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
        let done = make_private(&db).unwrap();
        assert_eq!(done.changed, vec![(db.clone(), 0o644), (wal.clone(), 0o664)]);
        assert!(done.skipped.is_empty());
        assert_eq!((mode_of(&db), mode_of(&wal), mode_of(&shm)), (Some(0o600), Some(0o600), None));
        assert_eq!(make_private(&db).unwrap(), Tightened::default(), "nothing left to tighten");
        assert!(is_wide(0o640) && is_wide(0o604) && !is_wide(0o600) && !is_wide(0o400));
        assert_eq!(fmt_mode(0o644), "0644");
    }

    #[test]
    fn a_mode_is_never_changed_through_a_symbolic_link() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("b.db");
        let victim = dir.path().join("someone-elses-file");
        for f in [&db, &victim] {
            std::fs::write(f, b"x").unwrap();
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let [_, wal, _] = with_sidecars(&db);
        std::os::unix::fs::symlink(&victim, &wal).unwrap(); // planted where a sidecar would be
        let done = make_private(&db).unwrap();
        assert_eq!(done.changed, vec![(db.clone(), 0o644)]);
        assert_eq!(done.skipped, vec![wal.clone()], "the link is reported, not followed");
        assert_eq!(mode_of(&victim), Some(0o644), "the file behind the link is untouched");
        assert!(is_symlink(&wal));
    }

    #[test]
    fn create_private_never_takes_a_link_for_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link.db");
        std::os::unix::fs::symlink(dir.path().join("nowhere.db"), &link).unwrap();
        assert!(create_private(&link).is_err(), "a dangling link is not 'already there'");
        assert!(!dir.path().join("nowhere.db").exists(), "and nothing was created through it");
    }

    #[test]
    fn resolve_follows_a_chain_to_a_target_that_need_not_exist() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir(d.join("real")).unwrap();
        symlink("b.db", d.join("a.db")).unwrap(); // relative, to another link
        symlink("real/c.db", d.join("b.db")).unwrap(); // relative, dangling
        assert_eq!(resolve(&d.join("a.db")).unwrap(), d.join("real/c.db"));
        assert_eq!(resolve(&d.join("plain.db")).unwrap(), d.join("plain.db"), "no link: itself");
        let (real, created) = create_board(&d.join("a.db")).unwrap();
        assert_eq!((real.clone(), created), (d.join("real/c.db"), true));
        assert_eq!(mode_of(&real), Some(0o600));
        assert_eq!(create_board(&d.join("a.db")).unwrap(), (real, false), "now it exists");
        assert!(is_symlink(&d.join("a.db")) && is_symlink(&d.join("b.db")), "the links stay links");

        symlink("loop2.db", d.join("loop1.db")).unwrap();
        symlink("loop1.db", d.join("loop2.db")).unwrap();
        let e = create_board(&d.join("loop1.db")).unwrap_err();
        assert!(e.contains("never ends") && e.contains(" — "), "{e}");
        symlink("missing-dir/x.db", d.join("into-nowhere.db")).unwrap();
        let e = create_board(&d.join("into-nowhere.db")).unwrap_err();
        assert!(e.contains("into a directory that does not exist") && e.contains("missing-dir"), "{e}");
        assert!(!d.join("missing-dir").exists(), "tb creates a file through a link, never directories");
    }
}
