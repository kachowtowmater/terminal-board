//! `tb boards delete NAME`: removes an ARCHIVED board for good, and refuses everything else —
//! driven through the real binary with a temp HOME (no TB_DB).
#![allow(clippy::disallowed_methods, reason = "a test sleeps to stage a race or wait for another process; tb itself sleeps only through src/waits.rs")]
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Everything that makes a process read as an agent (`actors::Identity::resolve`): removed, so
/// the person-path tests hold whatever shell runs the suite.
const AGENT_VARS: [&str; 11] = [
    "TB_HARNESS",
    "TTYBOARD_HARNESS",
    "AI_AGENT",
    "OMPCODE",
    "CLAUDECODE",
    "CODEX_SESSION_ID",
    "CODEX_THREAD_ID",
    "CODEX_SANDBOX",
    "CODEX_CI",
    "HERDR_PANE_ID",
    "HERDR_ENV",
];

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
    fn db(&self, name: &str) -> PathBuf {
        self.state().join("boards").join(format!("{name}.db"))
    }
    fn archive_dir(&self) -> PathBuf {
        self.state().join("archive")
    }
    /// Files in the archive directory whose name starts with `name@`, sorted.
    fn archive_files(&self, name: &str) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(self.archive_dir())
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("{name}@")))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }
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
            .env_remove("HERDR_AGENT_NAME")
            .stdin(Stdio::null());
        for k in AGENT_VARS {
            c.env_remove(k);
        }
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }
    /// A board `scratch` with one card, archived.
    fn archived_scratch(&self) -> PathBuf {
        self.ok(&["scratch", "add", "one"]);
        self.ok(&["boards", "archive", "scratch"]);
        let files = self.archive_files("scratch");
        assert_eq!(files.len(), 1, "{files:?}");
        files[0].clone()
    }
}

/// The `{ok:false, code}` of a `--json` refusal, checked to be a refusal.
fn refused(o: &Output) -> (String, String) {
    assert!(!o.status.success(), "not refused: {}", String::from_utf8_lossy(&o.stdout));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|_| panic!("no JSON refusal: {} {}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)));
    assert_eq!(v["ok"], false, "{v}");
    // the message, whole: `--json` splits it into `error` and the `hint` after it
    let text = format!("{} {}", v["error"].as_str().unwrap_or(""), v["hint"].as_str().unwrap_or(""));
    (v["code"].as_str().unwrap_or("").to_string(), text)
}

fn exists(p: &Path) -> bool {
    p.exists()
}

#[test]
fn deleting_an_archived_board_removes_its_files_and_says_which() {
    let h = Home::new();
    h.ok(&["keep", "add", "stays"]);
    let file = h.archived_scratch();
    // sidecars an earlier reader left beside the archive go too
    let wal = PathBuf::from(format!("{}-wal", file.display()));
    let shm = PathBuf::from(format!("{}-shm", file.display()));
    std::fs::write(&wal, b"").unwrap();
    std::fs::write(&shm, b"").unwrap();

    let out = h.ok(&["boards", "delete", "scratch", "--yes"]);
    for p in [&file, &wal, &shm] {
        assert!(!exists(p), "{} is still there", p.display());
        assert!(out.contains(&p.display().to_string()), "the output does not name {}:\n{out}", p.display());
    }
    assert!(h.archive_files("scratch").is_empty(), "left behind: {:?}", h.archive_files("scratch"));
    let listed: serde_json::Value = serde_json::from_str(&h.ok(&["boards", "--archived", "--json"])).unwrap();
    assert_eq!(listed, serde_json::json!([]), "still listed as archived");
    // the other boards are untouched
    assert!(h.db("keep").is_file());

    // --json: the removed files, and no backups to keep
    let file = h.archived_scratch();
    let v: serde_json::Value = serde_json::from_str(&h.ok(&["boards", "delete", "scratch", "--yes", "--json"])).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["board"], "scratch");
    assert_eq!(v["removed"], serde_json::json!([file.display().to_string()]), "{v}");
    assert_eq!(v["kept_backups"], serde_json::json!([]), "{v}");
    assert!(!exists(&file));
}

