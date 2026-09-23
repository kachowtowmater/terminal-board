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
    assert_eq!(keys, ["code", "error", "hint", "ok"]);
    assert_eq!(v["ok"], false);
    // the stable machine-readable symbol (#144), so a script can branch on the refusal
    // without matching English: read-only is its own code, not the catch-all
    assert_eq!(v["code"], "read_only");
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

/// Drive the full-screen board in a REAL TERMINAL: `script` gives it a pty, and the keys are
/// fed with pauses, because the board reads them as they arrive rather than all at once.
fn board_keys(h: &Home, actor: &str, typed: &str) -> Output {
    let tb = env!("CARGO_BIN_EXE_tb");
    // The keys are fed to SCRIPT's stdin, never to the board's: crossterm reads the terminal
    // (`/dev/tty`), not stdin, so keys piped straight at `tb` are read by nobody. The board
    // then sits there until `timeout` kills it — and a test that drove nothing still passes
    // its negative control, for entirely the wrong reason. `script` owns the pty, so what it
    // reads on ITS stdin is what the board reads as typed keys.
    let pty = if cfg!(target_os = "linux") {
        format!("script -q -e -c {} /dev/null", shell_quote(tb))
    } else {
        format!("script -q /dev/null {}", shell_quote(tb))
    };
    // The watchdog is perl's `alarm`, not `timeout`: macOS has no `timeout`, and the failure
    // it produced was the worst kind — `sh: timeout: command not found` left the board
    // unstarted, so the test read "a known name was refused" and pointed at the guard
    // instead of at itself. `alarm` survives the `exec`, so the timer still bounds the board.
    // `a` opens the add form, the title is typed, Enter moves to the (left empty) due-date
    // step, a second Enter saves it, `q` quits; the pauses are needed because the board reads
    // keys as they arrive rather than all at once.
    let feed = format!(
        "(sleep 2; printf a; sleep 1; printf %s {}; sleep 0.5; printf '\\r'; sleep 0.5; printf '\\r'; sleep 1.5; printf q; sleep 1) | perl -e 'alarm shift @ARGV; exec @ARGV' 25 {} 2>&1",
        shell_quote(typed),
        pty
    );
    let mut c = Command::new("sh");
    c.args(["-c", &feed]);
    c.current_dir(h.path()).env("HOME", h.path()).env("TB_AS", actor).env("TB_NO_HERDR", "1").env("TZ", "UTC");
    for k in ["TB_DB", "TB_BOARD", "TB_CONFIG", "TB_READONLY"] {
        c.env_remove(k);
    }
    let o = c.output().unwrap();
    // Proof that the board really ran and really quit under its own key. Without this, every
    // assertion below is satisfied just as well by a board that never started — which is
    // exactly what happened twice while this test was being written. The pty is sized 0x0 so
    // nothing legible is drawn, but entering and leaving the alternate screen is unmissable,
    // and the second sequence is written only on a clean shutdown.
    let seen = String::from_utf8_lossy(&o.stdout);
    assert!(seen.contains("\x1b[?1049h"), "the board never opened: {seen:?}");
    assert!(seen.contains("\x1b[?1049l"), "the board never took its quit key: {seen:?}");
    o
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// DEFECT 1, in the reviewer's own construction: the full-screen board calls the store
/// directly, so a check at the command layer has a door beside it. The check now lives in
/// `Store::log`, which every card change goes through, so the board cannot walk past it.
#[test]
fn the_full_screen_board_cannot_walk_past_the_names_list() {
    let h = Home::new();
    // past the first-run wizard, or the pty meets that instead of the board
    h.ok(&["setup", "--yes", "--no-github", "--no-agents"]);
    h.ok(&["add", "x: a card"]);
    h.ok(&["config", "actors", "alice"]);
    let before = std::fs::read(h.board_file("default")).unwrap();

    // a name the board does not know, adding a card through the board itself
    let o = board_keys(&h, "mallory", "mallory was here");
    assert_eq!(
        ids(&h.ok(&["list"])).len(),
        1,
        "an off-list name added a card through the full-screen board:\n{}",
        text(&o.stdout)
    );
    assert_eq!(std::fs::read(h.board_file("default")).unwrap(), before, "the board file changed");

    // the same board, the same keys, a name it does know: the card really is added, so the
    // test is proving the guard and not merely that the keys did nothing
    let o = board_keys(&h, "alice", "alice was here");
    let after = ids(&h.ok(&["list"]));
    assert_eq!(after.len(), 2, "the board refused a name it knows:\n{}", text(&o.stdout));
    assert!(h.ok(&["list"]).contains("alice was here"));
}

/// The board's other writing keys, through the board's own key handler — the harness the rest
/// of this repo uses for board behaviour, and the same store calls the pty above makes.
#[test]
fn every_writing_key_on_the_board_honours_the_names_list() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use terminal_board::store::Store;
    use terminal_board::tui::{App, Mode};
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    let mut s = Store::open(&db).unwrap();
    s.add("x: one", "", &[], "alice").unwrap();
    s.add("x: two", "", &[], "alice").unwrap();
    s.set_actors("alice", "alice").unwrap();
    let before = std::fs::read(&db).unwrap();

    let key = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
    let mut app = App::new(s.snapshot().unwrap(), "mallory");
    app.reload(&s);
    // every key that changes a card: next/take, done, delete (and its y), prio, and the add
    // and note forms typed out in full
    for k in [KeyCode::Char('n'), KeyCode::Char('d'), KeyCode::Char('x'), KeyCode::Char('y')] {
        app.handle_key(key(k), &mut s);
    }
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT), &mut s);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT), &mut s);
    for form in ["a", "N"] {
        app.handle_key(key(KeyCode::Char(form.chars().next().unwrap())), &mut s);
        for c in "typed by mallory".chars() {
            app.handle_key(key(KeyCode::Char(c)), &mut s);
        }
        app.handle_key(key(KeyCode::Enter), &mut s);
        if form == "a" {
            // title -> due-date step (card #104); enter again with an empty date to reach
            // the actual write (or refusal) the way a person just pressing enter twice would
            app.handle_key(key(KeyCode::Enter), &mut s);
        }
        app.mode = Mode::Normal;
    }
    assert_eq!(std::fs::read(&db).unwrap(), before, "a board key wrote for a name the board does not know");
    assert_eq!(s.list().unwrap().len(), 2, "a card was added or removed");

    // and the same keys as a name the board knows do work
    let mut app = App::new(s.snapshot().unwrap(), "alice");
    app.reload(&s);
    app.handle_key(key(KeyCode::Char('a')), &mut s);
    for c in "typed by alice".chars() {
        app.handle_key(key(KeyCode::Char(c)), &mut s);
    }
    app.handle_key(key(KeyCode::Enter), &mut s);
    app.handle_key(key(KeyCode::Enter), &mut s); // due-date step, left empty
    assert_eq!(s.list().unwrap().len(), 3, "the board refused a name it knows");
}

