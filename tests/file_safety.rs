//! File safety: a board file is private, `TB_DB` wins over `TB_BOARD` with one warning, and
//! a board written by an older tb is copied aside before its schema changes.
//!
//! Everything here drives the `tb` binary and reads the files it leaves behind — the way a
//! user, an agent or another program meets them.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch machine: its own HOME, and a pinned board file under `TB_DB` when asked.
struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Scratch {
        Scratch { dir: tempfile::tempdir().unwrap() }
    }
    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }
    /// The pinned board file (`TB_DB`), in a directory tb has to create.
    fn db(&self) -> PathBuf {
        self.dir.path().join("pinned/board.db")
    }
    /// A bare `tb` with every variable that picks a board or a name cleared.
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.env("HOME", self.home())
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .env_remove("HERDR_AGENT_NAME")
            .env_remove("TB_NOW");
        for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD"] {
            c.env_remove(k);
        }
        c
    }
    /// `tb ARGS` on the pinned file, with extra environment.
    fn pinned(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        self.cmd().args(args).env("TB_DB", self.db()).envs(env.iter().copied()).output().unwrap()
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn errs(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

fn ok(o: Output) -> Output {
    assert!(o.status.success(), "failed: {}\n{}", errs(&o), out(&o));
    o
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_str(&out(o)).unwrap_or_else(|e| panic!("not JSON ({e}): {}", out(o)))
}

fn sidecars(db: &Path) -> [PathBuf; 2] {
    [PathBuf::from(format!("{}-wal", db.display())), PathBuf::from(format!("{}-shm", db.display()))]
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())).permissions().mode() & 0o7777
}

#[cfg(unix)]
fn chmod(p: &Path, m: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
}

/// `tb ARGS` started by a shell that first sets the umask.
#[cfg(unix)]
fn under_umask(base: Command, umask: &str, args: &[&str]) -> Command {
    let tb = base.get_program().to_owned();
    let envs: Vec<_> = base.get_envs().map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned()))).collect();
    let mut c = Command::new("sh");
    c.arg("-c").arg(format!("umask {umask}; exec \"$0\" \"$@\"")).arg(tb).args(args);
    for (k, v) in envs {
        match v {
            Some(v) => c.env(k, v),
            None => c.env_remove(k),
        };
    }
    c
}

#[cfg(unix)]
#[test]
fn a_new_board_file_is_private_whatever_the_umask() {
    for umask in ["000", "022", "077"] {
        // a pinned file (TB_DB), in a directory tb creates
        let s = Scratch::new();
        let mut c = s.cmd();
        c.env("TB_DB", s.db());
        ok(under_umask(c, umask, &["add", "plain: a card"]).output().unwrap());
        assert_eq!(mode(&s.db()), 0o600, "TB_DB file under umask {umask}");

        // the boards directory: the default board and a named one
        let boards = s.home().join(".local/state/terminal-board/boards");
        ok(under_umask(s.cmd(), umask, &["add", "plain: a card"]).output().unwrap());
        ok(under_umask(s.cmd(), umask, &["work", "add", "plain: a card"]).output().unwrap());
        for name in ["default.db", "work.db"] {
            assert_eq!(mode(&boards.join(name)), 0o600, "{name} under umask {umask}");
        }
    }
}

#[cfg(unix)]
#[test]
fn the_sidecars_tb_creates_are_private_too() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let s = Scratch::new();
    let mut c = s.cmd();
    c.env("TB_DB", s.db());
    ok(under_umask(c, "000", &["add", "plain: a card"]).output().unwrap());
    // a long-lived tb keeps its connection — and so the -wal and -shm files — open
    let mut c = s.cmd();
    c.env("TB_DB", s.db());
    let mut child =
        under_umask(c, "000", &["watch", "--json"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut first).unwrap();
    assert!(first.contains("\"columns\""), "watch printed the board: {first}");
    let seen: Vec<(PathBuf, bool, Option<u32>)> =
        sidecars(&s.db()).into_iter().map(|f| (f.clone(), f.exists(), f.exists().then(|| mode(&f)))).collect();
    let _ = child.kill();
    let _ = child.wait();
    for (f, exists, m) in seen {
        assert!(exists, "{} exists while tb holds the board open", f.display());
        assert_eq!(m, Some(0o600), "{}", f.display());
    }
    assert_eq!(mode(&s.db()), 0o600);
}

