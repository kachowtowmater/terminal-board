#![cfg(unix)]
//! Who moves a card into DONE (store/verifier.rs): nothing reaches DONE except from REVIEW,
//! and REVIEW -> DONE only by a verifier — an agent with `TB_ROLE=verifier|reviewer`, a name on
//! `config verifiers`, or a person. Everything here drives the real `tb` binary with a
//! controlled environment (so whatever harness runs the suite never leaks in) and only
//! through the CLI, so a tree without the rule fails these by assertion, not by compile error.
use std::path::PathBuf;
use std::process::{Command, Output};

const UUID: &str = "0b9f6a52-7c1d-4e0a-9f3b-2a6c1d8e4f70";
/// An agent: a harness on record, no role.
const AGENT: &[(&str, &str)] = &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", UUID)];
/// A verifier: the same harness, with the role.
const VERIFIER: &[(&str, &str)] =
    &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", UUID), ("TB_ROLE", "verifier"), ("TB_MODEL", "model-x"), ("TB_HOST", "lab")];
/// A person in a plain terminal: nothing but the name.
const PERSON: &[(&str, &str)] = &[];

struct Board {
    dir: tempfile::TempDir,
}

impl Board {
    fn new() -> Board {
        let b = Board { dir: tempfile::tempdir().unwrap() };
        b.ok(PERSON, "lead", &["config", "wip", "9"]);
        b
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("b.db")
    }