/// Every other way in: the CLI, an import, an edit from a file, and a move between boards.
#[test]
fn every_write_route_honours_the_names_list() {
    let h = Home::new();
    h.ok(&["add", "x: one"]);
    h.ok(&["other", "add", "x: over there"]);
    h.ok(&["config", "actors", "alice"]);
    h.ok(&["other", "config", "actors", "alice"]);
    std::fs::write(h.path().join("cards.json"), r#"[{"title":"x: imported"}]"#).unwrap();
    std::fs::write(h.path().join("edit.json"), r#"[{"id":1,"description":"changed"}]"#).unwrap();
    let before = std::fs::read(h.board_file("default")).unwrap();

    for args in [
        &["add", "x: nope"][..],
        &["note", "1", "nope"][..],
        &["check", "1", "--add", "nope"][..],
        &["edit", "1", "--desc", "nope"][..],
        &["block", "1", "#2"][..],
        &["take", "1"][..],
        &["next"][..],
        &["prio", "1", "top"][..],
        &["rm", "1"][..],
        &["import", "cards.json"][..],
        &["edit", "--from", "edit.json"][..],
        &["mv", "1", "--to", "other"][..],
    ] {
        let o = h.cmd(args, "mallory").output().unwrap();
        assert_eq!(o.status.code(), Some(1), "{args:?} was not refused: {}{}", text(&o.stdout), text(&o.stderr));
        assert!(text(&o.stderr).contains("not one of this board's names"), "{args:?}: {}", text(&o.stderr));
    }
    assert_eq!(std::fs::read(h.board_file("default")).unwrap(), before, "an off-list name wrote something");

    // a move is refused by the board it would ARRIVE at as well, not only the one it leaves
    h.ok(&["config", "actors", "alice,mallory"]);
    let o = h.cmd(&["mv", "1", "--to", "other"], "mallory").output().unwrap();
    assert_eq!(o.status.code(), Some(1), "the destination board's list was not consulted");
    assert!(text(&o.stderr).contains("not one of this board's names"), "{}", text(&o.stderr));
    assert_eq!(ids(&h.ok(&["other", "list"])).len(), 1, "the card arrived anyway");
}

/// DEFECT 2. tb's own sync is not a person: its writes are made under `github`, by origin, so
/// a restrictive list must not stop `tb sync`. Driven through a real sync with a fake `gh`.
#[test]
fn a_names_list_does_not_block_tb_sync() {
    let h = Home::new();
    h.ok(&["add", "repo: gh#310 a linked card"]);
    h.ok(&["take", "1"]);
    let day = "2026-01-01T00:00:00Z";
    std::fs::write(
        h.path().join("prs.json"),
        format!(r#"[{{"number":333,"title":"fix","headRefName":"f","isDraft":false,"reviewDecision":"","createdAt":"{day}","updatedAt":"{day}","author":{{"login":"a"}},"statusCheckRollup":[],"closingIssuesReferences":[{{"number":310}}]}}]"#),
    )
    .unwrap();
    std::fs::write(
        h.path().join("issues.json"),
        format!(r#"[{{"number":310,"title":"a linked card","labels":[],"assignees":[],"createdAt":"{day}"}}]"#),
    )
    .unwrap();
    std::fs::write(h.path().join("empty.json"), "[]").unwrap();
    let gh = h.path().join("gh");
    let p = h.path().display();
    std::fs::write(
        &gh,
        format!("#!/bin/sh\ncase \"$1 $2\" in\n  \"repo view\") echo '{{\"nameWithOwner\":\"acme/widgets\"}}';;\n  \"pr list\") case \"$*\" in *merged*) cat {p}/empty.json;; *) cat {p}/prs.json;; esac;;\n  \"issue list\") cat {p}/issues.json;;\n  \"run list\") cat {p}/empty.json;;\n  api*) echo 42;;\n  *) echo '[]';;\nesac\n"),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let run = |args: &[&str], actor: &str| -> Output { h.cmd(args, actor).env("TB_GH", &gh).output().unwrap() };
    assert!(run(&["config", "github", "acme/widgets"], "alice").status.success());

    // a list naming only alice: alice's sync still works, because the writes it makes are the
    // SYNC's, recorded under `github`, not alice's
    h.ok(&["config", "actors", "alice"]);
    let o = run(&["sync"], "alice");
    assert!(o.status.success(), "a names list blocked tb sync: {}{}", text(&o.stdout), text(&o.stderr));
    let card = h.json(&["show", "1", "--json"]);
    assert_eq!(card["column"], "review", "the sync did not move the card: {card}");
    assert!(
        card["events"].as_array().unwrap().iter().any(|e| e["actor"] == "github"),
        "the sync's write is recorded under its own name: {card}"
    );

    // and the exemption is by ORIGIN, not a name anybody may borrow: a person cannot write as
    // `github` from the command line
    let o = h.cmd(&["note", "1", "riding in"], "github").output().unwrap();
    assert!(!o.status.success(), "a person wrote as the sync actor: {}", text(&o.stdout));
}