#[cfg(unix)]
#[test]
fn an_existing_wider_file_is_reported_and_tightened_only_on_request() {
    let s = Scratch::new();
    ok(s.pinned(&["add", "plain: a card"], &[]));
    let db = s.db();
    chmod(&db, 0o644); // a board file as every earlier version created it
    // another program has the board open, so the sidecars exist (with the file's mode)
    let held = rusqlite::Connection::open(&db).unwrap();
    let n: i64 = held.query_row("SELECT COUNT(*) FROM cards", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);

    // reported: one line, on every kind of command, and the mode is NOT changed
    let o = ok(s.pinned(&["list"], &[]));
    assert!(out(&o).contains("a card"));
    let e = errs(&o);
    let lines: Vec<&str> = e.lines().filter(|l| l.contains("open to other users")).collect();
    assert_eq!(lines.len(), 1, "one warning line: {e}");
    assert!(lines[0].starts_with("tb: ") && lines[0].contains(db.to_str().unwrap()), "{e}");
    assert!(lines[0].contains("(mode 0644)") && lines[0].contains("'tb config file-mode private'"), "{e}");
    assert!(lines[0].contains("'tb config file-mode shared'"), "{e}");
    assert_eq!(mode(&db), 0o644, "never tightened on the quiet");

    // --json: an additive `warnings` field on object output; array output keeps its shape
    let v = json(&ok(s.pinned(&["board", "--json"], &[])));
    assert_eq!(v["v"], 1);
    let w = v["warnings"].as_array().cloned().unwrap_or_default();
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].as_str().unwrap().contains("(mode 0644)"), "{w:?}");
    let o = ok(s.pinned(&["list", "--json"], &[]));
    assert!(json(&o).is_array(), "list --json is still an array");
    assert!(errs(&o).contains("open to other users"), "an array has no field: the warning is on stderr");
    let cfg = out(&ok(s.pinned(&["config"], &[])));
    assert!(cfg.lines().any(|l| l.starts_with("file-mode") && l.contains("0644")), "{cfg}");
    assert!(out(&ok(s.pinned(&["config", "file-mode"], &[]))).contains("0644 (open to other users)"));
    assert_eq!(mode(&db), 0o644, "reading the setting changes nothing");

    // tightened on request, visibly: the file AND its live sidecars, and it is logged
    let o = ok(s.pinned(&["config", "file-mode", "private"], &[]));
    let said = out(&o);
    assert!(said.contains("is now private (mode 0600)") && said.contains("board.db was 0644"), "{said}");
    assert_eq!(mode(&db), 0o600);
    for f in sidecars(&db) {
        assert!(f.exists(), "{} is live", f.display());
        assert_eq!(mode(&f), 0o600, "{}", f.display());
        assert!(said.contains(&format!("{} was", f.file_name().unwrap().to_str().unwrap())), "{said}");
    }
    let (actor, text): (String, String) = held
        .query_row("SELECT actor, text FROM board_events WHERE kind='file-mode' ORDER BY id DESC LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .expect("logged on the board");
    assert_eq!(actor, "tester");
    assert!(text.contains("private") && text.contains("0644"), "{text}");

    // and that is the end of it: no warning, no config row, no JSON field
    let o = ok(s.pinned(&["board", "--json"], &[]));
    assert!(!errs(&o).contains("open to other users"), "{}", errs(&o));
    assert!(json(&o).get("warnings").is_none() && !out(&o).contains("warnings"));
    assert!(!out(&ok(s.pinned(&["config"], &[]))).contains("file-mode"));
    assert!(out(&ok(s.pinned(&["config", "file-mode", "private"], &[]))).contains("already private"));
    assert!(out(&ok(s.pinned(&["config", "file-mode"], &[]))).contains("private (0600)"));

    // shared on purpose: say so once, and tb leaves the mode alone and stops reporting it
    chmod(&db, 0o664);
    assert!(errs(&ok(s.pinned(&["list"], &[]))).contains("(mode 0664)"));
    let v = json(&ok(s.pinned(&["config", "file-mode", "shared", "--json"], &[])));
    assert_eq!(v["ok"], true);
    assert_eq!(v["config"]["key"], "file-mode");
    assert_eq!(v["config"]["value"], "shared (0664)");
    let o = ok(s.pinned(&["list"], &[]));
    assert!(!errs(&o).contains("open to other users"), "{}", errs(&o));
    assert_eq!(mode(&db), 0o664);
    assert!(out(&ok(s.pinned(&["config"], &[]))).lines().any(|l| l.starts_with("file-mode") && l.contains("shared (0664)")));

    // anything else is refused in the house style
    let o = s.pinned(&["config", "file-mode", "0777"], &[]);
    assert!(!o.status.success());
    assert!(errs(&o).contains("is not private|shared") && errs(&o).contains("'tb config file-mode private'"), "{}", errs(&o));
    let o = s.pinned(&["config", "file-mode", "0777", "--json"], &[]);
    let v = json(&o);
    assert!(!o.status.success());
    assert_eq!(v["ok"], false);
    assert!(v["hint"].as_str().unwrap().contains("tb config file-mode private"), "{v}");
}

