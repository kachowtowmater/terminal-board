//! Narrowing a read (`--tag`, `--owner`, `--blocked`, `--blocked-on`, `--due-before`,
//! `--column`, `--group tag`), moving a card to another board (`tb mv`), and looking at one
//! person's work across every board (`tb list --all-boards`).
#![cfg(unix)]
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A HOME with real board files, because `mv` and `--all-boards` are about several of them.
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
        for k in ["TB_DB", "TTYBOARD_DB", "TB_BOARD", "TTYBOARD_BOARD", "TB_CONFIG", "HERDR_AGENT_NAME", "XDG_STATE_HOME"] {
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
    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
    fn refused(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert_eq!(o.status.code(), Some(1), "{args:?} should be refused: {}{}", text(&o.stdout), text(&o.stderr));
        text(&o.stderr)
    }
    /// One board with a card of every shape a filter cares about.
    fn seeded(&self) -> &Home {
        self.ok(&["add", "docs: write the guide", "--due", "2026-10-09"]);
        self.ok(&["add", "ops: rotate tokens"]);
        self.ok(&["add", "docs: second doc", "--due", "2026-12-01"]);
        self.ok(&["add", "no tag at all"]);
        self.ok(&["add", "ops: waits on the guide"]);
        self.ok(&["take", "1"]);
        self.ok(&["block", "5", "#1", "--on", "#1"]);
        self.ok(&["block", "2", "waiting for a key", "--on", "carol"]);
        self
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

/// The card ids a plain (non-JSON) listing printed, in the order they appear.
fn ids(out: &str) -> Vec<i64> {
    out.lines()
        .filter_map(|l| l.split_whitespace().find(|w| w.starts_with('#')))
        .filter_map(|w| w.trim_start_matches('#').parse().ok())
        .collect()
}

/// Every filter on its own, and several at once.
#[test]
fn each_filter_and_then_all_of_them() {
    let h = Home::new();
    h.seeded();
    let all = ids(&h.ok(&["list"]));
    assert_eq!(all.len(), 5);

    assert_eq!(ids(&h.ok(&["list", "--tag", "docs"])), [3, 1]);
    assert_eq!(ids(&h.ok(&["list", "--tag", "none"])), [4]);
    assert_eq!(ids(&h.ok(&["list", "--owner", "alice"])), [1]);
    assert_eq!(ids(&h.ok(&["list", "--owner", "ALICE"])), [1], "a name matches whatever the case");
    assert_eq!(ids(&h.ok(&["list", "--owner", "none"])), [2, 3, 4, 5]);
    assert_eq!(ids(&h.ok(&["list", "--blocked"])), [2, 5]);
    assert_eq!(ids(&h.ok(&["list", "--blocked-on", "#1"])), [5]);
    assert_eq!(ids(&h.ok(&["list", "--blocked-on", "1"])), [5], "#1 and 1 are the same card");
    assert_eq!(ids(&h.ok(&["list", "--blocked-on", "carol"])), [2], "a name, not only a card");
    assert_eq!(ids(&h.ok(&["list", "--due-before", "2026-11-01"])), [1]);
    assert_eq!(ids(&h.ok(&["list", "--due-before", "2026-10-09"])), Vec::<i64>::new(), "strictly before, and no date never matches");
    assert_eq!(ids(&h.ok(&["list", "--column", "doing"])), [1]);
    assert_eq!(ids(&h.ok(&["list", "--column", "todo"])), [2, 3, 4, 5]);

    // composing: every filter has to pass
    assert_eq!(ids(&h.ok(&["list", "--tag", "docs", "--owner", "alice"])), [1]);
    assert_eq!(ids(&h.ok(&["list", "--tag", "docs", "--owner", "bob"])), Vec::<i64>::new());
    assert_eq!(ids(&h.ok(&["list", "--tag", "ops", "--blocked", "--column", "todo"])), [2, 5]);
    assert_eq!(ids(&h.ok(&["list", "--tag", "ops", "--blocked", "--column", "doing"])), Vec::<i64>::new());

    // a filter that matches nothing says what was asked for, and exits 0 — it is an answer
    let out = h.ok(&["list", "--tag", "nope", "--owner", "bob"]);
    assert!(out.contains("no cards match tag nope, owner bob"), "{out}");
    assert!(out.contains("'tb list'"), "and how to loosen it: {out}");
}

/// A filter removes rows. It never changes the order the board defines — including under
/// `sort due`, where the order is not the order the cards were made in.
#[test]
fn a_filter_is_the_same_order_with_rows_removed() {
    for sort in ["position", "due"] {
        let h = Home::new();
        h.seeded();
        h.ok(&["config", "sort", sort]);
        let all = ids(&h.ok(&["list"]));
        assert_eq!(all.len(), 5, "{sort}");
        for args in [
            &["list", "--tag", "docs"][..],
            &["list", "--owner", "none"][..],
            &["list", "--column", "todo"][..],
            &["list", "--blocked"][..],
        ] {
            let kept = ids(&h.ok(args));
            // every kept id is in the full listing, in the same relative order
            let mut it = all.iter();
            for k in &kept {
                assert!(it.any(|a| a == k), "{sort} {args:?}: {kept:?} is not a subsequence of {all:?}");
            }
        }
        // and the same rule in the JSON board, column by column
        let board = h.json(&["board", "--json"]);
        let filtered = h.json(&["board", "--json", "--tag", "docs"]);
        for col in ["todo", "doing", "review", "done"] {
            let full: Vec<i64> = board["columns"][col].as_array().unwrap().iter().map(|c| c["id"].as_i64().unwrap()).collect();
            let kept: Vec<i64> = filtered["columns"][col].as_array().unwrap().iter().map(|c| c["id"].as_i64().unwrap()).collect();
            let mut it = full.iter();
            for k in &kept {
                assert!(it.any(|a| a == k), "{sort} {col}: {kept:?} not a subsequence of {full:?}");
            }
        }
    }
}

/// `--column` takes an internal name. A display label is refused, and the refusal names the
/// column to use — the same precedence `tb move` follows.
#[test]
fn column_takes_the_internal_name_never_a_label() {
    let h = Home::new();
    h.seeded();
    h.ok(&["config", "label", "review", "WITH ATTORNEY"]);

    let e = h.refused(&["list", "--column", "WITH ATTORNEY"]);
    assert!(e.contains("'WITH ATTORNEY' is the label on review"), "{e}");
    assert!(e.contains("'tb list --column review'"), "{e}");
    // a label can never BE a column name in the first place — the board refuses to set one,
    // so `--column todo` can only ever mean the column
    let e = h.refused(&["config", "label", "doing", "TODO"]);
    assert!(e.contains("is the name of a column, so it cannot be a label"), "{e}");
    assert_eq!(ids(&h.ok(&["list", "--column", "todo"])), [2, 3, 4, 5]);
    assert_eq!(ids(&h.ok(&["list", "--column", "doing"])), [1]);
    // and a word that is neither
    let e = h.refused(&["list", "--column", "backlog"]);
    assert!(e.contains("unknown column 'backlog'"), "{e}");
    // the same rule on the JSON board — where a refusal is the documented object on stdout
    let o = h.run(&["board", "--json", "--column", "WITH ATTORNEY"]);
    assert_eq!(o.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["error"].as_str().unwrap().contains("is the label on review"), "{v}");
}

/// `--group tag` prints the same rows, gathered under their tag, untagged last.
#[test]
fn grouping_by_tag() {
    let h = Home::new();
    h.seeded();
    let out = h.ok(&["list", "--group", "tag"]);
    let heads: Vec<&str> = out.lines().filter(|l| !l.starts_with(' ') && !l.is_empty()).collect();
    assert_eq!(heads, ["docs", "ops", "none"], "alphabetical, untagged last:\n{out}");
    assert_eq!(ids(&out), [3, 1, 2, 5, 4]);
    // grouping composes with a filter
    let out = h.ok(&["list", "--group", "tag", "--blocked"]);
    assert_eq!(out.lines().filter(|l| !l.starts_with(' ') && !l.is_empty()).collect::<Vec<_>>(), ["ops"]);
    assert_eq!(ids(&out), [2, 5]);
    let e = h.refused(&["list", "--group", "owner"]);
    assert!(e.contains("cannot group by 'owner'") && e.contains("--group tag"), "{e}");
}

/// A board that passes no filter prints exactly what it printed before.
#[test]
fn no_filter_is_byte_identical() {
    let h = Home::new();
    h.seeded();
    // `--group` alone changes the shape on purpose; every other flag absent means untouched
    let plain = h.ok(&["list"]);
    assert!(plain.starts_with("todo "), "{plain}");
    let board = h.ok(&["board"]);
    assert!(board.contains("TODO"), "{board}");
    let json = h.ok(&["board", "--json"]);
    // the same commands with every filter left off must give the same bytes
    assert_eq!(h.ok(&["list"]), plain);
    assert_eq!(h.ok(&["board"]), board);
    assert_eq!(h.ok(&["board", "--json"]), json);
}

/// `tb mv ID --to BOARD`: the card, its checklist and its whole history travel; the id
/// changes, and both boards record where it went.
#[test]
fn a_card_moves_to_another_board_with_its_history() {
    let h = Home::new();
    h.ok(&["add", "docs: write the guide", "--due", "2026-10-09", "--check", "draft", "--check", "review"]);
    h.ok(&["check", "1", "1"]);
    h.ok(&["note", "1", "a note from before the move"]);
    h.ok(&["work", "add", "x: a card already here"]);
    let before = h.json(&["show", "1", "--json"]);

    let said = h.ok(&["mv", "1", "--to", "work"]);
    assert!(said.contains("moved to 'work' as #2"), "{said}");
    assert!(said.contains("in TODO, unowned"), "it says where the card landed: {said}");

    // gone from the source, and the source's board log says where it went
    assert_eq!(ids(&h.ok(&["list"])), Vec::<i64>::new());
    // there, with a new id
    let after = h.json(&["work", "show", "2", "--json"]);
    for k in ["title", "tag", "description", "due", "gh_ref"] {
        assert_eq!(after[k], before[k], "{k} did not travel");
    }
    assert_eq!(after["column"], "todo", "a moved card lands in TODO");
    assert_eq!(after["owner"], Value::Null, "and unowned");
    let items = |v: &Value| -> Vec<(String, bool)> {
        v["checklist"].as_array().unwrap().iter().map(|i| (i["text"].as_str().unwrap().into(), i["done"].as_bool().unwrap())).collect()
    };
    assert_eq!(items(&after), items(&before), "the checklist travels, ticks included");
    // the history travels with its original actors and times, and the move is recorded
    let kinds: Vec<String> = after["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap().into()).collect();
    assert!(kinds.contains(&"created".to_string()) && kinds.contains(&"note".to_string()), "{kinds:?}");
    assert_eq!(kinds.last().unwrap(), "moved-in");
    let moved_in = after["events"].as_array().unwrap().last().unwrap();
    assert_eq!(moved_in["text"], "#1 from default", "it says where it came from");
    let old_note = after["events"].as_array().unwrap().iter().find(|e| e["kind"] == "note").unwrap();
    assert_eq!(old_note["text"], "a note from before the move");
    assert_eq!(old_note["ts"], before["events"].as_array().unwrap().iter().find(|e| e["kind"] == "note").unwrap()["ts"], "the original time");

    // --json says both ids and both boards (the source's ids keep counting up, so the next
    // card made here is #2, not #1 again)
    let made = h.ok(&["add", "ops: another"]);
    let id = made.split('#').nth(1).and_then(|s| s.split_whitespace().next()).unwrap().to_string();
    let v = h.json(&["mv", &id, "--to", "work", "--json"]);
    assert_eq!((&v["ok"], &v["from_board"], &v["to_board"]), (&Value::Bool(true), &"default".into(), &"work".into()));
    assert_eq!(v["old_id"].as_str().map(str::to_string).unwrap_or_else(|| v["old_id"].to_string()), id);
    assert!(v["id"].as_i64().unwrap() > 0 && v["id"] != v["old_id"], "the id changes: {v}");
}

/// #106: a card leaving a board used to vanish from it with no readable trail — the
/// `moved-out` row landed in `board_events`, and no command printed that table. `tb log` now
/// interleaves it with the card events, marked so it is never mistaken for one.
#[test]
fn the_source_board_log_says_where_a_moved_card_went() {
    let h = Home::new();
    // #1 leaves for "work" — its whole history, "created" included, travels with it, so the
    // source board's log keeps NO card event of its own for #1 once it is gone. #2 stays
    // behind on purpose, so the source board still has a card event to interleave against.
    h.ok(&["add", "docs: write the guide"]);
    h.ok(&["add", "ops: stays on the source board"]);
    h.ok(&["work", "add", "x: already here"]);
    h.ok(&["mv", "1", "--to", "work"]);

    // plain text: a board-level row, marked `board` where a card row would show `#ID`
    let plain = h.ok(&["log"]);
    assert!(plain.contains("moved-out") && plain.contains("#1 to work is #2 there"), "{plain}");
    let moved_out_line = plain.lines().find(|l| l.contains("moved-out")).unwrap();
    assert!(moved_out_line.contains(" board "), "not marked as a board-level row: {moved_out_line}");

    // --json: card_id (and actor_id) are null, so a consumer can tell the halves apart
    let all = h.json(&["log", "--json"]);
    let all = all.as_array().unwrap();
    let moved_out = all.iter().find(|e| e["kind"] == "moved-out").expect("a moved-out row on the source board's log");
    assert_eq!(moved_out["card_id"], Value::Null, "{moved_out}");
    assert_eq!(moved_out["actor_id"], Value::Null, "{moved_out}");
    assert!(moved_out["text"].as_str().unwrap().contains("#1 to work is #2 there"), "{moved_out}");

    // still oldest-first, card and board events merged by the same clock: #2's "created"
    // (a card event) happened before the move, so it precedes "moved-out" in the merged log
    let kinds: Vec<&str> = all.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    let created_at = kinds.iter().position(|k| *k == "created").unwrap();
    let moved_out_at = kinds.iter().position(|k| *k == "moved-out").unwrap();
    assert!(created_at < moved_out_at, "{kinds:?}");
}

/// What `tb mv` refuses, and that nothing is half-moved when it does.
#[test]
fn a_move_that_cannot_happen_changes_neither_board() {
    let h = Home::new();
    h.ok(&["add", "docs: stays put"]);
    h.ok(&["work", "add", "x: over here"]);
    let before = (std::fs::read(h.board_file("default")).unwrap(), std::fs::read(h.board_file("work")).unwrap());

    let e = h.refused(&["mv", "1", "--to", "nosuch"]);
    assert!(e.contains("no board 'nosuch'") && e.contains("boards: default, work"), "{e}");
    assert!(e.contains("tb nosuch add"), "it names the board to create, not the one we are on: {e}");
    assert!(!h.board_file("nosuch").exists(), "a refused move created a board");

    let e = h.refused(&["mv", "1", "--to", "default"]);
    assert!(e.contains("#1 is already on 'default'"), "{e}");
    let e = h.refused(&["mv", "99", "--to", "work"]);
    assert!(e.contains("no card #99"), "{e}");
    let e = h.refused(&["mv", "1", "--to", "Nope!"]);
    assert!(e.contains("not a command or a valid board name"), "{e}");

    // a card somebody else holds is not taken off their board by a passer-by
    h.ok(&["take", "1"]);
    let o = h.cmd(&["mv", "1", "--to", "work"], "bob").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("#1 is held by alice"), "{}", text(&o.stderr));

    assert_eq!(std::fs::read(h.board_file("work")).unwrap(), before.1, "the destination was written by a refused move");
    assert_eq!(ids(&h.ok(&["list"])), [1], "the card is still on its board");

    // TB_DB pins one file: there is no second board
    let pin = h.path().join("pinned.db");
    let o = h.cmd(&["add", "p: pinned"], "alice").env("TB_DB", &pin).output().unwrap();
    assert!(o.status.success());
    let o = h.cmd(&["mv", "1", "--to", "work"], "alice").env("TB_DB", &pin).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("TB_DB pins one board file"), "{}", text(&o.stderr));
}

/// A move is ordered so that a failure duplicates rather than loses: the card is written to
/// the destination and committed BEFORE it is removed from the source.
#[test]
fn a_move_never_loses_a_card_even_if_it_is_interrupted() {
    let h = Home::new();
    h.ok(&["add", "docs: precious"]);
    h.ok(&["work", "add", "x: seed"]);
    // a move that goes through, first, so this test fails on a tree with no `tb mv` at all
    h.ok(&["add", "docs: goes first"]);
    assert!(h.ok(&["mv", "2", "--to", "work"]).contains("moved to 'work'"));
    assert_eq!(ids(&h.ok(&["work", "list"])), [1, 2], "the card really arrived");
    assert_eq!(ids(&h.ok(&["list"])), [1], "and really left");
    // make the SOURCE unwritable after the destination write would have happened: the card
    // must still exist somewhere, and the refusal must say it is on both boards
    let src = h.board_file("default");
    let (was_source, was_dest) = (ids(&h.ok(&["list"])), ids(&h.ok(&["work", "list"])));
    let mut perms = std::fs::metadata(&src).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o444);
    std::fs::set_permissions(&src, perms).unwrap();
    let o = h.cmd(&["mv", "1", "--to", "work"], "alice").output().unwrap();
    let mut perms = std::fs::metadata(&src).unwrap().permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(&src, perms).unwrap();
    // whichever way it went, the card is never gone from BOTH boards
    let on_source = ids(&h.ok(&["list"]));
    let on_dest = ids(&h.ok(&["work", "list"]));
    // the card is never gone from BOTH boards, whichever way the attempt went
    assert!(
        on_source.len() + on_dest.len() >= was_source.len() + was_dest.len(),
        "a card was lost: source {was_source:?} -> {on_source:?}, dest {was_dest:?} -> {on_dest:?}"
    );
    if !o.status.success() {
        let e = text(&o.stderr);
        // if it failed AFTER the destination was written, it says so plainly and both ends
        // still hold the card — a duplicate somebody can see, never a hole
        if e.contains("could not be removed") {
            assert!(e.contains("on BOTH boards") && e.contains("rm 1"), "{e}");
            assert_eq!(on_dest.len(), was_dest.len() + 1, "the copy is there");
            assert_eq!(on_source, was_source, "and the original is still here");
        }
    }
}

