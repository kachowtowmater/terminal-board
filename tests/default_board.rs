//! `tb boards --default [NAME | --clear]`: which board plain `tb` opens.
//!
//! The precedence, tested for every pair: `TB_DB` > a name on the command line > `TB_BOARD` >
//! the saved default board > `default`. The choice is per user, so it lives in the
//! machine-local settings file (`~/.config/terminal-board/config.json`, or `TB_CONFIG`),
//! never in a board file. Every test drives the real binary under its own temporary HOME.
#![cfg(unix)]
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    /// A HOME with three boards, each holding one card that names its board, plus a file for
    /// `TB_DB` to pin.
    fn new() -> Home {
        let h = Home { dir: tempfile::tempdir().unwrap() };
        for b in ["default", "home", "work"] {
            h.ok(&[b, "add", &format!("x: card-on-{b}")], &[]);
        }
        let pinned = h.pinned();
        h.ok(&["add", "x: card-on-pinned"], &[("TB_DB", pinned.to_str().unwrap())]);
        h
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn pinned(&self) -> PathBuf {
        self.path().join("pinned.db")
    }
    fn config(&self) -> PathBuf {
        self.path().join(".config/terminal-board/config.json")
    }
    fn board_file(&self, name: &str) -> PathBuf {
        self.path().join(format!(".local/state/terminal-board/boards/{name}.db"))
    }
    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).current_dir(self.path()).env("HOME", self.path()).env("TB_AS", "tester").env("TB_NO_HERDR", "1");
        for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD", "TB_CONFIG", "TTYBOARD_CONFIG", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "HERDR_AGENT_NAME"] {
            c.env_remove(k);
        }
        for (k, v) in env {
            c.env(k, v);
        }
        c.stdin(std::process::Stdio::null()).output().unwrap()
    }
    fn ok(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let o = self.run(args, env);
        assert!(o.status.success(), "{args:?} {env:?} failed: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stdout)
    }
    fn json(&self, args: &[&str], env: &[(&str, &str)]) -> serde_json::Value {
        serde_json::from_str(&self.ok(args, env)).unwrap()
    }
    /// A refusal: exit 1, nothing on stdout, the message on stderr.
    fn refused(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let o = self.run(args, env);
        assert_eq!(o.status.code(), Some(1), "{args:?} {env:?} should be refused: {}{}", text(&o.stdout), text(&o.stderr));
        assert!(o.stdout.is_empty(), "{args:?}: a refusal prints nothing on stdout: {}", text(&o.stdout));
        text(&o.stderr)
    }
    /// Which board a command reached: the `card-on-NAME` its `list` shows.
    fn reaches(&self, prefix: &[&str], env: &[(&str, &str)]) -> String {
        let mut args = prefix.to_vec();
        args.push("list");
        let out = self.ok(&args, env);
        let found: Vec<&str> = ["default", "home", "work", "pinned"].into_iter().filter(|b| out.contains(&format!("card-on-{b}"))).collect();
        assert_eq!(found.len(), 1, "{prefix:?} {env:?} should show exactly one board's card:\n{out}");
        found[0].to_string()
    }
    fn save(&self, name: &str) {
        self.ok(&["boards", "--default", name], &[]);
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

fn sorted_keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

/// Set it, see it, clear it — and plain `tb`, `tb boards` and the JSON all follow.
#[test]
fn set_show_and_clear() {
    let h = Home::new();
    // nothing saved: today's behaviour, and asking does not create a settings file
    assert_eq!(h.reaches(&[], &[]), "default");
    assert_eq!(h.ok(&["boards", "--default"], &[]).trim(), "default — the built-in default; choose another with 'tb boards --default NAME'");
    let v = h.json(&["boards", "--default", "--json"], &[]);
    assert_eq!(sorted_keys(&v), ["default", "ok", "setting", "source"]);
    assert_eq!((&v["ok"], &v["default"], &v["source"], &v["setting"]), (&true.into(), &"default".into(), &"builtin".into(), &serde_json::Value::Null));
    assert!(!h.config().exists() && !h.config().parent().unwrap().exists(), "reading the setting created a settings file");

    let said = h.ok(&["boards", "--default", "work"], &[]);
    assert_eq!(said.trim(), "default board is now 'work' — plain 'tb' opens it; go back with 'tb boards --default --clear'");
    assert_eq!(h.reaches(&[], &[]), "work", "plain tb opens the saved default board");
    assert_eq!(h.json(&["--json"], &[])["board"], "work", "and says so in JSON");
    let v = h.json(&["boards", "--default", "--json"], &[]);
    assert_eq!((&v["default"], &v["source"], &v["setting"]), (&"work".into(), &"setting".into(), &"work".into()));
    assert!(h.ok(&["boards", "--default"], &[]).starts_with("work — the saved default board: plain 'tb' opens it"));

    // `tb boards` marks it — and only it
    let listing = h.ok(&["boards"], &[]);
    let marked: Vec<&str> = listing.lines().filter(|l| l.starts_with('*') && !l.starts_with("* =")).collect();
    assert_eq!(marked.len(), 1, "{listing}");
    assert!(marked[0].starts_with("* work "), "{listing}");
    let rows = h.json(&["boards", "--json"], &[]);
    let defaults: Vec<&str> = rows.as_array().unwrap().iter().filter(|r| r["default"] == true).map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(defaults, ["work"]);

    // the choice is in the machine-local settings, never in a board file
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(h.config()).unwrap()).unwrap();
    assert_eq!(saved, serde_json::json!({"default_board": "work"}));

    // clear: --clear, and naming the built-in `default` does the same
    let said = h.ok(&["boards", "--default", "--clear"], &[]);
    assert_eq!(said.trim(), "default board is back to 'default' — choose another with 'tb boards --default NAME'");
    assert_eq!(h.reaches(&[], &[]), "default");
    h.save("home");
    assert_eq!(h.reaches(&[], &[]), "home");
    let v = h.json(&["boards", "--default", "default", "--json"], &[]);
    assert_eq!((&v["default"], &v["source"], &v["setting"]), (&"default".into(), &"builtin".into(), &serde_json::Value::Null));
    assert_eq!(h.reaches(&[], &[]), "default");
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(h.config()).unwrap()).unwrap();
    assert_eq!(saved, serde_json::json!({}), "clearing removes the key");
}

