//! A10: `tb link ID VALUE --label LABEL` attaches evidence to a card (`tb link ID --rm N`
//! removes it). B3: `tb config done-needs-link LABEL` refuses `tb done` / `tb move ID done`
//! until the card carries a link with that label — the same honest-mistake shape `done-by`
//! already has, sitting right beside it in `Store::transition`'s guard order.
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Board {
    dir: tempfile::TempDir,
}

impl Board {
    fn new() -> Board {
        Board { dir: tempfile::tempdir().unwrap() }
    }
    fn db(&self) -> PathBuf {
        self.dir.path().join("board.db")
    }
    fn run(&self, who: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("HOME", self.dir.path().join("home"))
            .env("TB_DB", self.db())
            .env("TB_AS", who)
            .env("TB_NO_HERDR", "1")
            .env("TB_NOW", "1790000000")
            .env_remove("HERDR_AGENT_NAME")
            .env_remove("TB_BOARD")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
    fn ok(&self, who: &str, args: &[&str]) -> String {
        let o = self.run(who, args);
        assert!(o.status.success(), "{args:?} as {who} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    }
    fn refused(&self, who: &str, args: &[&str]) -> String {
        let o = self.run(who, args);
        assert_eq!(o.status.code(), Some(1), "{args:?} as {who} should be refused: {}", String::from_utf8_lossy(&o.stdout));
        String::from_utf8_lossy(&o.stderr).trim().to_string()
    }
    fn json(&self, who: &str, args: &[&str]) -> serde_json::Value {
        let o = self.run(who, args);
        serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{args:?}: not JSON ({e}): {}", String::from_utf8_lossy(&o.stdout)))
    }
    fn links(&self, who: &str, id: i64) -> Vec<serde_json::Value> {
        self.json(who, &["show", &id.to_string(), "--json"])["links"].as_array().unwrap().clone()
    }
    /// A card in REVIEW, held and finished by `worker`.
    fn in_review(&self, title: &str, worker: &str) -> i64 {
        self.ok("lead", &["add", title]);
        let id = self.json("lead", &["list", "--json"]).as_array().unwrap().len() as i64;
        self.ok(worker, &["take", &id.to_string()]);
        self.ok(worker, &["done", &id.to_string()]);
        id
    }
}

// ------------------------------------------------------------------ A10: tb link

#[test]
fn a_link_is_added_listed_and_removed_with_renumbering() {
    let b = Board::new();
    b.ok("lead", &["add", "widgets: fix it"]);
    let said = b.ok("lead", &["link", "1", "docs/brief.md", "--label", "brief"]);
    assert!(said.contains("#1 link 1 added (brief)"), "{said}");
    // an http(s) URL, and a git sha: tb never distinguishes or validates the three forms
    b.ok("lead", &["link", "1", "https://ci.example/run/9", "--label", "verdict"]);
    b.ok("bot-1", &["link", "1", "a1b2c3d", "--label", "commit"]);
    let links = b.links("lead", 1);
    assert_eq!(links.len(), 3);
    assert_eq!(
        (links[0]["idx"].as_i64(), links[0]["label"].as_str(), links[0]["value"].as_str(), links[0]["added_by"].as_str()),
        (Some(1), Some("brief"), Some("docs/brief.md"), Some("lead"))
    );
    assert_eq!(links[1]["label"], "verdict");
    assert_eq!((links[2]["label"].as_str(), links[2]["added_by"].as_str()), (Some("commit"), Some("bot-1")));
    assert!(links[0]["added_at"].as_i64().is_some());

    // it shows in the plain `tb show` text too
    let shown = b.ok("lead", &["show", "1"]);
    assert!(shown.contains("brief: docs/brief.md") && shown.contains("verdict: https://ci.example/run/9"), "{shown}");

    // remove the middle one; the survivor after it renumbers
    let said = b.ok("lead", &["link", "1", "--rm", "2"]);
    assert!(said.contains("#1 link 2 deleted, the rest renumbered"), "{said}");
    let links = b.links("lead", 1);
    assert_eq!(links.len(), 2);
    assert_eq!((links[0]["idx"].as_i64(), links[0]["label"].as_str()), (Some(1), Some("brief")));
    assert_eq!((links[1]["idx"].as_i64(), links[1]["label"].as_str()), (Some(2), Some("commit")), "renumbered from 3 to 2");

    // removing one that does not exist is refused, nothing changes
    let e = b.refused("lead", &["link", "1", "--rm", "9"]);
    assert!(e.contains("has no link 9 (it has 2)"), "{e}");

    // it is logged on the card's history, add and remove
    let events = b.json("lead", &["show", "1", "--json"])["events"].as_array().unwrap().clone();
    let link_events: Vec<(String, String)> =
        events.iter().filter(|e| e["kind"] == "link").map(|e| (e["actor"].as_str().unwrap().into(), e["text"].as_str().unwrap().into())).collect();
    assert!(link_events.contains(&("lead".to_string(), "+ brief: docs/brief.md".to_string())));
    assert!(link_events.iter().any(|(_, t)| t.starts_with("- verdict:")));
}

#[test]
fn a_card_may_carry_more_than_one_link_with_the_same_label() {
    let b = Board::new();
    b.ok("lead", &["add", "widgets: two commits"]);
    b.ok("lead", &["link", "1", "aaa111", "--label", "commit"]);
    b.ok("lead", &["link", "1", "bbb222", "--label", "Commit"]);
    let links = b.links("lead", 1);
    assert_eq!(links.len(), 2, "labels are free text — not a fixed, deduplicated set");
    assert_eq!(links[1]["label"], "commit", "stored lower-cased, whatever case was typed");
}

#[test]
fn bad_link_input_is_refused_with_the_command_to_run() {
    let b = Board::new();
    b.ok("lead", &["add", "widgets: fix it"]);
    let e = b.refused("lead", &["link", "1", "somewhere"]);
    assert!(e.contains("say what it is") && e.contains("--label brief"), "{e}");
    let e = b.refused("lead", &["link", "1"]);
    assert!(e.contains("give a value and --label, or --rm N"), "{e}");
    let e = b.refused("lead", &["link", "1", "", "--label", "brief"]);
    assert!(e.contains("the link is empty"), "{e}");
    let e = b.refused("lead", &["link", "1", "x", "--label", ""]);
    assert!(e.contains("the label is empty"), "{e}");
    let long_label = "x".repeat(33);
    let e = b.refused("lead", &["link", "1", "x", "--label", &long_label]);
    assert!(e.contains("the limit is 32"), "{e}");
    let e = b.refused("lead", &["link", "1", "a:b", "--label", "a:b"]);
    assert!(e.contains("letters, digits, spaces, hyphens and underscores"), "{e}");
    let e = b.refused("lead", &["link", "99", "x", "--label", "brief"]);
    assert!(e.contains("no card #99"), "{e}");
}

/// A link is untrusted, displayed text — never followed. The store keeps the value raw (like
/// a note), and the SAME display sanitiser every other stored text goes through removes
/// control bytes and escape sequences from it in plain output and in `--json`.
#[test]
fn a_links_control_bytes_never_reach_plain_or_json_output() {
    let b = Board::new();
    b.ok("lead", &["add", "widgets: fix it"]);
    // an escape sequence and a raw control byte inside an otherwise ordinary path
    let dirty = "docs/\x1b[31mbrief\x1b[0m\x07.md";
    b.ok("lead", &["link", "1", dirty, "--label", "brief"]);
    let shown = b.ok("lead", &["show", "1"]);
    assert!(shown.contains("docs/brief.md") && !shown.contains('\x1b') && !shown.contains('\x07'), "{shown:?}");
    let v = b.links("lead", 1);
    let value = v[0]["value"].as_str().unwrap();
    assert_eq!(value, "docs/brief.md", "the JSON view is cleaned the same way (text::sanitize_json)");
    // never read as a path — an absolute path that does not exist on this machine is
    // accepted exactly like any other string; tb never touches the filesystem for it
    b.ok("lead", &["link", "1", "/does/not/exist/anywhere.txt", "--label", "brief"]);
    assert_eq!(b.links("lead", 1).len(), 2);
}

// ------------------------------------------------------------------ B3: done-needs-link

#[test]
fn done_needs_link_refuses_done_until_a_matching_link_is_attached() {
    let b = Board::new();
    b.ok("lead", &["config", "wip", "9"]);
    assert_eq!(b.ok("lead", &["config", "done-needs-link"]).trim(), "off", "the default: nothing changes");
    let one = b.in_review("permits: renewal", "bob");
    // off by default: today's behaviour
    b.ok("carol", &["done", &one.to_string()]);
    assert_eq!(b.json("lead", &["show", &one.to_string(), "--json"])["column"], "done");

    assert_eq!(
        b.ok("lead", &["config", "done-needs-link", "verdict"]).trim(),
        "done-needs-link is now verdict — a card needs a link with that label ('tb link ID VALUE --label verdict') before it may reach done"
    );
    assert_eq!(b.json("lead", &["config", "done-needs-link", "--json"])["config"]["value"], "verdict");
    assert!(b.ok("lead", &["config"]).contains(&format!("{:<13} {}", "done-needs-link", "verdict")), "listed once set");

    let two = b.in_review("tax: return", "bob");
    let e = b.refused("carol", &["done", &two.to_string()]);
    assert!(
        e.contains(&format!("#{two} has no link labeled 'verdict'")) && e.contains(&format!("tb link {two}")) && e.contains("--force"),
        "{e}"
    );
    assert_eq!(b.json("lead", &["show", &two.to_string(), "--json"])["column"], "review", "nothing moved");
    // #81: the evidence gate carries the stable `done_needs_link` code
    let jo = b.run("carol", &["done", &two.to_string(), "--json"]);
    assert!(!jo.status.success());
    let jv: serde_json::Value = serde_json::from_slice(&jo.stdout).unwrap();
    assert_eq!(jv["code"], "done_needs_link");

    // every way into DONE is guarded, exactly like `done-by`
    assert!(b.refused("carol", &["move", &two.to_string(), "done"]).contains("has no link labeled 'verdict'"));

    // a link under a different label still refuses it
    b.ok("bob", &["link", &two.to_string(), "docs/brief.md", "--label", "brief"]);
    assert!(b.refused("carol", &["done", &two.to_string()]).contains("has no link labeled 'verdict'"));

    // the matching label, any case, lets it through
    b.ok("bob", &["link", &two.to_string(), "https://ci.example/run/1", "--label", "VERDICT"]);
    b.ok("carol", &["done", &two.to_string()]);
    assert_eq!(b.json("lead", &["show", &two.to_string(), "--json"])["column"], "done");

    // `--force` is open to everyone, and logged as its own event: the same honest-mistake bargain
    let three = b.in_review("lease: notice", "bob");
    b.ok("carol", &["done", &three.to_string(), "--force"]);
    assert_eq!(b.json("lead", &["show", &three.to_string(), "--json"])["column"], "done");
    let forced: Vec<String> = b.json("lead", &["show", &three.to_string(), "--json"])["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "force")
        .map(|e| e["text"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(forced, [format!("closed #{three} without a link labeled verdict")]);

    // clearing it lets anyone close again without a link
    assert_eq!(b.ok("lead", &["config", "done-needs-link", "--off"]).trim(), "done-needs-link is off — a card may reach done without a link");
    let four = b.in_review("lease: renew", "bob");
    b.ok("carol", &["done", &four.to_string()]);
    assert_eq!(b.json("lead", &["show", &four.to_string(), "--json"])["column"], "done");
    assert!(!b.ok("lead", &["config"]).contains("done-needs-link"), "cleared = no longer listed");
}

/// `done-needs-link` never contradicts the older rules: a name `done-by` refuses is stopped by
/// THAT message even with a matching link already attached, and a card's own author still
/// cannot approve their own work — both checked before the evidence gate.
#[test]
fn done_needs_link_never_overrides_done_by_or_self_approval() {
    let b = Board::new();
    b.ok("lead", &["config", "done-by", "anna"]);
    b.ok("lead", &["config", "done-needs-link", "verdict"]);
    let one = b.in_review("permits: renewal", "bob");
    b.ok("bob", &["link", &one.to_string(), "https://ci.example/run/1", "--label", "verdict"]);
    let e = b.refused("carol", &["done", &one.to_string()]);
    assert!(e.contains("only anna may close a card"), "done-by is checked first: {e}");

    // the author of the work still cannot approve it themselves, evidence or not
    b.ok("lead", &["config", "done-by", "--off"]);
    let two = b.in_review("tax: return", "bob");
    b.ok("bob", &["link", &two.to_string(), "https://ci.example/run/2", "--label", "verdict"]);
    let e = b.refused("bob", &["done", &two.to_string()]);
    assert!(e.contains("you did this work"), "self-approval is checked before the evidence gate: {e}");
}

// ------------------------------------------------------------------ Storage: mv and archive

/// `tb mv ID --to BOARD`: links travel with the card, like the checklist and the history.
#[test]
#[cfg(unix)]
fn links_travel_when_a_card_moves_to_another_board() {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("HOME", dir.path())
            .env("TB_AS", "alice")
            .env("TB_NO_HERDR", "1")
            .env_remove("TB_DB")
            .env_remove("TB_BOARD")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let ok = |args: &[&str]| -> String {
        let o = run(args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    let json = |args: &[&str]| -> serde_json::Value { serde_json::from_str(&ok(args)).unwrap() };

    ok(&["add", "docs: write the guide"]);
    ok(&["link", "1", "docs/brief.md", "--label", "brief"]);
    ok(&["link", "1", "a1b2c3d", "--label", "commit"]);
    ok(&["work", "add", "x: a card already here"]);

    let said = ok(&["mv", "1", "--to", "work"]);
    assert!(said.contains("2 link(s)"), "{said}");
    let after = json(&["work", "show", "2", "--json"]);
    let links = after["links"].as_array().unwrap();
    assert_eq!(links.len(), 2);
    assert_eq!((links[0]["label"].as_str(), links[0]["value"].as_str()), (Some("brief"), Some("docs/brief.md")));
    assert_eq!((links[1]["label"].as_str(), links[1]["added_by"].as_str()), (Some("commit"), Some("alice")));
}

/// Soft delete (`tb config rm archive`): a card's links go with it into the archive and come
/// straight back on `tb restore`.
#[test]
fn links_survive_archive_and_restore() {
    let b = Board::new();
    b.ok("lead", &["config", "rm", "archive"]);
    b.ok("lead", &["add", "case: file the brief"]);
    b.ok("lead", &["link", "1", "docs/brief.md", "--label", "brief"]);
    b.ok("lead", &["link", "1", "https://ci.example/run/9", "--label", "verdict"]);
    let before = b.links("lead", 1);

    b.ok("lead", &["rm", "1"]);
    assert!(!b.run("lead", &["show", "1"]).status.success(), "gone while archived");

    b.ok("lead", &["restore", "1"]);
    let after = b.links("lead", 1);
    assert_eq!(after, before, "links come back exactly as they were, index and all");
}