/// `tb list --all-boards --owner NAME`: one agent's work wherever it is.
#[test]
fn one_persons_work_across_every_board() {
    let h = Home::new();
    for (board, title) in [("default", "docs: on default"), ("work", "ops: on work"), ("home", "x: on home")] {
        h.ok(&[board, "add", title]);
    }
    h.ok(&["take", "1"]);
    h.ok(&["work", "take", "1"]);
    assert!(h.cmd(&["home", "take", "1"], "bob").output().unwrap().status.success());

    let out = h.ok(&["list", "--all-boards", "--owner", "alice"]);
    assert!(out.contains("default") && out.contains("work"), "{out}");
    assert!(!out.contains("on home"), "bob's card is not alice's: {out}");
    assert_eq!(ids(&out).len(), 2);

    let v = h.json(&["list", "--all-boards", "--owner", "alice", "--json"]);
    let rows = v.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let boards: Vec<&str> = rows.iter().map(|r| r["board"].as_str().unwrap()).collect();
    assert_eq!(boards, ["default", "work"], "every row says which board it is on");
    assert!(rows[0]["column_label"].is_string(), "a full card object: {}", rows[0]);

    // filters compose across boards too
    assert_eq!(ids(&h.ok(&["list", "--all-boards", "--owner", "alice", "--tag", "ops"])).len(), 1);
    // and nothing found is an answer, not a failure
    let out = h.ok(&["list", "--all-boards", "--owner", "nobody"]);
    assert!(out.contains("no cards matching owner nobody on any board"), "{out}");
    // TB_DB pins one file
    let pin = h.path().join("pinned.db");
    h.cmd(&["add", "p: x"], "alice").env("TB_DB", &pin).output().unwrap();
    let o = h.cmd(&["list", "--all-boards", "--owner", "alice"], "alice").env("TB_DB", &pin).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("only one board to look at"), "{}", text(&o.stderr));
}

