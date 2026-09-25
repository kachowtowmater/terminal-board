#![cfg(unix)]
//! Who moves a card into DONE (store/verifier.rs): nothing reaches DONE except from REVIEW,
//! and REVIEW -> DONE only by a verifier — an agent with `TB_ROLE=verifier|reviewer`, a name on
//! `config verifiers`, or a person. Everything here drives the real `tb` binary with a
//! controlled environment (so whatever harness runs the suite never leaks in) and only
//! through the CLI, so a tree without the rule fails these by assertion, not by compile error.
mod common;

use std::path::PathBuf;
use std::process::{Command, Output};

const UUID: &str = "0b9f6a52-7c1d-4e0a-9f3b-2a6c1d8e4f70";
/// A second session: what a VERIFIER runs in. Same-session refusals (`same_session`) compare
/// the close's session with the WORK's, so a fixture where both share `UUID` now models the
/// self-approval #134 forbids — every verifier-side env gets `VUUID` instead.
const VUUID: &str = "1f4e9d7b-6c2a-4e5b-8d3f-a0b1c2d3e4f5";
/// An agent: a harness on record, no role.
const AGENT: &[(&str, &str)] = &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", UUID)];
/// A verifier: the same harness, with the role, in its own session.
const VERIFIER: &[(&str, &str)] =
    &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", VUUID), ("TB_ROLE", "verifier"), ("TB_MODEL", "model-x"), ("TB_HOST", "lab")];
/// A person in a plain terminal: nothing but the name.
const PERSON: &[(&str, &str)] = &[];

struct Board {
    dir: tempfile::TempDir,
    /// A lazily-created registry dir this board registers its role-claiming runners in.
    registry: std::sync::OnceLock<common::VerifierRegistry>,
}

impl Board {
    fn new() -> Board {
        let b = Board { dir: tempfile::tempdir().unwrap(), registry: std::sync::OnceLock::new() };
        b.ok(PERSON, "lead", &["config", "wip", "9"]);
        b
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("b.db")
    }

    fn run_raw(&self, env: &[(&str, String)], who: &str, args: &[&str]) -> Output {
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
        c.envs(env.iter().map(|(k, v)| (*k, v.as_str())));
        c.output().unwrap()
    }

