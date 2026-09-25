//! Named boards, driven through the real binary with a temp HOME (no TB_DB).
#![allow(clippy::disallowed_methods, reason = "a test sleeps to stage a race or wait for another process; tb itself sleeps only through src/waits.rs")]
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home { dir: tempfile::tempdir().unwrap() }
    }
    fn state(&self) -> PathBuf {
        self.dir.path().join(".local/state/terminal-board")
    }
    /// Where the pre-rename `ttyboard` kept its state.
    fn old(&self) -> PathBuf {
        self.dir.path().join(".local/state/ttyboard")
    }
    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args)
            .env("HOME", self.dir.path())
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .env_remove("TB_DB")
            .env_remove("TB_BOARD")
            .env_remove("TTYBOARD_DB")
            .env_remove("TTYBOARD_BOARD")
            .env_remove("HERDR_AGENT_NAME");
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }
    fn run(&self, args: &[&str]) -> Output {
        self.run_env(args, &[])
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }
    fn db(&self, name: &str) -> PathBuf {
        self.state().join("boards").join(format!("{name}.db"))
    }
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn positional_board_routing() {
    let h = Home::new();
    let o = h.run(&["home", "add", "renew domain"]);
    assert!(o.status.success());
    assert!(err(&o).contains("created board 'home'"));
    assert!(h.db("home").exists());
    let o = h.run(&["home", "add", "second"]);
    assert!(!err(&o).contains("created"), "printed once");
    h.ok(&["work", "add", "ship it"]);
    assert!(h.ok(&["home", "list"]).contains("renew domain"));
    assert!(!h.ok(&["home", "list"]).contains("ship it"));
    assert!(h.ok(&["work", "next"]).contains("ship it"));
    // bare name, not a TTY: prints that board with its name in the header
    let out = h.ok(&["home"]);
    assert!(out.contains("TERMINAL BOARD · home · 2 cards"), "{out}");
    // no name: the default board, untouched so far
    assert!(!h.ok(&["list"]).contains("renew domain"));
    assert!(!h.db("default").exists(), "reads never create a board");
    h.ok(&["add", "plain"]);
    assert!(h.db("default").exists());
    assert!(h.ok(&[]).contains("TERMINAL BOARD · default · 1 cards"));
}

#[test]
fn precedence_positional_flag_env_default() {
    let h = Home::new();
    h.ok(&["-b", "work", "add", "via flag"]);
    assert!(h.db("work").exists());
    let o = h.run_env(&["add", "via env"], &[("TB_BOARD", "env")]);
    assert!(o.status.success());
    assert!(h.db("env").exists());
    // positional beats flag beats env
    let o = h.run_env(&["pos", "--board", "work", "add", "x"], &[("TB_BOARD", "env")]);
    assert!(o.status.success());
    assert!(h.db("pos").exists());
    let o = h.run_env(&["--board", "work", "list"], &[("TB_BOARD", "env")]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("via flag"));
}

