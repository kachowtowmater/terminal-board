//! Who may close a card (`done-by`), who has checked it (`--approve` on any card), and the
//! tag the user chose (`--tag`).
//!
//! `done-by` is an HONEST-MISTAKE STOP, not security: names are self-asserted and `--force`
//! is open to everyone. These tests pin that it stops the slip, that it is documented as
//! what it is, and that it never contradicts the older never-self-approve rule.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const NOW: i64 = 1_790_856_000; // 2026-10-01T12:00:00Z

struct Board {
    _dir: tempfile::TempDir,
    db: PathBuf,
}

impl Board {
    fn new() -> Board {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("board.db");
        Board { _dir: dir, db }
    }

    fn as_who(&self, who: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("TB_DB", &self.db)
            .env("TB_AS", who)
            .env("TB_NO_HERDR", "1")
            .env("TZ", "UTC")
            .env("TB_NOW", NOW.to_string())
            .env_remove("TB_BOARD")
            .env_remove("HERDR_AGENT_NAME")
            .output()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.as_who("alice", args)
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn ok_as(&self, who: &str, args: &[&str]) -> String {
        let o = self.as_who(who, args);
        assert!(o.status.success(), "[{who}] {args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let out = self.ok(args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    fn refused(&self, who: &str, args: &[&str]) -> String {
        let o = self.as_who(who, args);
        assert_eq!(o.status.code(), Some(1), "[{who}] {args:?} should be refused: {}", String::from_utf8_lossy(&o.stdout));
        String::from_utf8_lossy(&o.stderr).trim().to_string()
    }

    fn column(&self, id: i64) -> String {
        self.json(&["show", &id.to_string(), "--json"])["column"].as_str().unwrap().to_string()
    }

    /// A card in REVIEW, done by `worker`.
    fn in_review(&self, title: &str, worker: &str) -> i64 {
        self.ok(&["add", title]);
        let id = self.json(&["list", "--json"]).as_array().unwrap().len() as i64;
        self.ok_as(worker, &["take", &id.to_string()]);
        self.ok_as(worker, &["done", &id.to_string()]);
        id
    }
}

// ------------------------------------------------------------------ C9: who may close

#[test]
fn done_by_refuses_anyone_else_and_says_how_to_get_past_it() {
    let b = Board::new();
    b.ok(&["config", "wip", "9"]);
    assert_eq!(b.ok(&["config", "done-by"]).trim(), "anyone", "the default: nothing changes");
    let one = b.in_review("permits: renewal", "bob");
    assert_eq!(
        b.ok(&["config", "done-by", "anna,ben"]).trim(),
        "done-by is now anna, ben — only they may close a card. It stops an honest mistake, not an attacker: names are self-asserted and --force is logged but open to all"
    );
    assert_eq!(b.json(&["config", "done-by", "--json"])["config"]["value"], serde_json::json!(["anna", "ben"]));
    assert!(b.ok(&["config"]).contains(&format!("{:<13} {}", "done-by", "anna,ben")), "listed once set");

    let e = b.refused("carol", &["done", &one.to_string()]);
    assert_eq!(
        e,
        format!("tb: only anna or ben may close a card on this board (carol is not on the list) — ask one of them to run 'tb done {one}', or 'tb done {one} --force' if you mean it (logged)")
    );
    assert_eq!(b.column(one), "review", "nothing moved");
    // a name on the list may close it, whatever case it is typed in
    b.ok_as("ANNA", &["done", &one.to_string()]);
    assert_eq!(b.column(one), "done");

    // every way into DONE is guarded — moving the card elsewhere first is not a way round
    let two = b.in_review("tax: return", "bob");
    assert!(b.refused("carol", &["move", &two.to_string(), "done"]).contains("only anna or ben may close"));
    b.ok_as("carol", &["move", &two.to_string(), "todo"]);
    assert!(b.refused("carol", &["done", &two.to_string()]).contains("only anna or ben may close"), "todo -> done is closing too");
    assert_eq!(b.column(two), "todo");
    // `--force` is open to everyone, and logged as its own event: that is the honest-mistake bargain
    b.ok_as("carol", &["done", &two.to_string(), "--force"]);
    assert_eq!(b.column(two), "done");
    let forced: Vec<String> = b.json(&["show", &two.to_string(), "--json"])["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "force")
        .map(|e| format!("{}: {}", e["actor"].as_str().unwrap(), e["text"].as_str().unwrap()))
        .collect();
    assert_eq!(forced, [format!("carol: closed #{two}, not on the done-by list")]);

    // clearing it lets anyone close again
    assert_eq!(b.ok(&["config", "done-by", "--off"]).trim(), "done-by is off — anyone may close a card");
    let three = b.in_review("lease: notice", "bob");
    b.ok_as("carol", &["done", &three.to_string()]);
    assert_eq!(b.column(three), "done");
    assert!(!b.ok(&["config"]).contains("done-by"), "cleared = no longer listed");
    for bad in ["", " , "] {
        let e = b.refused("alice", &["config", "done-by", bad]);
        assert!(e.contains("say who may close a card") && e.contains("'tb config done-by --off'"), "{bad:?}: {e}");
    }
}

/// `done-by` never contradicts the older rule: a name on the list still cannot approve its
/// own work, and the more specific refusal is the one you get.
#[test]
fn a_name_on_the_list_still_cannot_close_its_own_work() {
    let b = Board::new();
    b.ok(&["config", "done-by", "anna,ben"]);
    let id = b.in_review("permits: renewal", "anna");
    let e = b.refused("anna", &["done", &id.to_string()]);
    assert_eq!(e, "tb: you did this work — ask another person or agent to review it", "the older rule answers first");
    assert_eq!(b.column(id), "review");
    b.ok_as("ben", &["done", &id.to_string()]);
    assert_eq!(b.column(id), "done");
    // the author rule is about the OWNER (#86), so a list name that merely moved the card may close it
    let other = b.in_review("tax: return", "worker");
    b.ok_as("anna", &["done", &other.to_string()]);
    assert_eq!(b.column(other), "done");
}

/// Nobody can hand-wave past `done-by` by claiming to be the sync: the CLI refuses the name
/// `github` outright (the identity rule), so the exemption is reachable only from tb's own
/// sync code. That the sync itself is exempt is pinned in `store::closing`'s unit tests.
#[test]
fn nobody_can_close_by_claiming_to_be_the_github_sync() {
    let b = Board::new();
    b.ok(&["config", "done-by", "anna"]);
    let id = b.in_review("widgets: gh#7 fix it", "bob");
    assert!(b.refused("carol", &["done", &id.to_string()]).contains("only anna may close"));
    let e = b.refused("github", &["done", &id.to_string()]);
    assert!(e.contains("is the name tb's own GitHub sync acts under"), "{e}");
    assert_eq!(b.column(id), "review");
}

// ------------------------------------------------------------- C10: approve on any card

#[test]
fn approve_records_who_checked_a_card_and_leaves_it_in_review() {
    let b = Board::new();
    b.ok(&["config", "wip", "9"]);
    let plain = b.in_review("permits: renewal", "bob"); // no gh ref at all
    let out = b.ok_as("carol", &["done", &plain.to_string(), "--approve"]);
    assert_eq!(out.trim(), format!("#{plain} checked by carol — it stays in review — close it with 'tb done {plain}'"));
    assert_eq!(b.column(plain), "review", "an approval never moves the card");
    let d = b.json(&["show", &plain.to_string(), "--json"]);
    assert_eq!(d["approved_by"], serde_json::json!(["carol"]));
    let last = d["events"].as_array().unwrap().last().unwrap().clone();
    assert_eq!((last["kind"].as_str(), last["actor"].as_str(), last["text"].as_str()), (Some("approved"), Some("carol"), Some("checked by carol (it stays in review)")));
    assert!(b.ok(&["show", &plain.to_string()]).contains("carol approved: checked by carol"), "the history reads plainly");
    // several checkers, each recorded once, oldest first; the board object carries it too
    b.ok_as("dave", &["done", &plain.to_string(), "--approve"]);
    b.ok_as("CAROL", &["done", &plain.to_string(), "--approve"]);
    let on_board = b.json(&["board", "--json"])["columns"]["review"].as_array().unwrap()[0].clone();
    assert_eq!(on_board["approved_by"], serde_json::json!(["carol", "dave"]));
    // a card that IS linked to an issue says what it waits for instead
    b.ok(&["add", "widgets: gh#7 fix it"]);
    let gh = b.json(&["list", "--json"]).as_array().unwrap().len() as i64;
    b.ok_as("bob", &["take", &gh.to_string()]);
    b.ok_as("bob", &["done", &gh.to_string()]);
    let out = b.ok_as("carol", &["done", &gh.to_string(), "--approve"]);
    assert_eq!(out.trim(), format!("#{gh} checked by carol — it stays in review until the gh#7 PR merges"));
    // the rules that already applied still do
    assert!(b.refused("bob", &["done", &plain.to_string(), "--approve"]).contains("you did this work"));
    b.ok(&["add", "ops: not in review"]);
    let todo = b.json(&["list", "--json"]).as_array().unwrap().len() as i64;
    let e = b.refused("carol", &["done", &todo.to_string(), "--approve"]);
    assert!(e.contains("is not in review") && e.contains(&format!("'tb move {todo} review'")), "{e}");
    // `done-by` does not gate a check: noting "I looked at this" is not closing it
    b.ok(&["config", "done-by", "anna"]);
    b.ok_as("carol", &["done", &plain.to_string(), "--approve"]);
    assert!(b.refused("carol", &["done", &plain.to_string()]).contains("only anna may close"));
    assert_eq!(b.json(&["show", &plain.to_string(), "--json"])["approved_by"], serde_json::json!(["carol", "dave"]));
}

// ------------------------------------------------------------------ A2: explicit tag

#[test]
fn an_explicit_tag_may_hold_digits_spaces_and_hyphens_and_wins_over_the_title() {
    let b = Board::new();
    // exactly the client's case: a title whose colon is not a tag
    b.ok(&["add", "due 10/9 (file by 10/6): prepare the brief", "--tag", "00-key 2"]);
    let c = b.json(&["show", "1", "--json"]);
    assert_eq!((c["tag"].as_str(), c["title"].as_str()), (Some("00-key 2"), Some("due 10/9 (file by 10/6): prepare the brief")));
    // without the flag, that title is what it always was: no tag, nothing stripped
    b.ok(&["add", "due 10/9 (file by 10/6): prepare the reply"]);
    let c = b.json(&["show", "2", "--json"]);
    assert_eq!((c["tag"].as_str(), c["title"].as_str()), (None, Some("due 10/9 (file by 10/6): prepare the reply")));
    // an explicit tag wins over a `tag:` prefix AND keeps the title whole — tb guesses
    // nothing about a title when it was told the tag
    b.ok(&["add", "widgets: fix it", "--tag", "filing"]);
    let c = b.json(&["show", "3", "--json"]);
    assert_eq!((c["tag"].as_str(), c["title"].as_str()), (Some("filing"), Some("widgets: fix it")));
    // a LEADING gh#N is still pulled out of the title, with or without the flag; one later
    // in the text stays there and still links, exactly as JSON.md has always said
    b.ok(&["add", "gh#7 fix it", "--tag", "filing"]);
    let c = b.json(&["show", "4", "--json"]);
    assert_eq!((c["tag"].as_str(), c["gh_ref"].as_i64(), c["title"].as_str()), (Some("filing"), Some(7), Some("fix it")));
    b.ok(&["add", "widgets: gh#8 fix it", "--tag", "filing"]);
    let c = b.json(&["show", "5", "--json"]);
    assert_eq!((c["tag"].as_str(), c["gh_ref"].as_i64(), c["title"].as_str()), (Some("filing"), Some(8), Some("widgets: gh#8 fix it")));
    // it is lower-cased and its spacing collapsed, like a guessed tag
    b.ok(&["add", "x", "--tag", "  Client-A   Matter 7 "]);
    assert_eq!(b.json(&["show", "6", "--json"])["tag"], "client-a matter 7");
    // and it shows up where a tag shows up
    assert!(b.ok(&["list"]).contains("[00-key 2 - 0m]"), "{}", b.ok(&["list"]));
}

#[test]
fn an_explicit_tag_is_set_cleared_and_survives_a_title_edit() {
    let b = Board::new();
    b.ok(&["add", "prepare the brief"]);
    assert!(b.json(&["show", "1", "--json"])["tag"].is_null());
    b.ok(&["edit", "1", "--tag", "00-key 2"]);
    assert_eq!(b.json(&["show", "1", "--json"])["tag"], "00-key 2", "--tag alone is a whole edit");
    // editing the title later must not drop a tag no title could carry
    b.ok(&["edit", "1", "--title", "prepare the reply"]);
    let c = b.json(&["show", "1", "--json"]);
    assert_eq!((c["tag"].as_str(), c["title"].as_str()), (Some("00-key 2"), Some("prepare the reply")));
    // a tag that COULD come from a title still behaves exactly as before: the title decides
    b.ok(&["edit", "1", "--tag", "filing"]);
    b.ok(&["edit", "1", "--title", "widgets: fix it"]);
    let c = b.json(&["show", "1", "--json"]);
    assert_eq!((c["tag"].as_str(), c["title"].as_str()), (Some("widgets"), Some("fix it")), "a guessed tag is still guessed");
    // `none` clears it
    b.ok(&["edit", "1", "--tag", "none"]);
    assert!(b.json(&["show", "1", "--json"])["tag"].is_null());
    // title and tag in one command, and the event says what changed
    b.ok(&["edit", "1", "--title", "ops: rotate tokens", "--tag", "00-key 2"]);
    let c = b.json(&["show", "1", "--json"]);
    assert_eq!((c["tag"].as_str(), c["title"].as_str()), (Some("00-key 2"), Some("ops: rotate tokens")));
    // refusals name the command to run, and write nothing
    for (bad, want) in [("a:b", "letters, digits, spaces, hyphens and underscores"), ("", "the tag is empty"), ("aaaaaaaaaaaaaaaaaaaaaaaa", "the limit is 20")] {
        let o = b.as_who("alice", &["edit", "1", "--tag", bad, "--json"]);
        assert_eq!(o.status.code(), Some(1), "{bad:?}");
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert!(v["error"].as_str().unwrap().contains(want), "{bad:?}: {v}");
        assert!(v["hint"].as_str().unwrap().contains("'tb edit 1 --tag filing'"), "{bad:?}: {v}");
        assert_eq!(b.json(&["show", "1", "--json"])["tag"], "00-key 2", "{bad:?} changed something");
    }
    let o = b.as_who("alice", &["add", "x", "--tag", "a/b", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(v["hint"].as_str().unwrap().contains("'tb add \"title\" --tag filing'"), "{v}");
    assert_eq!(b.json(&["list", "--json"]).as_array().unwrap().len(), 1, "no card was created");
}

/// A board that sets nothing and passes no flag is what it always was.
#[test]
fn a_board_that_sets_nothing_is_unchanged() {
    let b = Board::new();
    b.ok(&["add", "widgets: gh#7 fix it", "-d", "Done = fixed", "--check", "repro"]);
    b.ok(&["add", "00-key 2: x"]);
    b.ok(&["add", "see http://x"]);
    // the tags tb guesses are exactly the ones it guessed before — including the two the
    // client reported as `null`
    let list = b.json(&["list", "--json"]);
    let tags: Vec<Option<&str>> = list.as_array().unwrap().iter().map(|c| c["tag"].as_str()).collect();
    assert_eq!(tags, [Some("widgets"), None, None]);
    assert_eq!(
        b.ok(&["config"]),
        "wip           3\ntheme         dark\nlayout        auto\ngithub        off\ngithub-panel  shown\nagents-panel  shown\n"
    );
    let id = b.in_review("ops: rotate tokens", "bob");
    b.ok_as("carol", &["done", &id.to_string()]);
    assert_eq!(b.column(id), "done", "anyone may close on a board that never set done-by");
    let c = b.json(&["show", "1", "--json"]);
    assert_eq!(c["approved_by"], serde_json::json!([]), "additive and empty");
    assert_eq!(b.json(&["board", "--json"])["v"], 1);
}

/// The docs must say what `done-by` is — and what it is not.
#[test]
fn done_by_is_documented_as_an_honest_mistake_stop_not_security() {
    for doc in ["README.md", "docs/AGENTS.md", "docs/HUMANS.md"] {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(doc)).unwrap();
        assert!(text.contains("done-by"), "{doc} never mentions done-by");
        let honest = text.contains("not security") || text.contains("honest mistake") || text.contains("self-asserted");
        assert!(honest, "{doc} does not say plainly that done-by is not security");
    }
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")).unwrap();
    for phrase in ["self-asserted", "--force"] {
        assert!(readme.contains(phrase), "the README does not say `{phrase}`");
    }
}