    /// The entry point every helper runs through: role-claiming envs are registered first.
    fn run<'a>(&self, env: &'a [(&'a str, String)], who: &str, args: &[&str]) -> Output {
        let env = self.with_registration(env, who);
        self.run_raw(&env, who, args)
    }

    /// The registering runner under its original name at call sites that built envs as
    /// `Vec<(&str, String)>` before the wrapper existed.
    fn run_s<'a>(&self, env: &'a [(&'a str, String)], who: &str, args: &[&str]) -> Output {
        self.run(env, who, args)
    }

    fn ok(&self, env: &[(&str, &str)], who: &str, args: &[&str]) -> String {
        self.ok_s(&Self::str_env(env), who, args)
    }

    fn ok_raw(&self, env: &[(&str, String)], who: &str, args: &[&str]) -> String {
        let o = self.run_raw(env, who, args);
        assert!(o.status.success(), "{who}: tb {args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    fn ok_s(&self, env: &[(&str, String)], who: &str, args: &[&str]) -> String {
        let o = self.run(env, who, args);
        assert!(o.status.success(), "{who}: tb {args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    /// The `--json` refusal: (error text, code). Asserts it WAS refused.
    fn refused(&self, env: &[(&str, &str)], who: &str, args: &[&str]) -> (String, String) {
        self.refused_s(&Self::str_env(env), who, args)
    }

    fn refused_s(&self, env: &[(&str, String)], who: &str, args: &[&str]) -> (String, String) {
        let mut a = args.to_vec();
        a.push("--json");
        let o = self.run(env, who, &a);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
        assert!(!o.status.success(), "{who}: tb {args:?} was allowed: {v}");
        let text = format!("{} — {}", v["error"].as_str().unwrap_or(""), v["hint"].as_str().unwrap_or(""));
        (text, v["code"].as_str().unwrap_or("").to_string())
    }

    /// The registry's own r1-r6 tests manage registration by hand and assert exact refusals,
    /// so they run the command as-is — no automatic registration.
    fn refused_raw(&self, env: &[(&str, String)], who: &str, args: &[&str]) -> (String, String) {
        let mut a = args.to_vec();
        a.push("--json");
        let o = self.run_raw(env, who, &a);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
        assert!(!o.status.success(), "{who}: tb {args:?} was allowed: {v}");
        let text = format!("{} — {}", v["error"].as_str().unwrap_or(""), v["hint"].as_str().unwrap_or(""));
        (text, v["code"].as_str().unwrap_or("").to_string())
    }

    /// Lifts a compile-time env literal (`[(key, "value"); N]`) into the `(key, String)` pairs
    /// the `*_s` runners take.
    fn str_env(env: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
        env.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    /// A close whose env claims the verifier role also needs its session REGISTERED (card
    /// #169): the runners register any role-claiming env once per board, so the pre-registry
    /// role tests above keep testing the role rule alone. The registry's own r1–r6 tests pass
    /// envs built at runtime and call `VerifierRegistry::register` directly, so they are
    /// unaffected; a test that wants an UNREGISTERED role claim asserts
    /// `unregistered_verifier` and uses `run_raw`.
    fn with_registration<'a>(&self, env: &'a [(&'a str, String)], who: &str) -> Vec<(&'a str, String)> {
        let role = env.iter().any(|(k, v)| *k == "TB_ROLE" && v.eq_ignore_ascii_case("verifier") || *k == "TB_ROLE" && v.eq_ignore_ascii_case("reviewer"));
        if !role {
            return env.to_vec();
        }
        let reg = self.registry.get_or_init(common::VerifierRegistry::new);
        let session = env.iter().find(|(k, _)| *k == "CLAUDE_CODE_SESSION_ID" || *k == "CODEX_SESSION_ID").map(|(_, v)| v.clone());
        if let Some(session) = session {
            reg.register(&session, who, "claude-code");
        }
        let mut out: Vec<(&'a str, String)> = reg.env().to_vec();
        out.extend(env.iter().cloned());
        out
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
        assert!(e.contains("TB_ROLE=verifier") && e.contains("claude-code with no role"), "{e}");
        assert!(!e.contains("config verifiers"), "a refused agent is never shown how to list itself: {e}");
    }
    // a role that is not a verifier's is refused just the same
    let builder = Board::str_env(&[("CLAUDECODE", "1"), ("TB_ROLE", "builder")]);
    let (e, code) = b.refused_s(&builder, "orch", &["done", &id]);
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
    b.ok_s(&Board::str_env(&[("CLAUDECODE", "1"), ("TB_ROLE", "Reviewer")]), "rv-2", &["move", &two, "done"]);
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
        (Some("rv-1"), Some("claude-code"), Some("model-x"), Some("verifier"), Some(VUUID), Some("lab"))
    );
    // ... and in the plain `tb show` a person reads
    let text = b.ok(PERSON, "lead", &["show", &id]);
    assert!(text.contains(&format!("rv-1 — claude-code model-x verifier session {VUUID} on lab")), "{text}");
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
    let listed = Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", VUUID)]);
    b.ok_s(&listed, "RV-1", &["done", &id]);
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
    b.ok_s(&Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", VUUID)]), "orch", &["done", &id]);
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

#[test]
fn codex_is_an_agent_too() {
    // what `codex exec` hands its shell: none of AI_AGENT, CLAUDECODE or OMPCODE
    let codex: Vec<(&str, String)> = Board::str_env(&[("CODEX_SESSION_ID", UUID), ("CODEX_THREAD_ID", "thread-9"), ("CODEX_SANDBOX", "seatbelt"), ("CODEX_CI", "1")]);
    let b = Board::new();
    let id = b.in_review("m: codex closes it", "bot-1");
    let (e, code) = b.refused_s(&codex, "cx", &["done", &id]);
    assert_eq!(code, "not_verifier", "{e}");
    assert!(e.contains("cx is codex with no role"), "{e}");
    assert_eq!(b.column(&id), "review");
    // with the role it closes, and the trace says codex with its session — in a session of
    // its own, not the builder's (`same_session` would refuse that)
    let mut role = codex.to_vec();
    role[0].1 = VUUID.to_string();
    role.push(("TB_ROLE", "verifier".to_string()));
    b.ok_s(&role, "cx", &["done", &id]);
    let show = b.json(&["show", &id]);
    let who = show["actors"].as_array().unwrap().iter().find(|a| a["actor"] == "cx" && a["role"] == "verifier").cloned().unwrap();
    assert_eq!((who["harness"].as_str(), who["session"].as_str()), (Some("codex"), Some(VUUID)));
}

#[test]
fn only_a_person_changes_who_verifies_or_whether_the_rule_applies() {
    let b = Board::new();
    b.ok(PERSON, "lead", &["config", "verifiers", "rv-1"]);
    let id = b.in_review("n: an agent lists itself", "bot-1");
    let (_, code) = b.refused(AGENT, "w2", &["done", &id]);
    assert_eq!(code, "not_verifier");
    // the refused agent cannot put itself on the list, clear it, or switch the rule off —
    // not even an agent with a verifier role
    for (env, args) in [
        (AGENT, vec!["config", "verifiers", "rv-1,w2"]),
        (AGENT, vec!["config", "verifiers", "--off"]),
        (AGENT, vec!["config", "verifier-only", "off"]),
        (VERIFIER, vec!["config", "verifiers", "rv-9"]),
        (VERIFIER, vec!["config", "verifier-only", "off"]),
    ] {
        let (e, code) = b.refused(env, "w2", &args);
        assert_eq!(code, "person_only", "{args:?}: {e}");
        assert!(e.contains("only a person changes"), "{e}");
    }
    assert_eq!(b.json(&["config", "verifiers"])["config"]["value"], serde_json::json!(["rv-1"]), "the list is unchanged");
    assert_eq!(b.json(&["config", "verifier-only"])["config"]["value"], "on", "the rule is still on");
    let (_, code) = b.refused(AGENT, "w2", &["done", &id]);
    assert_eq!(code, "not_verifier", "and w2 still cannot close it");
    // reading them stays open to everyone
    assert_eq!(b.ok(AGENT, "w2", &["config", "verifier-only"]).trim(), "on");
    // a person still changes both
    b.ok(PERSON, "lead", &["config", "verifiers", "rv-1,rv-2"]);
    b.ok(PERSON, "lead", &["config", "verifier-only", "off"]);
}

#[test]
fn tb_log_carries_the_identity_of_whoever_moved_a_card_to_done() {
    let b = Board::new();
    let id = b.in_review("o: the trace", "bot-1");
    b.ok(VERIFIER, "rv-1", &["done", &id]);
    // plain text: the line that moved the card into DONE says who, in full
    let log = b.ok(PERSON, "lead", &["log"]);
    let line = log.lines().find(|l| l.contains("review -> done")).unwrap_or_else(|| panic!("{log}"));
    assert!(line.contains(&format!("(by rv-1 — claude-code model-x verifier session {VUUID} on lab)")), "{line}");
    // the other lines stay one short line each
    assert!(!log.lines().any(|l| l.contains("doing -> review") && l.contains("(by ")), "{log}");
    // --json: every row carries its identity inline
    let rows: serde_json::Value = serde_json::from_str(&b.ok(PERSON, "lead", &["log", "--json"])).unwrap();
    let moved = rows.as_array().unwrap().iter().find(|e| e["text"] == "review -> done").cloned().unwrap();
    let who = &moved["identity"];
    assert_eq!(
        (who["actor"].as_str(), who["harness"].as_str(), who["model"].as_str(), who["role"].as_str(), who["session"].as_str(), who["host"].as_str()),
        (Some("rv-1"), Some("claude-code"), Some("model-x"), Some("verifier"), Some(VUUID), Some("lab")),
        "{moved}"
    );
    assert_eq!(who["id"], moved["actor_id"]);
    // a person's row has no identity to show
    let created = rows.as_array().unwrap().iter().find(|e| e["kind"] == "created").cloned().unwrap();
    assert!(created["identity"].is_null(), "{created}");
}

/// A verifier that FAILS a card sends it back (review -> todo, which clears the owner). Once
/// the worker has fixed it and sent it to review again, the same verifier closes it.
#[test]
fn a_verifier_that_failed_a_card_closes_it_after_the_fix() {
    let b = Board::new();
    let id = b.in_review("k: fail then fix", "bot-1");
    b.ok(VERIFIER, "rv-1", &["move", &id, "todo"]);
    b.ok(AGENT, "bot-1", &["take", &id]);
    b.ok(AGENT, "bot-1", &["done", &id]);
    b.ok(VERIFIER, "rv-1", &["done", &id]);
    assert_eq!(b.column(&id), "done");
}

/// The same verifier sends a card back and later returns it to review itself (the failure was
/// ruled out of scope): moving it back is checking the work, not doing it, so the verifier
/// still closes it — and the worker who built it still cannot.
#[test]
fn a_verifier_that_sent_a_card_back_and_returned_it_itself_still_closes_it() {
    let b = Board::new();
    let id = b.in_review("l: sent back, returned", "bot-1");
    b.ok(VERIFIER, "rv-1", &["move", &id, "todo"]);
    b.ok(VERIFIER, "rv-1", &["move", &id, "review"]);
    // the builder, even claiming the role, is still the author
    let (e, code) = b.refused(VERIFIER, "bot-1", &["done", &id]);
    assert_eq!(code, "self_approve", "the builder closed its own card: {e}");
    assert_eq!(b.column(&id), "review");
    let o = b.run_s(&Board::str_env(VERIFIER), "rv-1", &["done", &id]);
    assert!(o.status.success(), "the verifier that sent it back was refused: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(b.column(&id), "done");
}

/// A name on `config verifiers` (no role) is a verifier for this too.
#[test]
fn a_listed_verifier_that_sent_a_card_back_and_returned_it_still_closes_it() {
    let b = Board::new();
    b.ok(PERSON, "lead", &["config", "verifiers", "rv-2"]);
    let id = b.in_review("m: listed verifier", "bot-1");
    b.ok(AGENT, "rv-2", &["move", &id, "todo"]);
    b.ok(AGENT, "rv-2", &["move", &id, "review"]);
    let listed = Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", VUUID)]);
    let o = b.run_s(&listed, "rv-2", &["done", &id]);
    assert!(o.status.success(), "the listed verifier that sent it back was refused: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(b.column(&id), "done");
}

/// A listed verifier (a name on `config verifiers`, no role) closes the card FROM THE SAME
/// SESSION it sent it back and returned it in: its own mover sessions are skipped whatever
/// made it a verifier — a role or the list — so only the builder's take session counts.
#[test]
fn a_listed_verifier_closes_it_from_the_same_session_it_moved_it_in() {
    let b = Board::new();
    b.ok(PERSON, "lead", &["config", "verifiers", "rv-l"]);
    let id = b.in_review("6a: listed mover, one session", "bot-1");
    let rv_l = &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "cccccccc-3333-4444-5555-666666666666")];
    b.ok(rv_l, "rv-l", &["move", &id, "todo"]);
    b.ok(rv_l, "rv-l", &["move", &id, "review"]);
    let o = b.run_s(&Board::str_env(rv_l), "rv-l", &["done", &id]);
    assert!(o.status.success(), "closing from the session it moved it in was refused: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(b.column(&id), "done");
}

/// A verifier that did the work is still refused, however the card got back to review: it
/// held the card, or it is the only one that ever moved it into review.
#[test]
fn a_verifier_that_built_the_card_is_still_refused_after_a_send_back() {
    let b = Board::new();
    let id = b.in_review("n: verifier built it", "rv-1");
    b.ok(VERIFIER, "rv-1", &["move", &id, "todo"]);
    b.ok(VERIFIER, "rv-1", &["move", &id, "review"]);
    let (e, code) = b.refused(VERIFIER, "rv-1", &["done", &id]);
    assert_eq!(code, "self_approve", "{e}");

    let two = b.add("o: verifier routed it alone");
    b.ok(VERIFIER, "rv-1", &["move", &two, "review"]);
    let (e, code) = b.refused(VERIFIER, "rv-1", &["done", &two]);
    assert_eq!(code, "self_approve", "{e}");
    assert_eq!(b.column(&two), "review");
}

/// A different name and a claimed verifier role do not change the session a harness records:
/// the session behind the close is compared with the sessions of every identity that took the
/// card or moved it into review, in any round — the hole ops #18/#19 fell through, where one
/// session graded its own work as `rv-w2fixture` and passed both name guards.
#[test]
fn a_verifier_in_the_builders_session_is_refused_whatever_its_name_or_role() {
    let b = Board::new();
    let id = b.in_review("p: same session, new name", "bot-1");
    // the verifier is in the builder's session, under another name, claiming the role
    let same = Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", UUID), ("TB_ROLE", "verifier"), ("TB_MODEL", "model-x"), ("TB_HOST", "lab")]);
    let (e, code) = b.refused_s(&same, "rv-other", &["done", &id]);
    assert_eq!(code, "same_session", "{e}");
    assert!(e.contains("shares your session (0b9f6a52"), "{e}");
    assert!(e.contains("bot-1"), "{e}");
    assert!(e.contains("its own session"), "{e}");
    assert_eq!(b.column(&id), "review", "nothing moved");
    // `tb move` meets the same rule
    let (_, code) = b.refused_s(&same, "rv-other", &["move", &id, "done"]);
    assert_eq!(code, "same_session");
    // the message never shows how to get past it another way
    assert!(!e.contains("config verifiers"), "{e}");
}

/// The same-session refusal follows the guard list, so `--force` closes and logs its own
/// force event naming the rule.
#[test]
fn force_gets_past_the_same_session_rule_and_is_logged() {
    let b = Board::new();
    let id = b.in_review("q: forced same session", "bot-1");
    let same = Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", UUID), ("TB_ROLE", "verifier"), ("TB_MODEL", "model-x"), ("TB_HOST", "lab")]);
    b.ok_s(&same, "rv-other", &["done", &id, "--force"]);
    assert_eq!(b.column(&id), "done");
    let forced: Vec<String> =
        b.events(&id).iter().filter(|e| e["kind"] == "force").filter_map(|e| e["text"].as_str().map(String::from)).collect();
    assert!(forced.iter().any(|t| t.contains("same_session")), "{forced:?}");
}

/// The session belongs to the WORK, not the name: a verifier whose session only ever wrote a
/// NOTE on the card closes it — notes are progress, not work — and so does one whose session
/// is only in an old `assigned` event's history.
#[test]
fn a_session_that_only_took_notes_or_routed_the_card_still_closes_it() {
    let b = Board::new();
    let id = b.add("r: a note is not work");
    b.ok(AGENT, "bot-1", &["note", &id, "looking at this"]);
    b.ok(VERIFIER, "bot-1", &["move", &id, "review"]);
    // the mover IS the verifier here, so let another verifier close it — its session matches
    // the note only
    b.ok(PERSON, "lead", &["config", "verifiers", "rv-note"]);
    let o = b.run(&Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", VUUID)]), "rv-note", &["done", &id]);
    assert!(o.status.success(), "a session that only noted was refused: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(b.column(&id), "done");

    // `assigned` is routing by the orchestrator, not a hold: its session does not count
    let two = b.add("s: assigned is routing");
    b.ok(PERSON, "lead", &["assign", &two, "bot-1"]);
    b.ok(AGENT, "bot-1", &["done", &two]);
    b.ok(PERSON, "lead", &["config", "verifiers", "rv-lead"]);
    let o = b.run(&Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", VUUID)]), "rv-lead", &["done", &two]);
    assert!(o.status.success(), "the assigning session's verifier was refused: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(b.column(&two), "done");
}

/// A verifier-role identity's own take/move sessions are skipped, exactly as `author_of`
/// skips its name: a verifier that failed the card (review -> todo) and moved it back into
/// review itself closes it, whatever session it did that from — unless the WORK's session
/// matches.
#[test]
fn a_verifier_movers_own_sessions_are_skipped_but_the_builders_never() {
    let b = Board::new();
    let id = b.in_review("t: verifier round-trips it", "bot-1");
    b.ok(VERIFIER, "rv-1", &["move", &id, "todo"]);
    b.ok(VERIFIER, "rv-1", &["move", &id, "review"]);
    // the builder's session, again under a fresh name — still refused
    let same = Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", UUID), ("TB_ROLE", "verifier"), ("TB_MODEL", "model-x"), ("TB_HOST", "lab")]);
    let (e, code) = b.refused_s(&same, "rv-other", &["done", &id]);
    assert_eq!(code, "same_session", "{e}");
    // the verifier that round-tripped it, in ITS session, closes it: its moves were skipped
    let o = b.run_s(&Board::str_env(VERIFIER), "rv-1", &["done", &id]);
    assert!(o.status.success(), "the verifier's own send-back round was refused: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(b.column(&id), "done");
}

/// Round 2 (rv-lead-tb #134): a builder that TOOK the card with `TB_ROLE=verifier` in its
/// session S is NOT skipped — the verifier-role skip belongs to movers, never to takers. A
/// taker's `taken` row is the work itself, whatever it claimed to be; the same session under
/// another verifier's name closes nothing.
#[test]
fn a_builder_that_took_with_a_verifier_role_is_never_skipped() {
    let b = Board::new();
    let id = b.add("6b: role-claimed taker still owns its session");
    b.ok(VERIFIER, "bot-1", &["take", &id]);
    b.ok(AGENT, "bot-1", &["done", &id]);
    assert_eq!(b.column(&id), "review");
    // a verifier in the TAKER's session, under a fresh name and the role: refused
    let (e, code) = b.refused(VERIFIER, "rv-other", &["done", &id]);
    assert_eq!(code, "same_session", "{e}");
    assert_eq!(b.column(&id), "review");
}

/// Two builders across two rounds: taking the card in session A, dropping it, a second
/// builder in session B moves it into review — a verifier in EITHER session is refused.
#[test]
fn a_session_from_any_round_is_refused() {
    let b = Board::new();
    let id = b.add("u: two rounds");
    b.ok_s(&Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "aaaaaaaa-1111-2222-3333-444444444444")]), "bot-1", &["take", &id]);
    b.ok_s(&Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "aaaaaaaa-1111-2222-3333-444444444444")]), "bot-1", &["drop", &id]);
    b.ok_s(&Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "bbbbbbbb-5555-6666-7777-888888888888")]), "bot-2", &["take", &id]);
    b.ok_s(&Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "bbbbbbbb-5555-6666-7777-888888888888")]), "bot-2", &["done", &id]);
    let (e, code) =
        b.refused_s(&Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "bbbbbbbb-5555-6666-7777-888888888888"), ("TB_ROLE", "verifier")]), "rv-b", &["done", &id]);
    assert_eq!(code, "same_session", "{e}");
    let (e, code) =
        b.refused_s(&Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "aaaaaaaa-1111-2222-3333-444444444444"), ("TB_ROLE", "verifier")]), "rv-a", &["done", &id]);
    assert_eq!(code, "same_session", "{e}");
    assert_eq!(b.column(&id), "review");
}