/// `TB_DB` > a typed name > `TB_BOARD` > the saved default board > `default`: every pair.
#[test]
fn precedence_every_pair() {
    let h = Home::new();
    let pin = h.pinned();
    let pin = pin.to_str().unwrap();

    // the bottom of the ladder, before anything is saved
    assert_eq!(h.reaches(&[], &[]), "default", "nothing set");
    assert_eq!(h.reaches(&[], &[("TB_BOARD", "home")]), "home", "TB_BOARD > default");
    assert_eq!(h.reaches(&["work"], &[]), "work", "typed > default");
    assert_eq!(h.reaches(&[], &[("TB_DB", pin)]), "pinned", "TB_DB > default");

    h.save("work");
    assert_eq!(h.reaches(&[], &[]), "work", "saved > default");
    assert_eq!(h.reaches(&[], &[("TB_BOARD", "home")]), "home", "TB_BOARD > saved");
    assert_eq!(h.reaches(&[], &[("TB_BOARD", "default")]), "default", "TB_BOARD > saved, even naming `default`");
    assert_eq!(h.reaches(&["home"], &[]), "home", "typed > saved");
    assert_eq!(h.reaches(&["default"], &[]), "default", "typed `default` > saved");
    assert_eq!(h.reaches(&["-b", "home"], &[]), "home", "-b > saved");
    assert_eq!(h.reaches(&["default"], &[("TB_BOARD", "home")]), "default", "typed > TB_BOARD");
    assert_eq!(h.reaches(&["-b", "default"], &[("TB_BOARD", "home")]), "default", "-b > TB_BOARD > saved");

    // TB_DB > saved: the pinned file, called `default`, and the saved board never shows
    assert_eq!(h.reaches(&[], &[("TB_DB", pin)]), "pinned", "TB_DB > saved");
    assert_eq!(h.json(&["--json"], &[("TB_DB", pin)])["board"], "default");
    let rows = h.json(&["boards", "--json"], &[("TB_DB", pin)]);
    assert_eq!(rows.as_array().unwrap().len(), 1, "{rows}");
    assert_eq!((&rows[0]["name"], &rows[0]["default"]), (&"default".into(), &true.into()), "{rows}");

    // TB_DB > a typed name: refused, as it always was
    let e = h.refused(&["home", "list"], &[("TB_DB", pin)]);
    assert!(e.contains("TB_DB is set — board names are ignored"), "{e}");

    // TB_DB > TB_BOARD (with and without a saved default): the board TB_BOARD names is never
    // the one that answers — tb either uses the pinned file or refuses the mix outright
    for env in [&[("TB_DB", pin), ("TB_BOARD", "home")][..], &[("TB_DB", pin), ("TB_BOARD", "work")][..]] {
        let o = h.run(&["list"], env);
        let out = format!("{}{}", text(&o.stdout), text(&o.stderr));
        for other in ["card-on-home", "card-on-work", "card-on-default"] {
            assert!(!out.contains(other), "{env:?} reached a board TB_DB does not pin:\n{out}");
        }
        assert!(!o.status.success() || out.contains("card-on-pinned"), "{env:?}:\n{out}");
    }
}