#[cfg(unix)]
#[test]
fn the_report_names_the_board_a_hint_has_to_name() {
    let s = Scratch::new();
    ok(s.cmd().args(["work", "add", "plain: a card"]).output().unwrap());
    let db = s.home().join(".local/state/terminal-board/boards/work.db");
    chmod(&db, 0o640);
    // `tb boards` opens every board: the hint must not send the reader to the default one
    let o = ok(s.cmd().args(["boards"]).output().unwrap());
    let e = errs(&o);
    assert!(e.contains("(mode 0640)") && e.contains("'tb work config file-mode private'"), "{e}");
    ok(s.cmd().args(["work", "config", "file-mode", "private"]).output().unwrap());
    assert_eq!(mode(&db), 0o600);
    assert!(!errs(&ok(s.cmd().args(["boards"]).output().unwrap())).contains("open to other users"));
}

#[test]
fn tb_db_wins_over_tb_board_with_one_warning() {
    let s = Scratch::new();
    let both = [("TB_BOARD", "work")];

    // plain: the command runs on the pinned file and says so in ONE line
    let o = ok(s.pinned(&["add", "plain: on the pinned file"], &both));
    assert!(out(&o).contains("added #1"), "{}", out(&o));
    let e = errs(&o);
    assert_eq!(e.lines().filter(|l| l.contains("TB_BOARD")).count(), 1, "one warning line: {e}");
    assert!(e.contains("tb: TB_DB is set, so TB_BOARD=work is ignored"), "{e}");
    let o = ok(s.pinned(&["list"], &both));
    assert!(out(&o).contains("on the pinned file"));
    assert_eq!(errs(&o).lines().filter(|l| l.contains("TB_BOARD")).count(), 1);

    // --json: an additive `warnings` array on a write, on the board, and on a failure
    let v = json(&ok(s.pinned(&["add", "plain: second", "--json"], &both)));
    assert_eq!(v["ok"], true);
    assert_eq!(v["card"]["id"], 2);
    let w = v["warnings"].as_array().cloned().unwrap_or_default();
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].as_str().unwrap().contains("TB_BOARD=work is ignored"), "{w:?}");
    let v = json(&ok(s.pinned(&["board", "--json"], &both)));
    assert_eq!(v["v"], 1);
    assert_eq!(v["board"], "default", "JSON reports the board that was opened, not the ignored name");
    assert_eq!(v["warnings"].as_array().map(Vec::len), Some(1));
    assert_eq!(v["columns"]["todo"].as_array().map(Vec::len), Some(2));
    let o = s.pinned(&["show", "99", "--json"], &both);
    assert!(!o.status.success());
    let v = json(&o);
    assert_eq!(v["ok"], false);
    assert!(v["hint"].is_string());
    assert_eq!(v["warnings"].as_array().map(Vec::len), Some(1));
    // `tb boards` reports the one board the file is — under its real name
    let v = json(&ok(s.pinned(&["boards", "--json"], &both)));
    assert_eq!(v[0]["name"], "default");
    assert!(errs(&ok(s.pinned(&["boards"], &both))).contains("TB_BOARD=work is ignored"));

    // the cards went to the pinned file and nowhere else
    let conn = rusqlite::Connection::open(s.db()).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM cards", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 2);
    assert!(!s.home().join(".local/state/terminal-board/boards").exists(), "no boards directory appeared");

    // the mistake the refusal guards is untouched: a board NAME typed while a file is pinned
    for args in [vec!["work", "list"], vec!["-b", "work", "list"], vec!["--board", "work", "add", "plain: mixed in"]] {
        for env in [&both[..], &[][..]] {
            let o = s.pinned(&args, env);
            assert!(!o.status.success(), "{args:?}");
            assert!(errs(&o).contains("TB_DB is set") && errs(&o).contains("unset TB_DB to use boards"), "{args:?}: {}", errs(&o));
        }
    }
    let o = s.pinned(&["work", "list", "--json"], &both);
    assert_eq!(json(&o)["ok"], false);

    // nothing to warn about: no TB_BOARD, or TB_BOARD naming the board a pinned file already is
    for env in [&[][..], &[("TB_BOARD", "default")][..]] {
        let o = ok(s.pinned(&["board", "--json"], env));
        assert!(errs(&o).is_empty(), "{}", errs(&o));
        assert!(!out(&o).contains("warnings"), "no field when there is nothing to say");
    }
}