/// Sessions are what the harness RECORDS, and only a harness records one: a person in a
/// plain terminal has no session at all, so it can never match one — and a verifier whose
/// session matches nothing closes a card built by sessionless identities as before.
#[test]
fn no_session_on_either_side_never_matches() {
    let b = Board::new();
    // the builder ran with no session exported (nothing recorded): any verifier closes it
    let id = b.add("v: no session");
    b.ok(AGENT, "bot-1", &["take", &id]);
    b.ok(AGENT, "bot-1", &["done", &id]);
    b.ok(VERIFIER, "rv-1", &["done", &id]);
    assert_eq!(b.column(&id), "done");
    // the verifier has no session but the builder does: closes it too
    let two = b.in_review("w: verifier sessionless", "bot-1");
    let o = b.run(&Board::str_env(&[("CLAUDECODE", "1"), ("TB_ROLE", "verifier")]), "rv-none", &["done", &two]);
    assert!(o.status.success(), "a sessionless verifier was refused: {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(b.column(&two), "done");
    // a person closes it regardless: no session to match, and never a refusal
    let three = b.in_review("x: a person closes", "bot-1");
    b.ok(PERSON, "anna", &["done", &three]);
    assert_eq!(b.column(&three), "done");
}

/// The negative of `no_session_on_either_side_never_matches`, both halves: a BUILDER with no
/// session exported and a verifier WITH one is allowed — `same_session` never matches a
/// session the work never recorded.
#[test]
fn a_builder_with_no_session_and_a_verifier_with_one_is_allowed() {
    let b = Board::new();
    // the work ran under a harness with no session id exported, so nothing was recorded
    let id = b.add("6c: no-session builder");
    b.ok_s(&Board::str_env(&[("CLAUDECODE", "1")]), "bot-1", &["take", &id]);
    b.ok_s(&Board::str_env(&[("CLAUDECODE", "1")]), "bot-1", &["done", &id]);
    b.ok(VERIFIER, "rv-y", &["done", &id]);
    assert_eq!(b.column(&id), "done");
}

/// A session is compared trimmed and case-insensitively, the way `self_approve` compares
/// names: whitespace padding around a session id is not a different session.
#[test]
fn a_session_matches_after_trim_and_case() {
    let b = Board::new();
    let id = b.in_review("y: padded session", "bot-1");
    let (e, code) = b.refused_s(&Board::str_env(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "  0B9F6A52-7C1D-4E0A-9F3B-2A6C1D8E4F70  "), ("TB_ROLE", "verifier")]), "rv-pad", &["done", &id]);
    assert_eq!(code, "same_session", "{e}");
}