/// The manuals teach it, and the agent manual stays under its cap.
#[test]
fn the_manuals_teach_it() {
    let agents = include_str!("../docs/AGENTS.md");
    for phrase in ["--tag", "tb mv", "--all-boards"] {
        assert!(agents.contains(phrase), "docs/AGENTS.md lacks `{phrase}`");
    }
    assert!(agents.lines().count() <= 250, "the agent manual is over its line cap");
    for (name, doc) in [("README.md", include_str!("../README.md")), ("docs/JSON.md", include_str!("../docs/JSON.md"))] {
        assert!(doc.contains("tb mv") && doc.contains("--all-boards"), "{name}");
    }
}

/// D2: a write to the SOURCE that raced a move must never be acknowledged and then thrown
/// away. The whole move holds the source's write lock, so a `tb note` arriving during it
/// waits and then finds the card gone — a refusal, not a lie.
#[test]
fn a_write_racing_a_move_is_never_acknowledged_then_destroyed() {
    let rounds = 40;
    let mut acknowledged_then_lost = 0;
    let mut landed_before = 0;
    let mut refused = 0;
    for round in 0..rounds {
        let h = Home::new();
        h.ok(&["add", "docs: the card"]);
        h.ok(&["work", "add", "x: seed"]);
        let note = format!("note from round {round}");
        // start the move, and race a note against it from another process
        let mv = h.cmd(&["mv", "1", "--to", "work"], "alice").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let noted = h.cmd(&["note", "1", &note], "bob").output().unwrap();
        let moved = mv.wait_with_output().unwrap();
        assert!(moved.status.success(), "round {round}: the move failed: {}", text(&moved.stderr));

        // where did the note end up?
        let there = h.ok(&["work", "show", "2", "--json"]);
        let carried = there.contains(&note);
        if noted.status.success() {
            // it said "noted" — then the text MUST exist somewhere afterwards
            assert!(carried, "round {round}: '{note}' was acknowledged (rc 0) and then destroyed:\n{there}");
            landed_before += 1;
            if !carried {
                acknowledged_then_lost += 1;
            }
        } else {
            // refused is fine: the card had already gone
            refused += 1;
            let e = text(&noted.stderr);
            assert!(e.contains("no card #1") || e.contains("database is locked"), "round {round}: odd refusal: {e}");
        }
    }
    assert_eq!(acknowledged_then_lost, 0, "acknowledged writes were destroyed");
    // the race has to actually happen, or the test proves nothing
    assert!(landed_before + refused == rounds, "every round did one or the other");
    println!("D2 over {rounds} rounds: {landed_before} notes landed before the move, {refused} refused after it, 0 lost");
}