/// The schema tb 1.1.0 wrote (no `cards.reviewer`, no `github_snapshot.fails`).
const SCHEMA_1_1: &str = r#"
CREATE TABLE IF NOT EXISTS cards (
    id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL, tag TEXT, description TEXT NOT NULL DEFAULT '',
    "column" TEXT NOT NULL DEFAULT 'todo' CHECK ("column" IN ('todo','doing','review','done')),
    owner TEXT, due TEXT, gh_ref INTEGER, created_at INTEGER NOT NULL, column_since INTEGER NOT NULL,
    blocked TEXT, position INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS cards_column ON cards("column");
CREATE TABLE IF NOT EXISTS checklist (
    card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE, idx INTEGER NOT NULL, text TEXT NOT NULL,
    done INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (card_id, idx)
);
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT, card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    ts INTEGER NOT NULL, actor TEXT NOT NULL, kind TEXT NOT NULL, text TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS events_card ON events(card_id, id);
CREATE TABLE IF NOT EXISTS github_snapshot (
    key INTEGER PRIMARY KEY CHECK (key = 1), fetched_at INTEGER NOT NULL DEFAULT 0, json TEXT, error TEXT
);
CREATE TABLE IF NOT EXISTS board_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, actor TEXT NOT NULL, kind TEXT NOT NULL,
    text TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS config (key TEXT PRIMARY KEY, value TEXT NOT NULL);
"#;

/// The card columns tb 1.1.0 selects.
const CARD_COLS_1_1: &str =
    r#"id, title, tag, description, "column", owner, due, gh_ref, created_at, column_since, blocked, position"#;

/// A board as tb 1.1.0 left it while something still has it open: WAL mode, one card, and
/// everything it committed still in the `-wal` (nothing checkpointed). The connection comes
/// back so the caller keeps the WAL hot.
fn old_board(db: &Path) -> rusqlite::Connection {
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(db).unwrap();
    let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0)).unwrap();
    conn.execute_batch(SCHEMA_1_1).unwrap();
    conn.execute(
        r#"INSERT INTO cards(title, tag, "column", created_at, column_since, position) VALUES ('written by 1.1', 'old', 'todo', 1700000000, 1700000000, 0)"#,
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO events(card_id, ts, actor, kind) VALUES (1, 1700000000, 'someone', 'created')", [])
        .unwrap();
    conn.execute("INSERT INTO config(key, value) VALUES ('wip', '4')", []).unwrap();
    conn
}

fn has_column(conn: &rusqlite::Connection, table: &str, col: &str) -> bool {
    let n: i64 = conn
        .query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name='{col}'"), [], |r| r.get(0))
        .unwrap();
    n > 0
}