/// `verifier-only off` removes the role requirement, not the session one: the same session
/// is still refused, and only `--force` closes it.
#[test]
fn same_session_applies_with_verifier_only_off() {
    let b = Board::new();
    b.ok(PERSON, "lead", &["config", "verifier-only", "off"]);
    let id = b.in_review("z: rule off, session on", "bot-1");
    let (e, code) = b.refused(AGENT, "rv-other", &["done", &id]);
    assert_eq!(code, "same_session", "{e}");
    b.ok(AGENT, "rv-other", &["done", &id, "--force"]);
    assert_eq!(b.column(&id), "done");
}

// --- the verifier registry (card #169): TB_ROLE=verifier is a claim; the session must be one
// --- tb-agent-start launched as a verifier (a JSON entry {"session","name","harness"} per file).

/// Every verifier env gets the test registry wired in: `TB_VERIFIERS_DIR` (tests) pointing at
/// a fresh temp dir, so the suite never writes the machine's real
/// `~/.local/state/terminal-board/verifiers`. Existing tests above run WITHOUT it: no
/// registry anywhere, so a role-claiming close is refused as `unregistered_verifier` — the
/// assertions above that say `not_verifier` are the AGENT-without-role cases, which refuse
/// before the registry is consulted.
fn registered(verifier_registry: &common::VerifierRegistry, session: &str, name: &str) -> Vec<(&'static str, String)> {
    verifier_registry.register(session, name, "claude-code");
    Board::str_env(VERIFIER).into_iter().chain(verifier_registry.env()).collect()
}