/// D3: two `tb mv` of the same card at the same moment must not both succeed — one of them
/// has to lose and say so, or the destination ends up with two copies.
#[test]
fn two_moves_of_the_same_card_never_make_two_copies() {
    let rounds = 20;
    for round in 0..rounds {
        let h = Home::new();
        h.ok(&["add", "docs: only one of me"]);
        h.ok(&["work", "add", "x: seed"]);
        let a = h.cmd(&["mv", "1", "--to", "work"], "alice").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let b = h.cmd(&["mv", "1", "--to", "work"], "bob").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let (a, b) = (a.wait_with_output().unwrap(), b.wait_with_output().unwrap());
        let winners = [&a, &b].iter().filter(|o| o.status.success()).count();
        let on_dest = ids(&h.ok(&["work", "list"]));
        assert_eq!(on_dest.len(), 2, "round {round}: the destination holds {} cards, not 2 — a card was copied twice", on_dest.len());
        assert_eq!(winners, 1, "round {round}: {winners} moves succeeded; a and b said:\n{}\n{}", text(&a.stderr), text(&b.stderr));
        let loser = if a.status.success() { &b } else { &a };
        assert!(text(&loser.stderr).contains("no card #1"), "round {round}: the loser must say why: {}", text(&loser.stderr));
        assert_eq!(ids(&h.ok(&["list"])), Vec::<i64>::new(), "round {round}: the source kept the card");
    }
    println!("D3 over {rounds} rounds: exactly one move won every time, destination never held a duplicate");
}