    fn run(&self, env: &[(&str, &str)], who: &str, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tb"));
        c.args(args).env_clear();
        c.env("TB_DB", self.db())
            .env("TB_AS", who)
            .env("TB_NO_HERDR", "1")
            .env("TB_GH", "/nonexistent/gh")
            .env("USER", "login-user")
            .env("TZ", "UTC")
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.dir.path());
        c.envs(env.iter().copied());
        c.output().unwrap()
    }

    fn ok(&self, env: &[(&str, &str)], who: &str, args: &[&str]) -> String {
        let o = self.run(env, who, args);
        assert!(o.status.success(), "{who}: tb {args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    /// The `--json` refusal: (error text, code). Asserts it WAS refused.
    fn refused(&self, env: &[(&str, &str)], who: &str, args: &[&str]) -> (String, String) {
        let mut a = args.to_vec();
        a.push("--json");
        let o = self.run(env, who, &a);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
        assert!(!o.status.success(), "{who}: tb {args:?} was allowed: {v}");
        // `--json` splits the message at its first ` — `: `error` is what happened, `hint` what to do
        let text = format!("{} — {}", v["error"].as_str().unwrap_or(""), v["hint"].as_str().unwrap_or(""));
        (text, v["code"].as_str().unwrap_or("").to_string())
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let mut a = args.to_vec();
        a.push("--json");
        serde_json::from_str(&self.ok(PERSON, "lead", &a)).unwrap()
    }

    fn add(&self, title: &str) -> String {
        let v: serde_json::Value = serde_json::from_str(&self.ok(PERSON, "lead", &["add", title, "--json"])).unwrap();
        v["card"]["id"].as_i64().unwrap().to_string()
    }

    /// A card `worker` (an agent) took and sent to REVIEW.
    fn in_review(&self, title: &str, worker: &str) -> String {
        let id = self.add(title);
        self.ok(AGENT, worker, &["take", &id]);
        self.ok(AGENT, worker, &["done", &id]);
        assert_eq!(self.column(&id), "review");
        id
    }

    fn column(&self, id: &str) -> String {
        let v = self.json(&["show", id]);
        v["column"].as_str().or(v["card"]["column"].as_str()).unwrap().to_string()
    }

    fn events(&self, id: &str) -> Vec<serde_json::Value> {
        self.json(&["show", id])["events"].as_array().unwrap().clone()
    }
}

#[test]
fn todo_and_doing_never_reach_done_except_from_review() {
    let b = Board::new();
    // a person, on a TODO card: every door into DONE refuses with the same code
    let todo = b.add("a: straight to done");
    for args in [vec!["done", todo.as_str()], vec!["move", todo.as_str(), "done"]] {
        let (e, code) = b.refused(PERSON, "lead", &args);
        assert_eq!(code, "not_from_review", "{args:?}: {e}");
        assert!(e.contains("nothing reaches done except from review") && e.contains(&format!("'tb take {todo}'")), "{e}");
    }
    assert_eq!(b.column(&todo), "todo");
    // its own holder, on a DOING card: `move ID done` is refused, and says what to run
    let doing = b.add("b: skip the review");
    b.ok(AGENT, "bot-1", &["take", &doing]);
    let (e, code) = b.refused(AGENT, "bot-1", &["move", &doing, "done"]);
    assert_eq!(code, "not_from_review", "{e}");
    assert!(e.contains(&format!("'tb done {doing}' moves it to review")), "{e}");
    assert_eq!(b.column(&doing), "doing");
    // a verifier does not get round it either: the rule is about the column, not the person
    let (_, code) = b.refused(VERIFIER, "rv-1", &["move", &todo, "done"]);
    assert_eq!(code, "not_from_review");
    // `--force` is the one way past it, and it is logged as its own `force` event
    b.ok(PERSON, "lead", &["move", &todo, "done", "--force"]);
    assert_eq!(b.column(&todo), "done");
    let events = b.events(&todo);
    let forced: Vec<&str> = events.iter().filter(|e| e["kind"] == "force").filter_map(|e| e["text"].as_str()).collect();
    assert!(forced.iter().any(|t| *t == format!("closed #{todo} from todo, skipping review")), "{forced:?}");
}

#[test]
fn an_agent_without_a_verifier_role_is_refused_review_to_done() {
    let b = Board::new();
    let id = b.in_review("c: the orchestrator closes it", "bot-1");
    for args in [vec!["done", id.as_str()], vec!["move", id.as_str(), "done"]] {
        let (e, code) = b.refused(AGENT, "orch", &args);
        assert_eq!(code, "not_verifier", "{args:?}: {e}");
        assert!(e.contains("TB_ROLE=verifier") && e.contains("tb config verifiers") && e.contains("claude-code with no role"), "{e}");
    }
    // a role that is not a verifier's is refused just the same
    let builder = [("CLAUDECODE", "1"), ("TB_ROLE", "builder")];
    let (e, code) = b.refused(&builder, "orch", &["done", &id]);
    assert_eq!(code, "not_verifier", "{e}");
    assert!(e.contains("role builder"), "{e}");
    assert_eq!(b.column(&id), "review", "nothing moved");
    // `--approve` still records a check by anyone who did not do the work, and still does
    // not move the card: it is not a way into DONE
    b.ok(AGENT, "orch", &["done", &id, "--approve"]);
    assert_eq!(b.column(&id), "review");
    // `--force` gets past it, logged
    b.ok(AGENT, "orch", &["done", &id, "--force"]);
    assert_eq!(b.column(&id), "done");
    assert!(b.events(&id).iter().any(|e| e["kind"] == "force" && e["text"] == format!("closed #{id} with no verifier role")), "{:?}", b.events(&id));
}

#[test]
fn a_verifier_role_closes_and_the_done_event_carries_its_whole_identity() {
    let b = Board::new();
    let id = b.in_review("d: verified work", "bot-1");
    b.ok(VERIFIER, "rv-1", &["done", &id]);
    assert_eq!(b.column(&id), "done");
    // `reviewer` is a verifier's role too, in any case
    let two = b.in_review("e: reviewed work", "bot-1");
    b.ok(&[("CLAUDECODE", "1"), ("TB_ROLE", "Reviewer")], "rv-2", &["move", &two, "done"]);
    assert_eq!(b.column(&two), "done");
    // the trace: the move into DONE names who made it, and the identity behind the name —
    // harness, model, role, session, host — in `tb show --json`
    let show = b.json(&["show", &id]);
    let moved = show["events"].as_array().unwrap().iter().find(|e| e["kind"] == "moved" && e["text"] == "review -> done").cloned().unwrap();
    assert_eq!(moved["actor"], "rv-1");
    let aid = moved["actor_id"].as_i64().expect("the move into done records an identity");
    let who = show["actors"].as_array().unwrap().iter().find(|a| a["id"] == aid).cloned().unwrap();
    assert_eq!(
        (who["actor"].as_str(), who["harness"].as_str(), who["model"].as_str(), who["role"].as_str(), who["session"].as_str(), who["host"].as_str()),
        (Some("rv-1"), Some("claude-code"), Some("model-x"), Some("verifier"), Some(UUID), Some("lab"))
    );
    // ... and in the plain `tb show` a person reads
    let text = b.ok(PERSON, "lead", &["show", &id]);
    assert!(text.contains(&format!("rv-1 — claude-code model-x verifier session {UUID} on lab")), "{text}");
    // ... and in `tb log --json`
    let log: serde_json::Value = serde_json::from_str(&b.ok(PERSON, "lead", &["log", "--json"])).unwrap();
    assert!(log.as_array().unwrap().iter().any(|e| e["kind"] == "moved" && e["text"] == "review -> done" && e["actor_id"] == aid), "{log}");
}

#[test]
fn a_verifier_still_never_closes_its_own_work() {
    let b = Board::new();
    let id = b.in_review("f: my own", "rv-1");
    let (e, code) = b.refused(VERIFIER, "rv-1", &["done", &id]);
    assert_eq!(code, "self_approve", "{e}");
    assert_eq!(b.column(&id), "review");
}

#[test]
fn a_name_on_the_board_verifier_list_closes_without_a_role() {
    let b = Board::new();
    assert_eq!(b.ok(PERSON, "lead", &["config", "verifiers"]).trim(), "none — only an agent with TB_ROLE=verifier, or a person, may close a card");
    b.ok(PERSON, "lead", &["config", "verifiers", "rv-1, rv-3"]);
    assert_eq!(b.json(&["config", "verifiers"])["config"]["value"], serde_json::json!(["rv-1", "rv-3"]));
    assert!(b.ok(PERSON, "lead", &["config"]).contains("verifiers"), "listed once set");
    let id = b.in_review("g: listed verifier", "bot-1");
    let (_, code) = b.refused(AGENT, "rv-2", &["done", &id]);
    assert_eq!(code, "not_verifier", "not on the list, no role");
    b.ok(AGENT, "RV-1", &["done", &id]);
    assert_eq!(b.column(&id), "done", "on the list, whatever case it is typed in");
    // the change is on the board's own log
    let log = b.ok(PERSON, "lead", &["log"]);
    assert!(log.contains("verifiers none -> rv-1, rv-3"), "{log}");
    // and clearing it takes the name's pass away again
    b.ok(PERSON, "lead", &["config", "verifiers", "--off"]);
    let two = b.in_review("h: list cleared", "bot-1");
    let (_, code) = b.refused(AGENT, "rv-1", &["done", &two]);
    assert_eq!(code, "not_verifier");
}

#[test]
fn a_person_closes_a_review_card() {
    let b = Board::new();
    let id = b.in_review("i: a person checks it", "bot-1");
    b.ok(PERSON, "anna", &["done", &id]);
    assert_eq!(b.column(&id), "done");
    assert!(!b.events(&id).iter().any(|e| e["kind"] == "force"), "a person needs no --force");
}

#[test]
fn verifier_only_off_lets_any_reviewer_close_but_never_from_outside_review() {
    let b = Board::new();
    assert_eq!(b.ok(PERSON, "lead", &["config", "verifier-only"]).trim(), "on", "on by default");
    assert!(!b.ok(PERSON, "lead", &["config"]).contains("verifier-only"), "the default is not listed");
    let out = b.ok(PERSON, "lead", &["config", "verifier-only", "off"]);
    assert!(out.starts_with("verifier-only is now off"), "{out}");
    assert_eq!(b.json(&["config", "verifier-only"])["config"]["value"], "off");
    assert!(b.ok(PERSON, "lead", &["log"]).contains("verifier-only on -> off"), "the change is logged on the board");
    let id = b.in_review("j: rule off", "bot-1");
    b.ok(AGENT, "orch", &["done", &id]);
    assert_eq!(b.column(&id), "done");
    // review-first is not part of the switch
    let todo = b.add("k: still review first");
    let (_, code) = b.refused(AGENT, "orch", &["move", &todo, "done"]);
    assert_eq!(code, "not_from_review");
    // back on, and refused again
    b.ok(PERSON, "lead", &["config", "verifier-only", "on"]);
    assert!(b.ok(PERSON, "lead", &["log"]).contains("verifier-only off -> on"));
    let two = b.in_review("l: rule on", "bot-1");
    let (_, code) = b.refused(AGENT, "orch", &["done", &two]);
    assert_eq!(code, "not_verifier");
    let (e, code) = b.refused(PERSON, "lead", &["config", "verifier-only", "maybe"]);
    assert_eq!(code, "invalid_value", "{e}");
}