#[test]
fn backups_are_kept_unless_asked_for() {
    let h = Home::new();
    h.archived_scratch();
    // a schema-upgrade backup the board left beside its old place
    let bak = h.state().join("boards").join("scratch.db.before-2.0.0.20260101T000000Z.bak");
    std::fs::write(&bak, b"backup").unwrap();
    let v: serde_json::Value = serde_json::from_str(&h.ok(&["boards", "delete", "scratch", "--yes", "--json"])).unwrap();
    assert_eq!(v["kept_backups"], serde_json::json!([bak.display().to_string()]), "{v}");
    assert!(exists(&bak), "a backup was deleted without --backups");

    h.archived_scratch();
    let v: serde_json::Value =
        serde_json::from_str(&h.ok(&["boards", "delete", "scratch", "--yes", "--backups", "--json"])).unwrap();
    assert!(v["removed"].as_array().unwrap().iter().any(|p| p == &bak.display().to_string()), "{v}");
    assert!(!exists(&bak), "--backups kept the backup");
}

#[test]
fn a_live_board_is_refused_archive_it_first() {
    let h = Home::new();
    h.ok(&["scratch", "add", "one"]);
    let (code, error) = refused(&h.run(&["boards", "delete", "scratch", "--yes", "--json"]));
    assert_eq!(code, "board_live", "{error}");
    assert!(error.contains("archive it first") && error.contains("tb boards archive scratch"), "{error}");
    assert!(h.db("scratch").is_file(), "the live board was touched");
    // a name nobody archived
    let (code, _) = refused(&h.run(&["boards", "delete", "nothing", "--yes", "--json"]));
    assert_eq!(code, "no_archive");
}

#[test]
fn it_asks_first_and_needs_yes_when_it_cannot() {
    let h = Home::new();
    let file = h.archived_scratch();
    // stdin is not a terminal here: no question can be asked, so nothing is deleted
    let o = h.run(&["boards", "delete", "scratch"]);
    assert!(!o.status.success(), "deleted without asking");
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("--yes"), "the refusal names the way through: {e}");
    assert!(exists(&file), "deleted without --yes");
    let (code, _) = refused(&h.run(&["boards", "delete", "scratch", "--json"]));
    assert_eq!(code, "confirm_required");
    assert!(exists(&file));
}

#[test]
fn the_board_plain_tb_opens_is_refused() {
    let h = Home::new();
    let file = h.archived_scratch();
    let o = h.cmd(&["boards", "delete", "scratch", "--yes", "--json"]).env("TB_BOARD", "scratch").output().unwrap();
    let (code, _) = refused(&o);
    assert_eq!(code, "default_board");
    assert!(exists(&file));
}

#[test]
fn an_agent_is_refused() {
    let h = Home::new();
    let file = h.archived_scratch();
    for agent in [("CLAUDECODE", "1"), ("AI_AGENT", "pi"), ("TB_HARNESS", "codex")] {
        let o = h.cmd(&["boards", "delete", "scratch", "--yes", "--json"]).env(agent.0, agent.1).output().unwrap();
        let (code, error) = refused(&o);
        assert_eq!(code, "person_only", "{agent:?}: {error}");
        assert!(exists(&file), "{agent:?} deleted it");
    }
}

#[test]
fn a_board_another_tb_has_open_is_refused() {
    let h = Home::new();
    let file = h.archived_scratch();
    // another tb opens the archived file itself
    let mut child = h
        .cmd(&["watch", "--json", "--events"])
        .env("TB_DB", &file)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let shm = PathBuf::from(format!("{}-shm", file.display()));
    let start = std::time::Instant::now();
    while !shm.exists() && start.elapsed() < std::time::Duration::from_secs(10) {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(shm.exists(), "the second process never opened the board");
    let o = h.cmd(&["boards", "delete", "scratch", "--yes", "--json"]).env("TB_LOCK_WAIT_MS", "300").output().unwrap();
    let (code, error) = refused(&o);
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(code, "board_busy", "{error}");
    assert!(error.contains("open in another process"), "{error}");
    assert!(exists(&file), "deleted a board another tb had open");
    // once it lets go, it goes
    h.ok(&["boards", "delete", "scratch", "--yes"]);
    assert!(!exists(&file));
}

#[test]
fn tb_db_is_refused() {
    let h = Home::new();
    let file = h.archived_scratch();
    let pinned = h.dir.path().join("pinned.db");
    let o = h.cmd(&["boards", "delete", "scratch", "--yes", "--json"]).env("TB_DB", &pinned).output().unwrap();
    let (code, _) = refused(&o);
    assert_eq!(code, "db_pinned");
    assert!(exists(&file));
}