/// Under `TB_DB` there is nothing to choose: showing says so, setting and clearing are refused
/// (a test harness must never rewrite a person's settings), and the saved value is not read.
#[test]
fn under_tb_db_the_saved_default_is_ignored_and_says_so() {
    let h = Home::new();
    h.save("work");
    let before = std::fs::read(h.config()).unwrap();
    let pin = h.pinned();
    let env = [("TB_DB", pin.to_str().unwrap())];
    assert_eq!(
        h.ok(&["boards", "--default"], &env).trim(),
        "default — TB_DB pins one board file, so a saved default board is not used; unset TB_DB to use boards"
    );
    let v = h.json(&["boards", "--default", "--json"], &env);
    assert_eq!((&v["default"], &v["source"], &v["setting"]), (&"default".into(), &"TB_DB".into(), &serde_json::Value::Null));
    for args in [&["boards", "--default", "home"][..], &["boards", "--default", "--clear"][..]] {
        let e = h.refused(args, &env);
        assert_eq!(e.trim(), "tb: TB_DB pins one board file, so there is no default board to choose — unset TB_DB, then 'tb boards --default NAME'");
    }
    assert_eq!(std::fs::read(h.config()).unwrap(), before);
    // a settings file that cannot be used does not touch a TB_DB run at all
    std::fs::write(h.config(), "{ broken").unwrap();
    let o = h.run(&["list"], &env);
    assert!(o.status.success() && o.stderr.is_empty() && text(&o.stdout).contains("card-on-pinned"), "{}{}", text(&o.stdout), text(&o.stderr));
    let o = h.run(&["boards"], &env);
    assert!(o.status.success() && o.stderr.is_empty(), "{}", text(&o.stderr));
}

/// An unknown board, an archived board, a name that is no board name, and mixed-up flags are
/// refused in the house style — and nothing is saved.
#[test]
fn unknown_and_archived_boards_are_refused() {
    let h = Home::new();
    h.save("home");
    let before = std::fs::read(h.config()).unwrap();

    let e = h.refused(&["boards", "--default", "wrok"], &[]);
    assert_eq!(
        e.trim(),
        "tb: no board 'wrok' — boards: default, home, work · choose one that exists: 'tb boards --default NAME' (create it first with 'tb wrok add \"…\"')"
    );
    let o = h.run(&["boards", "--default", "wrok", "--json"], &[]);
    assert_eq!(o.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(sorted_keys(&v), ["error", "hint", "ok"]);
    assert_eq!((&v["ok"], &v["error"]), (&false.into(), &"no board 'wrok'".into()));
    assert!(v["hint"].as_str().unwrap().contains("'tb boards --default NAME'"), "{v}");

    // an archived board is a file moved out of the boards directory: not a board to open
    let archive = h.path().join(".local/state/terminal-board/archive");
    std::fs::create_dir_all(&archive).unwrap();
    // (archived boards are named NAME@STAMP)
    std::fs::rename(h.board_file("work"), archive.join(format!("work{}20260921-101500.db", '@'))).unwrap();
    let e = h.refused(&["boards", "--default", "work"], &[]);
    assert!(e.starts_with("tb: no board 'work' — boards: default, home · "), "{e}");

    for bad in ["Work", "two words", "list"] {
        let e = h.refused(&["boards", "--default", bad], &[]);
        assert!(e.contains(&format!("'{bad}'")) && e.contains("board name"), "{bad}: {e}");
    }
    let e = h.refused(&["boards", "--default", "home", "--clear"], &[]);
    assert!(e.contains("give a board name or --clear, not both — 'tb boards --default NAME' or 'tb boards --default --clear'"), "{e}");
    let o = h.run(&["boards", "--clear"], &[]);
    assert_eq!(o.status.code(), Some(2), "--clear alone is a usage error: {}", text(&o.stderr));

    assert_eq!(std::fs::read(h.config()).unwrap(), before, "a refusal saved nothing");
    assert_eq!(h.reaches(&[], &[]), "home");
}

/// A saved default whose board is gone is refused: never re-created as an empty phantom,
/// never silently swapped for `default`. Naming a board still works, and clearing fixes it.
#[test]
fn a_saved_default_whose_board_is_gone_is_refused() {
    let h = Home::new();
    h.save("work");
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", h.board_file("work").display()));
    }
    let want = "tb: the saved default board is 'work', but there is no board 'work' — boards: default, home · choose another with 'tb boards --default NAME' or go back with 'tb boards --default --clear'";
    for args in [&["list"][..], &["add", "x: would land on a phantom"][..], &["next"][..], &["config", "wip", "5"][..], &[][..]] {
        assert_eq!(h.refused(args, &[]).trim(), want, "{args:?}");
    }
    assert!(!h.board_file("work").exists(), "the missing board was re-created");
    assert_eq!(h.reaches(&["home"], &[]), "home", "a named board still works");
    assert_eq!(h.reaches(&[], &[("TB_BOARD", "home")]), "home", "and so does TB_BOARD");
    let listing = h.ok(&["boards"], &[]);
    assert!(!listing.lines().any(|l| l.starts_with("* ") && !l.starts_with("* =")), "no row is the default:\n{listing}");
    h.ok(&["boards", "--default", "--clear"], &[]);
    assert_eq!(h.reaches(&[], &[]), "default");
}