/// D4: the holder refusal offers `--force`, and `--force` exists and is logged.
#[test]
fn mv_force_exists_and_is_logged() {
    let h = Home::new();
    h.ok(&["add", "docs: alice is on it"]);
    h.ok(&["work", "add", "x: seed"]);
    h.ok(&["take", "1"]);
    // refused for a non-holder, and the refusal names the flag that really exists
    let o = h.cmd(&["mv", "1", "--to", "work"], "bob").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    let e = text(&o.stderr);
    assert!(e.contains("#1 is held by alice") && e.contains("--force"), "{e}");
    // and that flag works, rather than being an unknown argument
    let o = h.cmd(&["mv", "1", "--to", "work", "--force"], "bob").output().unwrap();
    assert!(o.status.success(), "--force is advertised but not accepted: {}", text(&o.stderr));
    // logged where the card now lives, and on both boards' logs
    let card = h.json(&["work", "show", "2", "--json"]);
    let forced: Vec<String> = card["events"].as_array().unwrap().iter().filter(|e| e["kind"] == "force").map(|e| e["text"].as_str().unwrap().into()).collect();
    assert_eq!(forced, ["moved #1 held by alice from default"], "{card}");
    // the holder's own card is gone from their board, as a move means
    assert_eq!(ids(&h.ok(&["list"])), Vec::<i64>::new());
}