#[test]
fn names_are_validated_and_commands_are_not_boards() {
    let h = Home::new();
    let o = h.run(&["Home!", "list"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("not a command or a valid board name"), "{}", err(&o));
    let o = h.run(&["-b", "add", "list"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("'add' is a command"), "{}", err(&o));
    let o = h.run_env(&["list"], &[("TB_BOARD", "next")]);
    assert!(!o.status.success() && err(&o).contains("'next' is a command"));
    // `tb add` is always the subcommand
    h.ok(&["add", "real card"]);
    assert!(!h.db("add").exists());
    assert!(h.ok(&["list"]).contains("real card"));
    let long = "a".repeat(33);
    assert!(!h.run(&[&long, "list"]).status.success());
}

#[test]
fn legacy_board_is_migrated() {
    // the oldest layout: single files in the pre-rename ttyboard dir
    let h = Home::new();
    std::fs::create_dir_all(h.old()).unwrap();
    {
        let s = terminal_board::store::Store::open(&h.old().join("board.db")).unwrap();
        s.add("from the old days", "", &[], "me").unwrap();
        let d = terminal_board::store::Store::open(&h.old().join("demo.db")).unwrap();
        d.add("old demo", "", &[], "me").unwrap();
    }
    let o = h.run(&["list"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("from the old days"), "{}", err(&o));
    assert!(!h.old().join("board.db").exists() && h.db("default").exists());
    assert!(!h.old().join("board.db-wal").exists());
    assert!(h.db("demo").exists() && !h.old().join("demo.db").exists());
    assert!(h.ok(&["-b", "demo", "list"]).contains("old demo"));
    // a second legacy file never clobbers an existing board
    let s = terminal_board::store::Store::open(&h.old().join("board.db")).unwrap();
    s.add("newer legacy", "", &[], "me").unwrap();
    drop(s);
    let out = h.ok(&["list"]);
    assert!(out.contains("from the old days") && !out.contains("newer legacy"));
    assert!(Path::new(&h.old().join("board.db")).exists());
}

#[test]
fn ttyboard_boards_move_to_terminal_board() {
    let h = Home::new();
    std::fs::create_dir_all(h.old().join("boards")).unwrap();
    {
        let s = terminal_board::store::Store::open(&h.old().join("boards/home.db")).unwrap();
        s.add("home card", "", &[], "me").unwrap();
        let s = terminal_board::store::Store::open(&h.old().join("boards/default.db")).unwrap();
        s.add("default card", "", &[], "me").unwrap();
    }
    assert!(!h.state().exists());
    let o = h.run(&["home", "list"]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("home card"));
    let note = err(&o);
    assert!(note.contains("moved 2 board(s)") && note.lines().count() == 1, "one-line note: {note}");
    assert!(h.db("home").exists() && h.db("default").exists());
    assert!(!h.old().join("boards/home.db").exists(), "moved, not copied");
    assert!(h.ok(&["list"]).contains("default card"));
    // second run: nothing more to move, no note
    let o = h.run(&["list"]);
    assert!(!err(&o).contains("moved"));
    // the old env names still work as fallbacks; the new ones win
    let o = h.run_env(&["list"], &[("TTYBOARD_BOARD", "home")]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("home card"));
    let o = h.run_env(&["list"], &[("TTYBOARD_BOARD", "home"), ("TB_BOARD", "default")]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("default card"));
}

#[test]
fn boards_listing() {
    let h = Home::new();
    assert!(h.ok(&["boards"]).contains("no boards yet"));
    h.ok(&["add", "a"]);
    h.ok(&["add", "b"]);
    h.ok(&["next"]);
    h.ok(&["home", "add", "c"]);
    let out = h.ok(&["boards"]);
    let def = out.lines().find(|l| l.contains("default")).unwrap();
    assert!(def.starts_with('*') && def.contains("todo 1") && def.contains("doing 1"), "{out}");
    let home = out.lines().find(|l| l.contains("home")).unwrap();
    assert!(home.starts_with(' ') && home.contains("todo 1"), "{out}");
    let v: serde_json::Value = serde_json::from_str(&h.ok(&["boards", "--json"])).unwrap();
    let names: Vec<_> = v.as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, ["default", "home"]);
    assert_eq!(v[0]["default"], true);
    assert_eq!(v[0]["doing"], 1);
}

// --- archiving a board (gh#80) -------------------------------------------------------
// A board is a SQLite file; nothing in tb removed one, so retiring a board meant moving
// files by hand. `tb boards archive NAME` moves it into `archive/`, holding the board's own
// lock (#112, `store::lock_for_move`) across the whole move; `restore` moves it back with
// `store::link_into_place`, which refuses rather than ever clobbering. Nothing is deleted.

impl Home {
    fn archive_dir(&self) -> PathBuf {
        self.state().join("archive")
    }
    /// A `tb` command against this HOME, ready to spawn (the tests above wait for output;
    /// these need the child itself, for races and for a held-open second process).
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args)
            .env("HOME", self.dir.path())
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .env_remove("TB_DB")
            .env_remove("TB_BOARD")
            .env_remove("TTYBOARD_DB")
            .env_remove("TTYBOARD_BOARD")
            .env_remove("HERDR_AGENT_NAME");
        c
    }
    /// Every archived file for `name`, oldest first.
    fn archives(&self, name: &str) -> Vec<PathBuf> {
        let mut hits: Vec<PathBuf> = std::fs::read_dir(self.archive_dir())
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("{name}@")))
                    .collect()
            })
            .unwrap_or_default();
        hits.sort();
        hits
    }
    /// The one archived file for `name` (the test archives a name at most once).
    fn archived(&self, name: &str) -> PathBuf {
        let mut hits = self.archives(name);
        assert_eq!(hits.len(), 1, "one archive of '{name}': {hits:?}");
        hits.pop().unwrap()
    }
    /// Titles on the board `name`, live only (never through `Store::open` on a path that
    /// might not exist — that would CREATE it, the exact defect under test).
    fn titles(&self, name: &str) -> Vec<String> {
        titles_in_file(&self.db(name))
    }
}