#[test]
fn a_role_claim_without_a_registry_entry_is_refused() {
    let reg = common::VerifierRegistry::new();
    let b = Board::new();
    let id = b.in_review("r1: env-only verifier", "bot-1");
    let env: Vec<(&str, String)> = Board::str_env(VERIFIER).into_iter().chain(reg.env()).collect();
    let (e, code) = b.refused_raw(&env, "rv-x", &["done", &id]);
    assert_eq!(code, "unregistered_verifier", "{e}");
    assert!(e.contains("tb-agent-start did not start") && e.contains(&id), "{e}");
    assert!(e.contains("--force, logged"), "{e}");
    assert_eq!(b.column(&id), "review");
}

#[test]
fn a_registered_session_closes_with_its_own_name_and_harness() {
    let reg = common::VerifierRegistry::new();
    let b = Board::new();
    let id = b.in_review("r2: registered verifier", "bot-1");
    let env = registered(&reg, VUUID, "rv-1");
    b.ok_raw(&env, "rv-1", &["done", &id]);
    assert_eq!(b.column(&id), "done");
    // the trace still carries the whole identity
    let show = b.json(&["show", &id]);
    let moved = show["events"].as_array().unwrap().iter().find(|e| e["text"] == "review -> done").cloned().unwrap();
    let who = show["actors"].as_array().unwrap().iter().find(|a| a["id"] == moved["actor_id"]).cloned().unwrap();
    assert_eq!((who["role"].as_str(), who["session"].as_str()), (Some("verifier"), Some(VUUID)));
}