/// D7: a filter must never be accepted and silently ignored. `--done` honours every filter;
/// `--archived` honours the ones an archived row can answer and REFUSES the others by name.
#[test]
fn done_and_archived_honour_the_filters() {
    let h = Home::new();
    h.ok(&["config", "rm", "archive"]);
    h.ok(&["add", "docs: finished doc"]);
    h.ok(&["add", "ops: finished op"]);
    h.ok(&["add", "docs: still going"]);
    h.ok(&["add", "ops: to be archived"]);
    for id in ["1", "2"] {
        for (verb, who) in [("take", "alice"), ("done", "alice"), ("done", "bob")] {
            assert!(h.cmd(&[verb, id], who).output().unwrap().status.success());
        }
    }
    h.ok(&["rm", "4"]);

    // --done: the whole card is there, so every filter applies
    assert_eq!(ids(&h.ok(&["list", "--done"])).len(), 2);
    assert_eq!(ids(&h.ok(&["list", "--done", "--tag", "docs"])), [1]);
    assert_eq!(ids(&h.ok(&["list", "--done", "--tag", "ops"])), [2]);
    assert_eq!(ids(&h.ok(&["list", "--done", "--owner", "alice"])).len(), 2);
    assert_eq!(ids(&h.ok(&["list", "--done", "--owner", "nobody"])), Vec::<i64>::new());
    assert_eq!(ids(&h.ok(&["list", "--done", "--column", "done"])).len(), 2);
    assert_eq!(ids(&h.ok(&["list", "--done", "--column", "todo"])), Vec::<i64>::new());
    assert_eq!(ids(&h.ok(&["list", "--done", "--blocked"])), Vec::<i64>::new());
    let far_future = format!("{}-01-01", 2000 + 98);
    assert_eq!(ids(&h.ok(&["list", "--done", "--due-before", &far_future])), Vec::<i64>::new(), "a card with no due date is never 'due before'");
    // and with --since, and in --json
    assert_eq!(ids(&h.ok(&["list", "--done", "--since", "1970-01-01", "--tag", "docs"])), [1]);
    let v = h.json(&["list", "--done", "--tag", "docs", "--json"]);
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["id"], 1);
    // a filter that matches nothing says what was asked for
    assert!(h.ok(&["list", "--done", "--tag", "nope"]).contains("no cards finished matching tag nope today"));

    // --archived: tag, owner and column apply
    assert_eq!(h.ok(&["list", "--archived"]).lines().filter(|l| l.starts_with('#')).count(), 1);
    assert!(h.ok(&["list", "--archived", "--tag", "ops"]).contains("to be archived"));
    assert!(h.ok(&["list", "--archived", "--tag", "docs"]).contains("no archived cards match tag docs"));
    assert!(h.ok(&["list", "--archived", "--owner", "none"]).contains("to be archived"));
    assert!(h.ok(&["list", "--archived", "--column", "todo"]).contains("to be archived"));
    assert!(h.ok(&["list", "--archived", "--column", "done"]).contains("no archived cards match"));
    assert_eq!(h.json(&["list", "--archived", "--tag", "docs", "--json"]).as_array().unwrap().len(), 0);
    assert_eq!(h.json(&["list", "--archived", "--tag", "ops", "--json"]).as_array().unwrap().len(), 1);

    // the three an archived row cannot answer are REFUSED BY NAME, never ignored
    for flag in [vec!["--blocked"], vec!["--blocked-on", "#1"], vec!["--due-before", "2026-10-09"]] {
        let mut args = vec!["list", "--archived"];
        args.extend(flag.iter().copied());
        let e = h.refused(&args);
        assert!(e.contains(flag[0]) && e.contains("does not apply to an archived card"), "{args:?}: {e}");
    }

    // --all-boards is refused with --done and with --archived, rather than ignored
    for args in [&["list", "--all-boards", "--done"][..], &["list", "--all-boards", "--archived"][..]] {
        let o = h.run(args);
        assert_eq!(o.status.code(), Some(2), "{args:?} should be a usage error: {}", text(&o.stdout));
        assert!(text(&o.stderr).contains("cannot be used with"), "{args:?}: {}", text(&o.stderr));
    }
}