fn backups_of(db: &Path) -> Vec<PathBuf> {
    let stem = format!("{}.before-", db.file_name().unwrap().to_str().unwrap());
    let mut v: Vec<PathBuf> = std::fs::read_dir(db.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_str().unwrap();
            n.starts_with(&stem) && n.ends_with(".bak")
        })
        .collect();
    v.sort();
    v
}

#[test]
fn an_older_board_is_copied_aside_before_its_schema_changes() {
    let s = Scratch::new();
    let db = s.db();
    let held = old_board(&db);
    let [wal, _] = sidecars(&db);
    assert!(std::fs::metadata(&wal).map(|m| m.len() > 0).unwrap_or(false), "the old board's data sits in a hot -wal");
    // what a plain copy of the .db file would have kept: not the card
    let naive = s.dir.path().join("naive-copy.db");
    std::fs::copy(&db, &naive).unwrap();
    let kept = rusqlite::Connection::open(&naive)
        .and_then(|c| c.query_row("SELECT COUNT(*) FROM cards", [], |r| r.get::<_, i64>(0)))
        .unwrap_or(0);
    assert_eq!(kept, 0, "a copy of the .db alone loses what is still in the -wal");

    // any command — a read will do — upgrades the board, and says where the copy went
    let o = ok(s.pinned(&["list"], &[("TB_NOW", "1790000000")]));
    assert!(out(&o).contains("written by 1.1"), "{}", out(&o));
    let baks = backups_of(&db);
    assert_eq!(baks.len(), 1, "one backup: {baks:?}");
    let bak = &baks[0];
    let e = errs(&o);
    let line = e.lines().find(|l| l.contains("backed up")).unwrap_or_else(|| panic!("says so: {e}"));
    assert!(line.starts_with("tb: ") && line.contains(bak.to_str().unwrap()) && line.contains("UPGRADING.md"), "{line}");
    // next to the board, named for the version that upgraded it and the (UTC) time; never `*.db`
    assert_eq!(
        bak.file_name().unwrap().to_str().unwrap(),
        format!("board.db.before-{}.20260921-141320.bak", env!("CARGO_PKG_VERSION"))
    );
    #[cfg(unix)]
    assert_eq!(mode(bak), 0o600, "a backup holds the same cards: it is private too");
    for f in sidecars(bak) {
        assert!(!f.exists(), "the backup is one complete file: {}", f.display());
    }

    // the backup is the board as the old version wrote it — card from the -wal included
    let b = rusqlite::Connection::open_with_flags(bak, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(b.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(), "ok");
    assert!(!has_column(&b, "cards", "reviewer") && !has_column(&b, "github_snapshot", "fails"), "schema untouched");
    let title: String = b.query_row("SELECT title FROM cards WHERE id=1", [], |r| r.get(0)).unwrap();
    assert_eq!(title, "written by 1.1");
    let wip: String = b.query_row("SELECT value FROM config WHERE key='wip'", [], |r| r.get(0)).unwrap();
    assert_eq!(wip, "4");
    drop(b);

    // … so the old binary can still open it: run what 1.1.0 runs on open, and its card query
    let restored = s.dir.path().join("restored/board.db");
    std::fs::create_dir_all(restored.parent().unwrap()).unwrap();
    std::fs::copy(bak, &restored).unwrap();
    let old = rusqlite::Connection::open(&restored).unwrap();
    let _: String = old.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0)).unwrap();
    old.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;").unwrap();
    old.execute_batch(SCHEMA_1_1).unwrap();
    assert!(has_column(&old, "cards", "blocked") && has_column(&old, "cards", "position"), "1.1.0 finds its columns");
    let titles: Vec<String> = old
        .prepare(&format!("SELECT {CARD_COLS_1_1} FROM cards ORDER BY id"))
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(titles, ["written by 1.1"]);
    old.execute("INSERT INTO events(card_id, ts, actor, kind, text) VALUES (1, 1, 'old-tb', 'note', 'still writable')", [])
        .unwrap();

    // the live board WAS upgraded, kept its card, and is not backed up again
    assert!(has_column(&held, "cards", "reviewer") && has_column(&held, "github_snapshot", "fails"));
    let o = ok(s.pinned(&["list"], &[("TB_NOW", "1790000999")]));
    assert!(out(&o).contains("written by 1.1"));
    assert!(!errs(&o).contains("backed up"), "{}", errs(&o));
    assert_eq!(backups_of(&db).len(), 1, "an up-to-date board is left alone");
}