#[test]
fn a_registered_session_under_another_name_is_refused() {
    let reg = common::VerifierRegistry::new();
    let b = Board::new();
    let id = b.in_review("r3: session borrowed", "bot-1");
    let env = registered(&reg, VUUID, "rv-1");
    let (e, code) = b.refused_raw(&env, "rv-z", &["done", &id]);
    assert_eq!(code, "unregistered_verifier", "{e}");
    // a name the entry does not carry closes nothing, even with the role claimed
    assert_eq!(b.column(&id), "review");
}

#[test]
fn a_registered_session_with_another_harness_is_refused() {
    let reg = common::VerifierRegistry::new();
    let b = Board::new();
    let id = b.in_review("r4: harness swapped", "bot-1");
    reg.register(VUUID, "rv-1", "omp");
    // same role/session, but the identity says omp — the entry's harness is claude-code
    let with_harness: Vec<(&str, String)> =
        Board::str_env(VERIFIER).into_iter().chain(reg.env()).chain([("TB_HARNESS", "omp".to_string())]).collect();
    let (e, code) = b.refused_raw(&with_harness, "rv-1", &["done", &id]);
    assert_eq!(code, "unregistered_verifier", "{e}");
    assert_eq!(b.column(&id), "review");
}

#[test]
fn a_session_file_with_bad_or_missing_json_is_no_entry() {
    let reg = common::VerifierRegistry::new();
    let b = Board::new();
    let id = b.in_review("r5: corrupt entry", "bot-1");
    std::fs::create_dir_all(reg.dir.path()).unwrap();
    std::fs::write(reg.dir.path().join(VUUID), "{not json").unwrap();
    let full: Vec<(&str, String)> = Board::str_env(VERIFIER).into_iter().chain(reg.env()).collect();
    let (e, code) = b.refused_raw(&full, "rv-1", &["done", &id]);
    assert_eq!(code, "unregistered_verifier", "{e}");
}

#[test]
fn the_registry_refusal_names_the_fix_not_the_list() {
    let reg = common::VerifierRegistry::new();
    let b = Board::new();
    let id = b.in_review("r6: message check", "bot-1");
    let full: Vec<(&str, String)> = Board::str_env(VERIFIER).into_iter().chain(reg.env()).collect();
    let (e, code) = b.refused_raw(&full, "rv-1", &["done", &id]);
    assert_eq!(code, "unregistered_verifier", "{e}");
    assert!(e.contains("tb-agent-start") && e.contains("--role verifier"), "{e}");
    assert!(!e.contains("config verifiers"), "never teach the refused agent the list: {e}");
}
