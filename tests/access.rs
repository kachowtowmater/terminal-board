//! One card per agent (`wip-per-owner`), known names (`actors`) and read-only mode.
#![cfg(unix)]
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home { dir: tempfile::tempdir().unwrap() }
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn board_file(&self, name: &str) -> PathBuf {
        self.path().join(format!(".local/state/terminal-board/boards/{name}.db"))
    }
    fn cmd(&self, args: &[&str], actor: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).current_dir(self.path()).env("HOME", self.path()).env("TB_AS", actor).env("TB_NO_HERDR", "1").env("TZ", "UTC");
        for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD", "TB_CONFIG", "TB_READONLY", "TTYBOARD_READONLY", "HERDR_AGENT_NAME", "XDG_STATE_HOME"] {
            c.env_remove(k);
        }
        c.stdin(Stdio::null());
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args, "alice").output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stdout)
    }
    fn ok_as(&self, args: &[&str], actor: &str) -> String {
        let o = self.cmd(args, actor).output().unwrap();
        assert!(o.status.success(), "{args:?} as {actor} failed: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stdout)
    }
    fn refused(&self, args: &[&str]) -> String {
        self.refused_as(args, "alice")
    }
    fn refused_as(&self, args: &[&str], actor: &str) -> String {
        let o = self.cmd(args, actor).output().unwrap();
        assert_eq!(o.status.code(), Some(1), "{args:?} as {actor} should be refused: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stderr)
    }
    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

fn ids(out: &str) -> Vec<i64> {
    out.lines()
        .filter_map(|l| l.split_whitespace().find(|w| w.starts_with('#')))
        .filter_map(|w| w.trim_start_matches('#').parse().ok())
        .collect()
}

/// A7, the client's own report: with a board-wide wip of 2, one agent took both slots and the
/// second agent was locked out. A per-owner cap is what makes a shared board fair.
#[test]
fn one_agent_can_no_longer_take_every_slot() {
    let h = Home::new();
    for n in 1..=4 {
        h.ok(&["add", &format!("x: card {n}")]);
    }
    h.ok(&["config", "wip", "2"]);

    // today's behaviour, reproduced: alice takes both slots and bob is locked out
    h.ok_as(&["take", "1"], "alice");
    h.ok_as(&["take", "2"], "alice");
    let e = h.refused_as(&["take", "3"], "bob");
    assert!(e.contains("doing is full (2/2"), "the board-wide limit still speaks first: {e}");
    h.ok_as(&["drop", "2"], "alice");

    // with the cap, alice's second take is refused and bob gets a slot
    h.ok(&["config", "wip-per-owner", "1"]);
    let e = h.refused_as(&["take", "2"], "alice");
    assert!(e.contains("you already hold 1 of 1 (#1 card 1)"), "{e}");
    assert!(e.contains("finish one with 'tb done 1'"), "it says what the asker can do: {e}");
    assert!(e.contains("'tb config wip-per-owner 2'"), "and how to raise it: {e}");
    h.ok_as(&["take", "2"], "bob");
    assert_eq!(ids(&h.ok(&["list", "--column", "doing"])).len(), 2, "two people, one card each");
    // and the board-wide limit is still there for everyone together
    let e = h.refused_as(&["take", "3"], "carol");
    assert!(e.contains("doing is full (2/2"), "{e}");

    // `tb next` is the same rule (it is the same transition)
    h.ok(&["config", "wip", "9"]);
    let e = h.refused_as(&["next"], "alice");
    assert!(e.contains("you already hold 1 of 1"), "{e}");
    // and so is moving a card into doing by hand
    let e = h.refused_as(&["move", "3", "doing"], "alice");
    assert!(e.contains("you already hold 1 of 1"), "{e}");
    // finishing one makes room again
    h.ok_as(&["done", "1"], "alice");
    h.ok_as(&["take", "3"], "alice");
}

/// How the two limits compose, and how the per-owner one treats a blocked card: the same way
/// the board-wide one does, under the same setting, capped by its own number.
#[test]
fn the_two_wip_limits_compose() {
    let h = Home::new();
    for n in 1..=6 {
        h.ok(&["add", &format!("x: card {n}")]);
    }
    h.ok(&["config", "wip", "9"]);
    h.ok(&["config", "wip-per-owner", "1"]);
    h.ok_as(&["take", "1"], "alice");

    // counting blocked cards (the default): a blocked card still uses alice's slot
    assert!(h.refused_as(&["take", "2"], "alice").contains("you already hold 1 of 1"));
    h.ok(&["block", "1", "#9"]);
    let e = h.refused_as(&["take", "2"], "alice");
    assert!(e.contains("you already hold 1 of 1"), "blocked still counts by default: {e}");

    // `wip-counts-blocked no` discounts it for the PERSON as it does for the board — waiting
    // on somebody else stalls a person exactly as it stalls a board
    h.ok(&["config", "wip-counts-blocked", "no"]);
    h.ok_as(&["take", "2"], "alice");
    assert_eq!(ids(&h.ok(&["list", "--column", "doing"])).len(), 2);
    // ...and the discount is capped by the cap, so blocking everything never hands out
    // unlimited work: alice holds 2, one blocked, so one counts and she is at her limit again
    let e = h.refused_as(&["take", "3"], "alice");
    assert!(e.contains("(1 of them blocked and not counted)"), "the message shows the discount: {e}");
    h.ok(&["block", "2", "#9"]);
    let e = h.refused_as(&["take", "3"], "alice");
    assert!(e.contains("you already hold 1 of 1"), "at most `cap` blocked cards are discounted: {e}");

    // the board-wide limit binds first when it is the tighter one. Alice holds two, BOTH
    // blocked, so the board discounts both and genuinely has room — which is the setting
    // working, not a hole: bob may start work while alice waits on somebody else.
    h.ok(&["config", "wip", "2"]);
    h.ok_as(&["take", "3"], "bob");
    h.ok_as(&["take", "4"], "carol");
    // now three unblocked cards are in DOING against a board limit of 2, so the board is full
    // for everyone, whatever their own cap says
    let e = h.refused_as(&["take", "5"], "dave");
    assert!(e.contains("doing is full"), "{e}");
}

/// The setting itself: read, set, clear, refuse a bad value, and stay out of a board that
/// does not use it.
#[test]
fn the_per_owner_setting() {
    let h = Home::new();
    h.ok(&["add", "x: one"]);
    assert!(h.ok(&["config", "wip-per-owner"]).contains("wip-per-owner is off"));
    assert!(!h.ok(&["config"]).contains("wip-per-owner"), "an unset key is not listed");
    assert_eq!(h.json(&["config", "--json"])["config"].get("wip-per-owner"), None);

    assert!(h.ok(&["config", "wip-per-owner", "1"]).contains("nobody may hold more than that many"));
    assert!(h.ok(&["config"]).contains("wip-per-owner 1"));
    assert_eq!(h.ok(&["config", "wip-per-owner"]).trim(), "1");
    assert_eq!(h.json(&["config", "wip-per-owner", "2", "--json"])["config"]["value"], 2);
    assert_eq!(h.json(&["config", "--json"])["config"]["wip-per-owner"], "2");

    assert!(h.ok(&["config", "wip-per-owner", "0"]).contains("is now off"));
    assert!(!h.ok(&["config"]).contains("wip-per-owner"));
    for bad in ["100", "lots"] {
        let e = h.refused(&["config", "wip-per-owner", bad]);
        assert!(e.contains("wip-per-owner"), "{bad}: {e}");
        assert!(e.contains("'tb config wip-per-owner 1'"), "{bad}: {e}");
    }
    // a leading dash is the parser's business, and it says so (exit 2, a usage error)
    assert_eq!(h.run(&["config", "wip-per-owner", "-1"]).status.code(), Some(2));
    // the change is recorded on the board's own log. Nothing prints board_events yet (that
    // is its own open card), so this reads the row rather than a command's output.
    h.ok(&["config", "wip-per-owner", "3"]);
    let db = h.board_file("default");
    let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let logged: String = conn
        .query_row("SELECT text FROM board_events WHERE kind='wip-per-owner' ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(logged, "wip-per-owner off -> 3");
}

/// D6: a board that knows its names refuses one it does not, so a typo cannot invent an agent.
#[test]
fn a_board_can_know_its_names() {
    let h = Home::new();
    h.ok(&["add", "x: one"]);
    h.ok(&["add", "x: two"]);
    assert!(h.ok(&["config", "actors"]).contains("actors is off"));

    h.ok(&["config", "actors", "alice, bob"]);
    assert!(h.ok(&["config"]).contains("actors        alice, bob"));
    assert_eq!(h.json(&["config", "--json"])["config"]["actors"], "alice, bob");

    // a known name writes; a typo is refused and says what to do
    h.ok_as(&["take", "1"], "alice");
    h.ok_as(&["note", "1", "hello"], "ALICE");
    let e = h.refused_as(&["add", "x: from a phantom"], "alicce");
    assert!(e.contains("'alicce' is not one of this board's names (alice, bob)"), "{e}");
    assert!(e.contains("'tb config actors alice,bob,alicce'"), "{e}");
    assert_eq!(ids(&h.ok(&["list"])).len(), 2, "the phantom's card was never made");

    // READS are never refused — a watcher does not need to be on the list
    h.ok_as(&["list"], "carol");
    h.ok_as(&["board", "--json"], "carol");
    h.ok_as(&["show", "1"], "carol");
    h.ok_as(&["log"], "carol");

    // names are compared trimmed and without case, as owners are everywhere else
    h.ok_as(&["note", "1", "spaced"], "  bob  ");
    h.ok(&["config", "actors", " Carol , alice , bob "]);
    assert!(h.ok(&["config"]).contains("Carol, alice, bob"), "order and spelling kept, spaces gone");
    h.ok_as(&["note", "1", "carol was here"], "carol");
    // a name given twice is one name
    h.ok(&["config", "actors", "alice,ALICE,bob"]);
    assert!(h.ok(&["config"]).contains("actors        alice, bob"));

    // clearing it lets anyone back in
    h.ok(&["config", "actors", "--off"]);
    assert!(h.ok(&["config", "actors"]).contains("actors is off"));
    h.ok_as(&["note", "1", "anyone again"], "dave");
}

/// The list must never become a trap, and must not break tb's own sync or existing work.
#[test]
fn a_names_list_cannot_lock_the_board() {
    let h = Home::new();
    h.ok(&["add", "x: one"]);
    // a card already held by a name that is not on the list stays exactly as it was
    h.ok_as(&["take", "1"], "mallory");
    h.ok(&["config", "actors", "alice,bob"]);
    let card = h.json(&["show", "1", "--json"]);
    assert_eq!((&card["owner"], &card["column"]), (&"mallory".into(), &"doing".into()), "existing work is untouched");
    assert!(h.ok(&["list"]).contains("mallory"), "and still visible");
    // mallory cannot write any more, but somebody on the list can take the card back
    assert!(h.refused_as(&["note", "1", "sneaky"], "mallory").contains("not one of this board's names"));
    h.ok_as(&["drop", "1", "--force"], "alice");

    // setting a list that does not include you is refused — it would lock you out
    let e = h.refused(&["config", "actors", "bob,carol"]);
    assert!(e.contains("'alice' is not in that list, and setting it would lock you out"), "{e}");
    assert!(e.contains("'tb config actors bob,carol,alice'"), "{e}");
    assert!(h.ok(&["config"]).contains("alice, bob"), "the old list is unchanged");

    // and `tb config` is never gated, so even a list nobody present satisfies can be fixed
    h.ok_as(&["config", "actors", "bob"], "bob");
    assert!(h.refused_as(&["add", "x: y"], "alice").contains("not one of this board's names"));
    h.ok_as(&["config", "actors", "alice,bob"], "alice");
    h.ok_as(&["add", "x: back in"], "alice");

    // tb's own sync name is never on a list and never refused by one
    h.ok(&["config", "actors", "alice"]);
    let o = h.cmd(&["note", "1", "from the sync"], "github").output().unwrap();
    assert!(!text(&o.stderr).contains("not one of this board's names"), "the sync actor was refused: {}", text(&o.stderr));
}

/// D7: read-only refuses every write, with the documented JSON shape, and changes nothing.
#[test]
fn read_only_refuses_every_write() {
    let h = Home::new();
    h.ok(&["add", "x: one"]);
    h.ok(&["add", "x: two"]);
    h.ok(&["take", "1"]);
    let before = std::fs::read(h.board_file("default")).unwrap();

    let writes: Vec<Vec<&str>> = vec![
        vec!["add", "x: nope"],
        vec!["note", "1", "nope"],
        vec!["check", "1", "--add", "nope"],
        vec!["edit", "1", "--desc", "nope"],
        vec!["block", "1", "#2"],
        vec!["done", "1"],
        vec!["move", "2", "doing"],
        vec!["take", "2"],
        vec!["next"],
        vec!["drop", "1"],
        vec!["prio", "1", "top"],
        vec!["rm", "2"],
        vec!["config", "wip", "5"],
        vec!["config", "actors", "alice"],
        vec!["mv", "1", "--to", "other"],
        vec!["import", "cards.json"],
        vec!["new", "another"],
    ];
    for how in ["env", "flag"] {
        for w in &writes {
            let mut args: Vec<&str> = w.clone();
            if how == "flag" {
                args.push("--read-only");
            }
            let mut c = h.cmd(&args, "alice");
            if how == "env" {
                c.env("TB_READONLY", "1");
            }
            let o = c.output().unwrap();
            assert_eq!(o.status.code(), Some(1), "{how} {args:?} was not refused: {}{}", text(&o.stdout), text(&o.stderr));
            let e = text(&o.stderr);
            assert!(e.contains("read-only mode:"), "{how} {args:?}: {e}");
            assert!(e.contains("unset TB_READONLY"), "{how} {args:?} does not say how to write: {e}");
        }
    }
    assert_eq!(std::fs::read(h.board_file("default")).unwrap(), before, "read-only changed the board file");

    // reads all work
    for r in [&["list"][..], &["board"][..], &["board", "--json"][..], &["show", "1"][..], &["config"][..], &["config", "github"][..], &["config", "--json"][..], &["boards"][..], &["log"][..], &["export", "--json"][..], &["list", "--done"][..]] {
        let o = h.cmd(r, "alice").env("TB_READONLY", "1").output().unwrap();
        assert!(o.status.success(), "read-only refused a READ {r:?}: {}", text(&o.stderr));
    }
    assert_eq!(std::fs::read(h.board_file("default")).unwrap(), before);

    // the documented failure shape
    let o = h.cmd(&["add", "x: nope", "--json"], "alice").env("TB_READONLY", "1").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let mut keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["error", "hint", "ok"]);
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().starts_with("read-only mode: 'tb add'"), "{v}");
    assert!(v["hint"].as_str().unwrap().contains("unset TB_READONLY"), "{v}");

    // the values a person might reasonably use to turn it OFF really do
    for off in ["0", "no", "false", ""] {
        let o = h.cmd(&["note", "1", "allowed"], "alice").env("TB_READONLY", off).output().unwrap();
        assert!(o.status.success(), "TB_READONLY={off:?} should not be read-only: {}", text(&o.stderr));
    }
    for on in ["1", "yes", "true"] {
        let o = h.cmd(&["note", "1", "nope"], "alice").env("TB_READONLY", on).output().unwrap();
        assert_eq!(o.status.code(), Some(1), "TB_READONLY={on} should be read-only");
    }
}

/// THE BACKSTOP: read-only is enforced at the connection, not only at the command layer. A
/// write that somehow reached the database would fail there too — proved by asking the
/// library to write directly, with no command in the way.
#[test]
fn read_only_is_enforced_at_the_database_not_only_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    {
        let s = terminal_board::store::Store::open(&db).unwrap();
        s.add("x: one", "", &[], "alice").unwrap();
    }
    let before = std::fs::read(&db).unwrap();
    std::env::set_var("TB_READONLY", "1");
    let store = terminal_board::store::Store::open(&db).expect("a read-only board still opens");
    // reads work
    assert_eq!(store.list().unwrap().len(), 1);
    // and a write straight into the store — no command, no check — is still refused
    let e = store.add("x: two", "", &[], "alice").unwrap_err().0;
    assert!(e.contains("read-only mode"), "a direct write was not refused: {e}");
    let e = store.note(1, "straight in", "alice").unwrap_err().0;
    assert!(e.contains("read-only mode"), "{e}");
    std::env::remove_var("TB_READONLY");
    assert_eq!(std::fs::read(&db).unwrap(), before, "the file changed");
}

/// The full-screen board writes as you use it, so it is not offered to a viewer.
#[test]
fn read_only_does_not_open_the_full_screen_board() {
    let h = Home::new();
    h.ok(&["add", "x: one"]);
    // without a terminal, bare `tb` prints the board once — that is a read and must work
    let o = h.cmd(&[], "alice").env("TB_READONLY", "1").output().unwrap();
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains("TODO"), "{}", text(&o.stdout));
}

/// The manuals teach all three, and the agent manual stays under its cap.
#[test]
fn the_manuals_teach_it() {
    let agents = include_str!("../docs/AGENTS.md");
    for phrase in ["wip-per-owner", "read-only"] {
        assert!(agents.contains(phrase), "docs/AGENTS.md lacks `{phrase}`");
    }
    let n = agents.lines().count();
    assert!(n <= 250, "the agent manual is {n} lines, over its cap");
    for (name, doc) in [("README.md", include_str!("../README.md")), ("docs/JSON.md", include_str!("../docs/JSON.md"))] {
        assert!(doc.contains("wip-per-owner") && doc.contains("actors") && doc.contains("read-only"), "{name}");
    }
}
