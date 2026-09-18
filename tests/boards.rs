//! Named boards, driven through the real binary with a temp HOME (no TB_DB).
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