#[test]
fn the_backup_is_reported_in_json_and_a_current_board_gets_none() {
    // --json: the same sentence, in `warnings`
    let s = Scratch::new();
    let _held = old_board(&s.db());
    let v = json(&ok(s.pinned(&["board", "--json"], &[])));
    let w = v["warnings"].as_array().cloned().unwrap_or_default();
    // (an old board is usually world-readable as well: that is a second, separate warning)
    assert_eq!(w.iter().filter(|x| x.as_str().unwrap().contains("backed up to")).count(), 1, "{w:?}");
    assert_eq!(v["columns"]["todo"][0]["title"], "written by 1.1");
    assert_eq!(v["wip"], 4);
    assert_eq!(backups_of(&s.db()).len(), 1);

    // a board this version created, written to and reopened: nothing to back up, ever
    let s = Scratch::new();
    ok(s.pinned(&["add", "plain: a card"], &[]));
    ok(s.pinned(&["take", "1"], &[]));
    let o = ok(s.pinned(&["board", "--json"], &[]));
    assert!(errs(&o).is_empty(), "{}", errs(&o));
    assert!(!out(&o).contains("warnings"));
    assert!(backups_of(&s.db()).is_empty());
    let only: Vec<String> = std::fs::read_dir(s.db().parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(only, ["board.db"], "no stray files next to the board");
}

#[cfg(unix)]
#[test]
fn a_backup_that_cannot_be_written_refuses_the_upgrade_and_changes_nothing() {
    let uid = Command::new("id").arg("-u").output().map(|o| out(&o).trim().to_string()).unwrap_or_default();
    if uid == "0" {
        return; // root writes anywhere: there is no way to make the backup fail
    }
    let s = Scratch::new();
    let db = s.db();
    let held = old_board(&db); // keeps the sidecars alive, so the board itself still opens
    let dir = db.parent().unwrap().to_path_buf();
    chmod(&dir, 0o500);
    let plain = s.pinned(&["list"], &[]);
    let as_json = s.pinned(&["board", "--json"], &[]);
    chmod(&dir, 0o700);

    assert!(!plain.status.success(), "refused: {}", out(&plain));
    let e = errs(&plain);
    assert!(e.contains("cannot back up") && e.contains(db.to_str().unwrap()), "{e}");
    assert!(e.contains("nothing was changed") && e.contains("run the command again"), "{e}");
    assert!(!as_json.status.success());
    let v = json(&as_json);
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("cannot back up"), "{v}");
    assert!(v["hint"].as_str().unwrap().contains("nothing was changed"), "{v}");

    // fail closed: the board is exactly what the old version wrote
    assert!(!has_column(&held, "cards", "reviewer") && !has_column(&held, "github_snapshot", "fails"));
    assert!(backups_of(&db).is_empty(), "no half-written backup left behind");
    // and once the directory is writable again the same command goes through
    ok(s.pinned(&["list"], &[]));
    assert!(has_column(&held, "cards", "reviewer"));
    assert_eq!(backups_of(&db).len(), 1);
}

/// A board exactly as tb 1.1.0 left it, with nothing holding it open.
fn old_board_at_rest(db: &Path) {
    drop(old_board(db));
}