/// The command a hint names, e.g. `tb home take 2` out of `… take it with 'tb home take 2'`.
fn hinted_command(out: &str) -> Vec<String> {
    let start = out.find("'tb ").unwrap_or_else(|| panic!("no hint in: {out}")) + 1;
    let end = start + out[start..].find('\'').unwrap();
    out[start..end].split_whitespace().skip(1).map(str::to_string).collect()
}

/// (what was typed, the environment, the board it is on, how the hint must start)
type HintCase = (&'static [&'static str], &'static [(&'static str, &'static str)], &'static str, &'static str);

/// Every hint keeps naming the right board when plain `tb` no longer means `default`: the hint
/// is copied into a fresh shell and must act on the board the user was looking at.
#[test]
fn a_copied_hint_acts_on_the_same_board() {
    let h = Home::new();
    h.save("work");
    let cases: [HintCase; 8] = [
        (&[], &[], "work", "'tb take "),
        (&["work"], &[], "work", "'tb take "),
        (&["default"], &[], "default", "'tb default take "),
        (&["-b", "default"], &[], "default", "'tb default take "),
        (&["home"], &[], "home", "'tb home take "),
        (&[], &[("TB_BOARD", "home")], "home", "'tb take "),
        (&["work"], &[("TB_BOARD", "home")], "work", "'tb work take "),
        (&["default"], &[("TB_BOARD", "home")], "default", "'tb default take "),
    ];
    for (typed, env, board, hint) in cases {
        let mut args = typed.to_vec();
        args.extend(["add", "x: follow the hint"]);
        let out = h.ok(&args, env);
        assert!(out.contains(hint), "{typed:?} {env:?}: the hint should start {hint}: {out}");
        // run the hint exactly as printed, in the same environment, from a fresh process
        let copied = hinted_command(&out);
        let copied: Vec<&str> = copied.iter().map(String::as_str).collect();
        let taken = h.json(&[&copied[..], &["--json"]].concat(), env);
        assert_eq!(taken["ok"], true, "{copied:?}: {taken}");
        let id = taken["card"]["id"].as_i64().unwrap().to_string();
        // the card the hint took is on the board the add went to
        let shown = h.json(&[board, "show", &id, "--json"], &[]);
        assert_eq!((&shown["title"], &shown["column"]), (&"follow the hint".into(), &"doing".into()), "{typed:?} {env:?} -> {copied:?}");
        h.ok(&[board, "drop", &id], &[]);
        h.ok(&[board, "rm", &id], &[]);
    }
    // error hints too, plain and --json
    let e = h.refused(&["default", "done", "99"], &[]);
    assert!(e.contains("'tb default list'"), "{e}");
    let o = h.run(&["default", "done", "99", "--json"], &[]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["hint"], "see 'tb default list' for ids", "{v}");
    let e = h.refused(&["done", "99"], &[]);
    assert!(e.contains("'tb list'") && !e.contains("'tb work"), "plain tb is the saved board, so the bare hint is right: {e}");

    // nothing saved: exactly today's hints
    h.ok(&["boards", "--default", "--clear"], &[]);
    assert!(h.ok(&["default", "add", "x: y"], &[]).contains("'tb take "));
    assert!(h.ok(&["work", "add", "x: y"], &[]).contains("'tb work take "));
    assert!(h.ok(&["home", "add", "x: y"], &[("TB_BOARD", "home")]).contains("'tb home take "));
}