/// Titles in the board file at `path`, or `[]` when it is not there — checked with `exists()`
/// FIRST: `Store::open` creates what it does not find, which must never be a side effect of a
/// test assertion helper (this is exactly defect 1 from PR #88's first review round).
fn titles_in_file(path: &Path) -> Vec<String> {
    if !path.exists() {
        return Vec::new();
    }
    let Ok(s) = terminal_board::store::Store::open(path) else { return Vec::new() };
    s.snapshot()
        .map(|snap| terminal_board::store::COLUMNS.iter().flat_map(|c| snap.in_column(c)).map(|c| c.title.clone()).collect())
        .unwrap_or_default()
}

fn sizes(default_rounds: usize) -> (Vec<usize>, usize) {
    let racers = std::env::var("TB_RACE_RACERS")
        .ok()
        .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![2usize, 4]);
    let rounds = std::env::var("TB_RACE_ROUNDS").ok().and_then(|s| s.parse().ok()).unwrap_or(default_rounds);
    (racers, rounds)
}

#[test]
fn archive_moves_a_board_out_and_restore_brings_it_back() {
    let h = Home::new();
    h.ok(&["add", "keep me"]);
    h.ok(&["scratch", "add", "one"]);
    h.ok(&["scratch", "add", "two"]);
    h.ok(&["scratch", "take", "1"]);
    let before = std::fs::read(h.db("scratch")).unwrap();

    let out = h.ok(&["boards", "archive", "scratch"]);
    assert!(out.contains("tb boards restore scratch"), "no restore line:\n{out}");
    assert!(!h.db("scratch").exists(), "the board file is still in boards/");
    for ext in ["-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{ext}", h.db("scratch").display()));
        assert!(!p.exists(), "sidecar left behind: {}", p.display());
    }
    let arc = h.archived("scratch");
    assert!(arc.is_file(), "nothing in archive/");
    let stored = std::fs::read(&arc).unwrap();
    assert_eq!(stored, before, "archiving rewrote the board instead of moving it");

    // it is gone from `tb boards`, and `tb scratch list` says so instead of showing a phantom
    let listed = h.ok(&["boards"]);
    assert!(!listed.contains("scratch"), "archived board still listed:\n{listed}");
    let v: serde_json::Value = serde_json::from_str(&h.ok(&["boards", "--json"])).unwrap();
    assert!(v.as_array().unwrap().iter().all(|b| b["name"] != "scratch"), "{v}");
    assert!(!h.run(&["scratch", "list"]).status.success(), "an archived board still opens");

    // --archived lists it, with the counts it had
    let a = h.ok(&["boards", "--archived"]);
    assert!(a.contains("scratch") && a.contains("todo 1") && a.contains("doing 1"), "{a}");
    let v: serde_json::Value = serde_json::from_str(&h.ok(&["boards", "--archived", "--json"])).unwrap();
    assert_eq!(v[0]["name"], "scratch");
    assert_eq!(v[0]["todo"], 1);
    assert_eq!(v[0]["doing"], 1);
    assert_eq!(v[0]["path"], arc.display().to_string());
    assert!(v[0]["archived_at"].as_str().unwrap().len() == 16, "{v}");

    // the restore line it printed is the one that works
    let out = h.ok(&["boards", "restore", "scratch"]);
    assert!(out.contains("scratch"), "{out}");
    assert!(!arc.exists(), "restore moved it, the archive copy is gone");
    assert_eq!(std::fs::read(h.db("scratch")).unwrap(), stored, "restored byte-for-byte");
    assert!(h.ok(&["scratch", "list"]).contains("two"));
    assert!(h.ok(&["boards"]).contains("scratch"));
    assert!(h.ok(&["boards", "--archived"]).contains("no archived boards"));
}