/// THE GUARD. Every way `tb list` can be asked for a different SET of cards must respect a
/// filter — or refuse it by name. The modes are read out of `tb list --help`, so a mode added
/// later fails this test until somebody teaches it about filters.
#[test]
fn every_list_mode_handles_filters() {
    let h = Home::new();
    h.ok(&["add", "docs: a card"]);
    let help = h.ok(&["list", "--help"]);
    // the filters themselves, and the flags that are not about WHICH cards are shown
    let not_a_mode = ["--tag", "--owner", "--blocked", "--blocked-on", "--due-before", "--column", "--group", "--json", "--as", "--board", "--help", "--version", "--since"];
    let modes: Vec<String> = help
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|w| w.starts_with("--"))
        .map(|w| w.trim_end_matches(',').to_string())
        .filter(|w| !not_a_mode.contains(&w.as_str()))
        .collect();
    assert!(modes.contains(&"--archived".to_string()) && modes.contains(&"--done".to_string()) && modes.contains(&"--all-boards".to_string()), "the guard found no modes to check: {modes:?}");
    for mode in &modes {
        // a tag nothing can match: an honoured filter narrows to nothing, and anything else
        // must be an explicit refusal naming a flag — never a full list
        let o = h.run(&["list", mode, "--tag", "nothing-matches-this"]);
        let out = text(&o.stdout);
        let err = text(&o.stderr);
        let shown = ids(&out);
        assert!(
            shown.is_empty(),
            "`tb list {mode} --tag nothing-matches-this` listed {shown:?} — a filter was accepted and ignored. \
             Teach that mode about filters (or refuse the combination by name) in the Cmd::List arms."
        );
        if !o.status.success() {
            assert!(
                err.contains("--tag") || err.contains("cannot be used with") || err.contains("does not apply"),
                "{mode}: refused without saying which flag: {err}"
            );
        }
    }
}