/// The settings file: where it is, private, shared with other owners' keys, and never
/// overwritten when it cannot be read.
#[test]
fn the_settings_file_is_private_shared_and_never_clobbered() {
    use std::os::unix::fs::PermissionsExt;
    let h = Home::new();
    // other owners' keys survive set and clear
    std::fs::create_dir_all(h.config().parent().unwrap()).unwrap();
    let theirs = serde_json::json!({"lint": {"argv": ["/usr/bin/true", "--strict"], "sha256": "ab12"}});
    std::fs::write(h.config(), serde_json::to_string(&serde_json::json!({"hooks": theirs, "zz_future": [1, 2, 3]})).unwrap()).unwrap();
    h.save("work");
    let read = || -> serde_json::Value { serde_json::from_str(&std::fs::read_to_string(h.config()).unwrap()).unwrap() };
    assert_eq!(read(), serde_json::json!({"default_board": "work", "hooks": theirs, "zz_future": [1, 2, 3]}));
    assert_eq!(std::fs::metadata(h.config()).unwrap().permissions().mode() & 0o777, 0o600, "the settings file is private");
    h.ok(&["boards", "--default", "--clear"], &[]);
    assert_eq!(read(), serde_json::json!({"hooks": theirs, "zz_future": [1, 2, 3]}));

    // TB_CONFIG names another file; the one under HOME is left alone
    let elsewhere = h.path().join("elsewhere/settings.json");
    let env = [("TB_CONFIG", elsewhere.to_str().unwrap())];
    h.ok(&["boards", "--default", "home"], &env);
    assert_eq!(h.reaches(&[], &env), "home");
    assert_eq!(h.reaches(&[], &[]), "default", "HOME's settings were not touched");
    assert_eq!(std::fs::metadata(&elsewhere).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(elsewhere.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);

    // a file tb cannot read as settings: plain tb refuses (it cannot know which board was
    // meant), a named board still works, and NOTHING overwrites the file
    for bad in ["{ \"default_board\": \"work\", ", "[\"work\"]", "{\"default_board\": 7}"] {
        std::fs::write(h.config(), bad).unwrap();
        let e = h.refused(&["list"], &[]);
        assert!(e.contains(&h.config().display().to_string()), "the refusal names the file: {e}");
        assert!(e.contains(" — "), "and says what to do: {e}");
        assert_eq!(h.reaches(&["home"], &[]), "home");
        assert_eq!(h.reaches(&[], &[("TB_BOARD", "work")]), "work");
        let o = h.run(&["boards"], &[]);
        assert!(o.status.success() && text(&o.stderr).contains(&h.config().display().to_string()), "tb boards lists, and says why no saved board is marked: {}", text(&o.stderr));
        if bad != "{\"default_board\": 7}" {
            let e = h.refused(&["boards", "--default", "home"], &[]);
            assert!(e.contains("never overwrites"), "{e}");
        }
        assert_eq!(std::fs::read_to_string(h.config()).unwrap(), bad, "the file is exactly as the person left it");
    }
    // a wrong VALUE in a readable file can simply be set again
    h.save("home");
    assert_eq!(h.reaches(&[], &[]), "home");
}

/// A machine that saves nothing is untouched: no settings directory appears, whatever runs.
#[test]
fn nothing_saved_nothing_created() {
    let h = Home::new();
    for args in [&["list"][..], &["boards"][..], &["boards", "--json"][..], &["boards", "--default"][..], &["boards", "--default", "--clear"][..], &["next"][..], &["work", "list"][..]] {
        h.ok(args, &[]);
    }
    assert!(!h.path().join(".config").exists(), "a settings directory appeared although nothing was saved");
    let listing = h.ok(&["boards"], &[]);
    assert!(listing.contains("* default ") && listing.ends_with("* = default (plain 'tb'); open another with 'tb NAME'\n"), "{listing}");
    let help = h.ok(&["--help"], &[]);
    assert!(help.contains("boards [--default [NAME|--clear]]") && help.lines().count() <= 25, "{help}");
}

/// The precedence is written down once in each manual.
#[test]
fn the_manuals_state_the_precedence() {
    for (name, doc) in [("README.md", include_str!("../README.md")), ("docs/HUMANS.md", include_str!("../docs/HUMANS.md")), ("docs/AGENTS.md", include_str!("../docs/AGENTS.md")), ("docs/JSON.md", include_str!("../docs/JSON.md"))] {
        assert!(doc.contains("tb boards --default"), "{name} never shows tb boards --default");
    }
    let readme = include_str!("../README.md");
    assert!(readme.contains("`TB_DB` > a board named on the command line > `TB_BOARD` > the saved default board > `default`"), "README states the precedence");
    assert!(include_str!("../docs/AGENTS.md").lines().count() <= 250, "the agent manual is over its line cap");
}