/// The busy refusal must come from an acquired lock, NOT from the presence of `-wal`/`-shm`:
/// those survive a crash, so a sidecar check refuses forever after one.
#[test]
fn archive_ignores_sidecar_files_nobody_holds() {
    let h = Home::new();
    h.ok(&["scratch", "add", "one"]);
    for ext in ["-wal", "-shm"] {
        std::fs::write(format!("{}{ext}", h.db("scratch").display()), b"").unwrap();
    }
    let o = h.run(&["boards", "archive", "scratch"]);
    assert!(o.status.success(), "stale sidecars caused a false refusal: {}", err(&o));
    assert!(h.archived("scratch").is_file());
}

/// The real thing: a second PROCESS holding the board's SQLite file open via `Store::open`
/// (`crate::lock`'s SHARED half). `tb NAME watch` keeps one connection for as long as it
/// runs, exactly as a `tb` window does.
#[test]
fn archive_refuses_while_another_process_holds_the_board() {
    let h = Home::new();
    h.ok(&["scratch", "add", "one"]);

    let mut child =
        h.cmd(&["scratch", "watch", "--json", "--events"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    // wait until it has actually opened the database (the -shm appears on connect). This is
    // only a start signal for the test — the refusal below must not depend on it.
    let shm = PathBuf::from(format!("{}-shm", h.db("scratch").display()));
    let start = std::time::Instant::now();
    while !shm.exists() && start.elapsed() < std::time::Duration::from_secs(10) {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(shm.exists(), "the second process never opened the board");

    let o = h.cmd(&["boards", "archive", "scratch"]).env("TB_LOCK_WAIT_MS", "300").output().unwrap();
    let e = err(&o);
    assert!(!o.status.success(), "archived a board another process holds open");
    assert!(e.contains("scratch") && e.contains("open in another process"), "which board, and what to do: {e}");
    assert!(h.db("scratch").is_file(), "the board moved anyway");
    assert!(!h.archive_dir().exists() || std::fs::read_dir(h.archive_dir()).unwrap().count() == 0);
    // --json says the same thing in the documented error shape, with the stable code
    let o = h.cmd(&["boards", "archive", "scratch", "--json"]).env("TB_LOCK_WAIT_MS", "300").output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["code"], "board_busy");
    assert!(v["error"].as_str().unwrap().contains("open in another process"), "{v}");
    assert!(v["hint"].as_str().unwrap().contains("close process") || v["hint"].as_str().unwrap().contains("close whatever process"), "{v}");

    // once it lets go, the same command works — well under the wait bound: the lock is the
    // kernel's, so it releases with the process, not on a timer
    child.kill().unwrap();
    child.wait().unwrap();
    let start = std::time::Instant::now();
    loop {
        let o = h.run(&["boards", "archive", "scratch"]);
        if o.status.success() {
            break;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "still refused after the holder died: {}", err(&o));
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(h.archived("scratch").is_file());
}

#[test]
fn archive_and_restore_refuse_the_default_board_tb_db_and_unknown_names() {
    let h = Home::new();
    h.ok(&["add", "a"]);
    h.ok(&["scratch", "add", "b"]);

    let e = err(&h.run(&["boards", "archive", "default"]));
    assert!(e.contains("bare 'tb' opens"), "{e}");
    assert!(h.db("default").is_file());
    // TB_BOARD moves which board that is
    let e = err(&h.run_env(&["boards", "archive", "scratch"], &[("TB_BOARD", "scratch")]));
    assert!(e.contains("bare 'tb' opens"), "{e}");
    assert!(h.run_env(&["boards", "archive", "default"], &[("TB_BOARD", "scratch")]).status.success());
    h.ok(&["boards", "restore", "default"]);

    let pinned = h.dir.path().join("pinned.db");
    let e = err(&h.run_env(&["boards", "archive", "scratch"], &[("TB_DB", pinned.to_str().unwrap())]));
    assert!(e.contains("TB_DB"), "{e}");

    let e = err(&h.run(&["boards", "archive", "nope"]));
    assert!(e.contains("no board 'nope'") && e.contains("scratch"), "{e}");
    let e = err(&h.run(&["boards", "archive"]));
    assert!(e.contains("needs a board name"), "{e}");
    let e = err(&h.run(&["boards", "wipe", "scratch"]));
    assert!(e.contains("unknown") && e.contains("tb boards archive NAME"), "{e}");
    // `rm` is a card verb, deliberately not a board one
    let e = err(&h.run(&["boards", "rm", "scratch"]));
    assert!(e.contains("unknown"), "{e}");
    assert!(h.db("scratch").is_file(), "nothing was removed");
}

#[test]
fn restore_refuses_when_a_live_board_of_that_name_exists() {
    let h = Home::new();
    h.ok(&["scratch", "add", "archived one"]);
    h.ok(&["boards", "archive", "scratch"]);
    let arc = h.archived("scratch");
    h.ok(&["scratch", "add", "new one"]);

    let o = h.run(&["boards", "restore", "scratch"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("already exists"), "{}", err(&o));
    assert!(arc.is_file(), "the archive was consumed by a refused restore");
    assert!(h.ok(&["scratch", "list"]).contains("new one"), "the live board was overwritten");

    let e = err(&h.run(&["boards", "restore", "nothing"]));
    assert!(e.contains("no archived board 'nothing'") && e.contains("scratch"), "{e}");
}

/// A stray `-wal` left beside the destination (a pre-#80 hand-move that only moved the
/// `.db`) refuses restore exactly like a live `.db` would, rather than letting SQLite replay
/// a foreign WAL over the restored file on its next open.
#[test]
fn restore_refuses_when_a_stray_wal_sits_at_the_destination() {
    let h = Home::new();
    h.ok(&["scratch", "add", "archived one"]);
    h.ok(&["boards", "archive", "scratch"]);
    std::fs::create_dir_all(h.db("scratch").parent().unwrap()).unwrap();
    std::fs::write(format!("{}-wal", h.db("scratch").display()), b"not tb's").unwrap();

    let o = h.run(&["boards", "restore", "scratch"]);
    assert!(!o.status.success(), "restored over a stray -wal");
    assert!(err(&o).contains("already exists"), "{}", err(&o));
    assert!(!h.db("scratch").exists(), "restore partially landed the .db");
    assert!(h.archived("scratch").is_file(), "the archive was consumed anyway");
}

/// The `B` picker offers exactly `boards::picker_rows(actor)`; an archived board leaves the
/// boards directory, so it stops being offered there too.
#[test]
fn the_board_picker_stops_offering_an_archived_board() {
    let h = Home::new();
    h.ok(&["add", "a"]);
    h.ok(&["scratch", "add", "b"]);
    // this test (alone in this file) reads the process HOME, as the picker does
    std::env::set_var("HOME", h.dir.path());
    for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD"] {
        std::env::remove_var(k);
    }
    let names = || terminal_board::boards::picker_rows("t").unwrap().iter().map(|b| b.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(), ["default", "scratch"]);
    h.ok(&["boards", "archive", "scratch"]);
    assert_eq!(names(), ["default"], "the picker still offers an archived board");
    h.ok(&["boards", "restore", "scratch"]);
    assert_eq!(names(), ["default", "scratch"]);
}

/// Two `tb boards archive NAME` at once must leave the boards directory clean. `lock_for_move`
/// is exclusive and BLOCKS rather than polls, so exactly one racer can ever be inside the
/// critical section: there is no "both locked each other out" outcome to tolerate any more
/// (unlike the SQLite-probe design two earlier rounds of this feature shipped and had sent
/// back).
#[test]
fn concurrent_archives_leave_no_empty_board_behind() {
    const ROUNDS: usize = 30;
    const RACERS: usize = 3;
    for round in 0..ROUNDS {
        let h = Home::new();
        h.ok(&["scratch", "add", "one"]);
        h.ok(&["scratch", "add", "two"]);
        let racers: Vec<_> = (0..RACERS)
            .map(|_| h.cmd(&["boards", "archive", "scratch"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap())
            .collect();
        let out: Vec<_> = racers.into_iter().map(|c| c.wait_with_output().unwrap()).collect();

        let won = out.iter().filter(|o| o.status.success()).count();
        assert_eq!(won, 1, "round {round}: {won} of {RACERS} archives won (expected exactly one — blocking, not polling)");
        for o in out.iter().filter(|o| !o.status.success()) {
            let e = String::from_utf8_lossy(&o.stderr).to_string();
            assert!(e.contains("no board 'scratch'"), "round {round}: a loser failed for an unexpected reason: {e}");
        }
        assert_eq!(h.archives("scratch").len(), 1, "round {round}: one archived file for the one winner");
        for ext in ["-wal", "-shm"] {
            let p = PathBuf::from(format!("{}{ext}", h.db("scratch").display()));
            assert!(!p.exists(), "round {round}: sidecar left behind: {}", p.display());
        }
        assert!(!h.db("scratch").exists(), "round {round}: an archive race re-created the board file");
        let listed = h.ok(&["boards"]);
        assert!(!listed.contains("scratch"), "round {round}: still listed:\n{listed}");
        let restored = h.ok(&["boards", "restore", "scratch"]);
        assert!(restored.contains("scratch"), "round {round}: {restored}");
        let list = h.ok(&["scratch", "list"]);
        assert!(list.contains("one") && list.contains("two"), "round {round}: cards lost:\n{list}");
    }
}

/// `tb boards --archived` reads counts; it must not leave anything beside the archived file.
#[test]
fn listing_archived_boards_writes_nothing() {
    let h = Home::new();
    h.ok(&["scratch", "add", "one"]);
    h.ok(&["boards", "archive", "scratch"]);
    let arc = h.archived("scratch");
    let before = std::fs::read(&arc).unwrap();

    for _ in 0..3 {
        assert!(h.ok(&["boards", "--archived"]).contains("todo 1"));
        assert!(h.ok(&["boards", "--archived", "--json"]).contains("\"todo\""));
    }
    for ext in ["-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{ext}", arc.display()));
        assert!(!p.exists(), "--archived left {} behind", p.display());
    }
    assert_eq!(std::fs::read(&arc).unwrap(), before, "--archived changed the archived board");
    let names: Vec<String> = std::fs::read_dir(h.archive_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names.len(), 1, "archive/ holds only the board file: {names:?}");
}

/// A rich board — cards in every column, notes, a checklist, a blocked card, a due date, a
/// link, non-default settings — round-trips archive -> restore identically: not just card
/// counts, the full JSON dump.
#[test]
fn a_rich_board_round_trips_through_archive_and_restore() {
    let h = Home::new();
    h.ok(&["scratch", "add", "todo card", "-d", "desc", "--check", "step one"]);
    h.ok(&["scratch", "add", "doing card"]);
    h.ok(&["scratch", "take", "2"]);
    h.ok(&["scratch", "note", "2", "in progress"]);
    h.ok(&["scratch", "check", "1", "1"]);
    h.ok(&["scratch", "block", "2", "#1"]);
    h.ok(&["scratch", "add", "review card", "--due", "2026-12-25"]);
    h.ok(&["scratch", "take", "3"]);
    h.ok(&["scratch", "move", "3", "review"]);
    h.ok(&["scratch", "link", "3", "https://example.com/evidence", "--label", "verdict"]);
    h.ok(&["scratch", "add", "done card"]);
    h.ok(&["scratch", "take", "4"]);
    h.ok(&["scratch", "done", "4"]);
    h.ok(&["scratch", "config", "wip", "5"]);
    h.ok(&["scratch", "config", "theme", "light"]);

    let before = h.ok(&["scratch", "board", "--json"]);
    let before_config = h.ok(&["scratch", "config"]);

    h.ok(&["boards", "archive", "scratch"]);
    h.ok(&["boards", "restore", "scratch"]);

    let after = h.ok(&["scratch", "board", "--json"]);
    let after_config = h.ok(&["scratch", "config"]);
    assert_eq!(before, after, "the board's JSON dump changed across archive -> restore");
    assert_eq!(before_config, after_config, "settings changed across archive -> restore");
}

/// The defect PR #88's SECOND review round found: `tb boards restore NAME` racing `tb NAME
/// add` (which re-creates whatever is at that path) must never destroy the restored cards —
/// `restore` holds `lock_for_move` from its very first look at the destination through the
/// final `link_into_place`, so an `add` either lands on the freshly-restored board (both
/// survive) or is made to wait until the lock is free and finds the destination already
/// there (also both survive); it must never see a half-finished restore.
#[test]
fn restore_races_add_and_never_loses_a_card() {
    let (racer_counts, rounds) = sizes(20);
    for &n in &racer_counts {
        for round in 0..rounds {
            let h = Home::new();
            h.ok(&["scratch", "add", "keep-one"]);
            h.ok(&["scratch", "add", "keep-two"]);
            h.ok(&["boards", "archive", "scratch"]);
            let nr = (n / 2).max(1);
            let mut kids: Vec<(bool, String, std::process::Child)> = Vec::new();
            for _ in 0..nr {
                kids.push((false, String::new(), h.cmd(&["boards", "restore", "scratch"]).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap()));
            }
            for j in 0..n.saturating_sub(nr).max(1) {
                let title = format!("r{round}n{n}w{j}");
                kids.push((
                    true,
                    title.clone(),
                    h.cmd(&["scratch", "add", &title]).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap(),
                ));
            }
            let mut committed = Vec::new();
            for (is_add, title, k) in kids {
                let o = k.wait_with_output().unwrap();
                if is_add {
                    assert!(o.status.success(), "an add crashed instead of refusing cleanly: {}", String::from_utf8_lossy(&o.stderr));
                    committed.push(title);
                } else if !o.status.success() {
                    let e = String::from_utf8_lossy(&o.stderr).to_string();
                    assert!(e.contains("already exists"), "round {round} n={n}: restore failed for an unexpected reason: {e}");
                }
            }
            // an acknowledged add is never lost: it either landed on the freshly-restored
            // board, or `add` created the slot itself — both survive live, by construction
            let live = h.titles("scratch");
            for t in &committed {
                assert!(live.contains(t), "round {round} n={n}: an acknowledged add vanished: {t}\nlive={live:?}");
            }
            // the seed cards are never destroyed: EITHER a restore won and they are live, OR
            // an `add` won and `restore` correctly refused rather than clobber it — they stay
            // exactly where `archive` put them, untouched. Losing them from BOTH places would
            // be the actual defect (PR #88's second review round: 55 of 55 dangerous rounds).
            let live_has_seed = live.contains(&"keep-one".to_string()) && live.contains(&"keep-two".to_string());
            let archived_has_seed = h.archives("scratch").iter().any(|p| {
                let t = titles_in_file(p);
                t.contains(&"keep-one".to_string()) && t.contains(&"keep-two".to_string())
            });
            assert!(
                live_has_seed || archived_has_seed,
                "round {round} n={n}: seed cards lost entirely — not live, not archived. live={live:?}"
            );
        }
    }
}

/// The defect PR #88's SECOND review round found on the other side: `tb boards archive NAME`
/// racing ordinary short-lived writers must never lose an acknowledged write, and must never
/// resurrect a live board after a winning archive (`lock_for_move` blocks EVERY `Store::open`
/// on the same name from before its exists-check to after its last write — there is no window
/// left for a writer to slip in between).
#[test]
fn archive_races_short_writers_and_never_loses_a_committed_write() {
    let (racer_counts, rounds) = sizes(20);
    for &n in &racer_counts {
        for round in 0..rounds {
            let h = Home::new();
            h.ok(&["scratch", "add", "one"]);
            h.ok(&["scratch", "add", "two"]);
            let na = (n / 2).max(1);
            let nw = n.saturating_sub(na).max(1);
            let tb = env!("CARGO_BIN_EXE_tb");
            let mut kids: Vec<(bool, std::process::Child)> = Vec::new();
            for _ in 0..na {
                kids.push((
                    false,
                    h.cmd(&["boards", "archive", "scratch"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap(),
                ));
            }
            for j in 0..nw {
                let script = format!(
                    "for i in $(seq 1 6); do \"$TB\" scratch add \"w{round}x{n}x{j}x$i\" --as rv >/dev/null 2>&1 && echo \"w{round}x{n}x{j}x$i\"; done; true"
                );
                let mut writer = Command::new("sh");
                writer
                    .arg("-c")
                    .arg(script)
                    .env("HOME", h.dir.path())
                    .env("TB", tb)
                    .env("TB_AS", "rv")
                    .env("TB_NO_HERDR", "1")
                    .env_remove("TB_DB")
                    .env_remove("TB_BOARD")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());
                kids.push((true, writer.spawn().unwrap()));
            }
            let mut committed = Vec::new();
            let mut archive_wins = 0;
            for (is_writer, k) in kids {
                let o = k.wait_with_output().unwrap();
                if is_writer {
                    committed.extend(String::from_utf8_lossy(&o.stdout).lines().map(str::to_string));
                } else if o.status.success() {
                    archive_wins += 1;
                }
            }
            let mut found: Vec<String> = h.titles("scratch");
            for p in h.archives("scratch") {
                found.extend(titles_in_file(&p));
            }
            for t in &committed {
                assert!(found.contains(t), "round {round} n={n}: lost a committed write: {t}\nfound={found:?}");
            }
            for p in std::iter::once(h.db("scratch")).chain(h.archives("scratch")) {
                if p.exists() {
                    assert_ne!(std::fs::metadata(&p).unwrap().len(), 0, "round {round} n={n}: zero-byte file left at {}", p.display());
                }
            }
            // the board starts live, so at least one archiver must win it (a writer can only
            // ever recreate the slot AFTER a winning archive vacates it, never prevent the
            // first win outright — `lock_for_move` blocks every `Store::open` on the name
            // until an archive already in progress finishes)
            assert!(archive_wins >= 1, "round {round} n={n}: no archiver ever won a live board");
        }
    }
}