#[test]
fn processes_racing_to_upgrade_a_board_write_exactly_one_backup_of_the_old_schema() {
    const RACERS: usize = 8;
    const ROUNDS: usize = 6;
    for round in 0..ROUNDS {
        let s = Scratch::new();
        let db = s.db();
        old_board_at_rest(&db);
        // every process opens the old board at the same moment
        let children: Vec<_> = (0..RACERS)
            .map(|_| {
                s.cmd()
                    .args(["list"])
                    .env("TB_DB", &db)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        let outs: Vec<Output> = children.into_iter().map(|c| c.wait_with_output().unwrap()).collect();
        for o in &outs {
            assert!(o.status.success(), "round {round}: every racer succeeds: {}", errs(o));
            assert!(out(o).contains("written by 1.1"), "round {round}: and sees the card: {}", out(o));
        }
        let baks = backups_of(&db);
        assert_eq!(baks.len(), 1, "round {round}: exactly one backup, however many racers: {baks:?}");
        let said = outs.iter().filter(|o| errs(o).contains("backed up")).count();
        assert_eq!(said, 1, "round {round}: and exactly one process says it wrote one");
        // that backup is always the board as the OLD version wrote it
        let b = rusqlite::Connection::open_with_flags(&baks[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert!(!has_column(&b, "cards", "reviewer") && !has_column(&b, "github_snapshot", "fails"), "round {round}: old schema");
        let n: i64 = b.query_row("SELECT COUNT(*) FROM cards", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "round {round}");
        assert_eq!(b.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(), "ok");
        // nothing half-written is left next to the board, and the board itself is upgraded
        let stray: Vec<String> = std::fs::read_dir(db.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".partial") || n.ends_with("-journal"))
            .collect();
        assert!(stray.is_empty(), "round {round}: {stray:?}");
        let live = rusqlite::Connection::open(&db).unwrap();
        assert!(has_column(&live, "cards", "reviewer"), "round {round}: the live board is upgraded");
    }
}

#[cfg(unix)]
#[test]
fn a_backup_killed_half_way_never_looks_like_a_backup() {
    // what a killed upgrade leaves behind: a `.partial`, never a file named `….bak`
    let s = Scratch::new();
    let db = s.db();
    old_board_at_rest(&db);
    let stamp = format!("board.db.before-{}.20260921-141320.bak", env!("CARGO_PKG_VERSION"));
    let partial = db.parent().unwrap().join(format!("{stamp}.partial"));
    std::fs::write(&partial, b"half a database").unwrap();
    ok(s.pinned(&["list"], &[("TB_NOW", "1790000000")]));
    let baks = backups_of(&db);
    assert_eq!(baks.len(), 1, "{baks:?}");
    assert_eq!(baks[0].file_name().unwrap().to_str().unwrap(), stamp);
    assert!(!partial.exists(), "the leftover was cleared, and the finished backup took its name");
    assert_eq!(mode(&baks[0]), 0o600);
    let b = rusqlite::Connection::open_with_flags(&baks[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(b.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(), "ok");
}

#[cfg(unix)]
fn symlink(to: impl AsRef<Path>, at: &Path) {
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(to, at).unwrap();
}

#[cfg(unix)]
fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

#[cfg(unix)]
#[test]
fn a_board_created_through_a_symbolic_link_is_private_too() {
    // TB_DB is a dangling link (absolute target): tb, not SQLite, creates the target
    let s = Scratch::new();
    let d = s.dir.path();
    std::fs::create_dir_all(d.join("real")).unwrap();
    let link = d.join("links/board.db");
    symlink(d.join("real/target.db"), &link);
    let mut c = s.cmd();
    c.env("TB_DB", &link);
    let o = ok(under_umask(c, "000", &["add", "plain: through a link"]).output().unwrap());
    assert!(!errs(&o).contains("open to other users"), "born private, nothing to report: {}", errs(&o));
    assert_eq!(mode(&d.join("real/target.db")), 0o600);
    assert!(is_link(&link), "the link is still a link");
    // the sidecars follow the real file, and they are private as well
    let held = rusqlite::Connection::open(d.join("real/target.db")).unwrap();
    let n: i64 = held.query_row("SELECT COUNT(*) FROM cards", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "the card is in the target");
    let mut c = s.cmd();
    c.env("TB_DB", &link);
    ok(under_umask(c, "000", &["add", "plain: second"]).output().unwrap());
    for f in sidecars(&d.join("real/target.db")) {
        assert!(f.exists(), "{}", f.display());
        assert_eq!(mode(&f), 0o600, "{}", f.display());
    }
    drop(held);

    // the boards folder: default.db is a dangling RELATIVE link, through a CHAIN of two
    let s = Scratch::new();
    let boards = s.home().join(".local/state/terminal-board/boards");
    std::fs::create_dir_all(s.home().join("vault")).unwrap();
    symlink("hop.db", &boards.join("default.db"));
    symlink("../../../../vault/default.db", &boards.join("hop.db"));
    ok(under_umask(s.cmd(), "000", &["add", "plain: in the vault"]).output().unwrap());
    assert_eq!(mode(&s.home().join("vault/default.db")), 0o600);
    assert!(is_link(&boards.join("default.db")) && is_link(&boards.join("hop.db")));
    assert!(out(&ok(s.cmd().args(["list"]).output().unwrap())).contains("in the vault"));
}

#[cfg(unix)]
#[test]
fn a_link_to_an_existing_board_is_followed_and_never_re_moded_on_the_quiet() {
    let s = Scratch::new();
    let d = s.dir.path();
    // an existing private board behind a link: just works, nothing to say
    ok(s.pinned(&["add", "plain: the real board"], &[]));
    let link = d.join("links/board.db");
    symlink(s.db(), &link);
    let o = ok(s.cmd().args(["list"]).env("TB_DB", &link).output().unwrap());
    assert!(out(&o).contains("the real board"));
    assert!(errs(&o).is_empty(), "{}", errs(&o));
    assert_eq!(mode(&s.db()), 0o600);

    // an existing 0644 board behind a link: reported under its REAL name, mode untouched
    chmod(&s.db(), 0o644);
    let o = ok(s.cmd().args(["list"]).env("TB_DB", &link).output().unwrap());
    let e = errs(&o);
    assert!(e.contains(s.db().to_str().unwrap()) && e.contains("(mode 0644)"), "{e}");
    assert_eq!(mode(&s.db()), 0o644);
    // tightened on request: the real file, and the link stays a link
    let said = out(&ok(s.cmd().args(["config", "file-mode", "private"]).env("TB_DB", &link).output().unwrap()));
    assert!(said.contains(s.db().to_str().unwrap()) && said.contains("is now private"), "{said}");
    assert_eq!(mode(&s.db()), 0o600);
    assert!(is_link(&link));
}

#[cfg(unix)]
#[test]
fn a_link_that_leads_nowhere_usable_is_refused_and_creates_nothing() {
    let s = Scratch::new();
    let d = s.dir.path();
    // into a directory that does not exist: tb creates a file through a link, never directories
    let link = d.join("links/board.db");
    symlink(d.join("no-such-dir/target.db"), &link);
    let o = s.cmd().args(["add", "plain: x"]).env("TB_DB", &link).output().unwrap();
    assert!(!o.status.success());
    let e = errs(&o);
    assert!(e.contains("is a symbolic link into a directory that does not exist") && e.contains("no-such-dir"), "{e}");
    assert!(e.contains("create that directory, or fix the link"), "{e}");
    assert!(!d.join("no-such-dir").exists(), "nothing was created");
    let o = s.cmd().args(["add", "plain: x", "--json"]).env("TB_DB", &link).output().unwrap();
    let v = json(&o);
    assert_eq!(v["ok"], false);
    assert!(v["hint"].as_str().unwrap().contains("fix the link"), "{v}");
    // once the directory exists the same link works, and the target is private
    std::fs::create_dir_all(d.join("no-such-dir")).unwrap();
    ok(s.cmd().args(["add", "plain: x"]).env("TB_DB", &link).output().unwrap());
    assert_eq!(mode(&d.join("no-such-dir/target.db")), 0o600);

    // a loop
    symlink("b.db", &d.join("loop/a.db"));
    symlink("a.db", &d.join("loop/b.db"));
    let o = s.cmd().args(["add", "plain: x"]).env("TB_DB", d.join("loop/a.db")).output().unwrap();
    assert!(!o.status.success());
    assert!(errs(&o).contains("is a symbolic link that never ends") && errs(&o).contains("fix the link"), "{}", errs(&o));
    let names: Vec<String> =
        std::fs::read_dir(d.join("loop")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert_eq!(names.len(), 2, "only the two links: {names:?}");
}
