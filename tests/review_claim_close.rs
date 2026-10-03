#![cfg(unix)]
//! The reviewer claim meets DONE (default#813): a live `reviewer` claim from
//! `tb next --review` is the CLAIMANT's to close — another verifier is refused with
//! `claimed_by_other` (`tb done` and `tb move ID done` alike), a STALE claim (its
//! claimant dead by the same liveness World `tb release` asks, `TB_REAP_FAKE_*`
//! fixtures) is freed and the close proceeds with the event log saying so, the
//! claimant itself closes as before, and `tb move ID review` still frees a claim for
//! anyone. Everything here drives the real `tb` binary through the CLI only, so a tree
//! without the rule fails these by assertion, not by compile error.
mod common;

use std::path::PathBuf;
use std::process::{Command, Output};

/// A second session per verifier: the claim and the close must never share one.
const AUUID: &str = "2a6c1d8e-4f70-4b9f-a52c-9f3b2a6c1d8e";
const BUUID: &str = "3b7d2e9f-5a81-4c0a-b63d-0a4c3b7d2e9f";
/// An agent: a harness on record, no role (the builder that sends work to review).
const AGENT: &[(&str, &str)] = &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "4c8e3f01-9b2a-4d57-8e6f-1a2b3c4d5e6f")];
/// A verifier in its OWN session, with the role.
fn verifier(session: &str) -> Vec<(&'static str, String)> {
    vec![
        ("CLAUDECODE", "1".into()),
        ("CLAUDE_CODE_SESSION_ID", session.into()),
        ("TB_ROLE", "verifier".into()),
        ("TB_MODEL", "model-x".into()),
    ]
}
/// A person in a plain terminal: nothing but the name.
fn person() -> Vec<(&'static str, String)> {
    Vec::new()
}

struct Board {
    dir: tempfile::TempDir,
    registry: std::sync::LazyLock<common::VerifierRegistry>,
}

impl Board {
    fn new() -> Board {
        let b = Board { dir: tempfile::tempdir().unwrap(), registry: std::sync::LazyLock::new(common::VerifierRegistry::new) };
        b.ok(&person(), "lead", &["config", "wip", "9"], false);
        b
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("b.db")
    }

    /// Liveness always in fixture mode (`TB_REAP_FAKE_*`): nothing here asks the real
    /// herdr, tmux or process table. `claimant_alive` decides whether the CLAIMANT's
    /// name (`rv-a`) is a live agent the probes vouch for.
    fn run(&self, env: &[(&'static str, String)], who: &str, args: &[&str], claimant_alive: bool) -> Output {
        let env = self.with_registration(env, who);
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
        for v in ["TB_REAP_FAKE_AGENTS", "TB_REAP_FAKE_TMUX", "TB_REAP_FAKE_PANES", "TB_REAP_FAKE_SESSIONS", "TB_REAP_FAKE_PROCS"] {
            c.env(v, "");
        }
        c.env("TB_REAP_FAKE_AGENTS", if claimant_alive { "rv-a" } else { "" });
        c.env("TB_REAP_PROTECT", if claimant_alive { "rv-a" } else { "" });
        c.envs(env.iter().map(|(k, v)| (*k, v.as_str())));
        c.output().unwrap()
    }

    fn ok(&self, env: &[(&'static str, String)], who: &str, args: &[&str], claimant_alive: bool) -> String {
        let o = self.run(env, who, args, claimant_alive);
        assert!(o.status.success(), "{who}: tb {args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap()
    }

    /// The `--json` refusal: (error text, code). Asserts it WAS refused.
    fn refused(&self, env: &[(&'static str, String)], who: &str, args: &[&str], claimant_alive: bool) -> (String, String) {
        let mut a: Vec<&str> = args.to_vec();
        a.push("--json");
        let o = self.run(env, who, &a, claimant_alive);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
        assert!(!o.status.success(), "{who}: tb {args:?} was allowed: {v}");
        (
            format!("{} — {}", v["error"].as_str().unwrap_or(""), v["hint"].as_str().unwrap_or("")),
            v["code"].as_str().unwrap_or("").to_string(),
        )
    }

    /// Every agent close needs its session REGISTERED (card #169).
    fn with_registration<'a>(&self, env: &'a [(&'a str, String)], who: &str) -> Vec<(&'a str, String)> {
        let is_agent = env.iter().any(|(k, _)| matches!(*k, "CLAUDECODE" | "OMPCODE" | "TB_SESSION" | "TB_HARNESS"));
        if !is_agent {
            return env.to_vec();
        }
        let session = env.iter().find(|(k, _)| *k == "CLAUDE_CODE_SESSION_ID").map(|(_, v)| v.clone());
        if let Some(session) = session {
            self.registry.register(&session, who, "claude-code");
        }
        let mut out: Vec<(&'a str, String)> = self.registry.env().to_vec();
        out.extend(env.iter().cloned());
        out
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let mut a = args.to_vec();
        a.push("--json");
        serde_json::from_str(&self.ok(&person(), "lead", &a, false)).unwrap()
    }

    fn add(&self, title: &str) -> String {
        let v = self.json(&["add", title]);
        v["card"]["id"].as_i64().unwrap().to_string()
    }

    /// A card `worker` (an agent) took and sent to REVIEW.
    fn in_review(&self, title: &str, worker: &str) -> String {
        let id = self.add(title);
        self.ok(AGENT.iter().map(|(k, v)| (*k, v.to_string())).collect::<Vec<_>>().as_slice(), worker, &["take", &id], false);
        self.ok(AGENT.iter().map(|(k, v)| (*k, v.to_string())).collect::<Vec<_>>().as_slice(), worker, &["done", &id], false);
        assert_eq!(self.column(&id), "review");
        id
    }

    fn column(&self, id: &str) -> String {
        let v = self.json(&["show", id]);
        v["column"].as_str().or(v["card"]["column"].as_str()).unwrap().to_string()
    }

    fn reviewer_of(&self, id: &str) -> Option<String> {
        let v = self.json(&["show", id]);
        v["reviewer"].as_str().or(v["card"]["reviewer"].as_str()).map(str::to_string)
    }

    fn events(&self, id: &str) -> Vec<serde_json::Value> {
        self.json(&["show", id])["events"].as_array().unwrap().clone()
    }
}

/// rv-a claims (ALIVE); rv-b tries to close it: refused, claim kept, card in REVIEW.
#[test]
fn another_verifier_cannot_close_a_live_claim() {
    let b = Board::new();
    let id = b.in_review("claimed: rv-b may not close rv-a's claim", "bot-1");
    b.ok(verifier(AUUID).as_slice(), "rv-a", &["next", "--review"], true);
    assert_eq!(b.reviewer_of(&id).as_deref(), Some("rv-a"));
    for args in [vec!["done", id.as_str()], vec!["move", id.as_str(), "done"]] {
        let (e, code) = b.refused(verifier(BUUID).as_slice(), "rv-b", &args, true);
        assert_eq!(code, "claimed_by_other", "{args:?}: {e}");
        assert!(e.contains("rv-a") && e.contains(&format!("tb move {id} review")), "{e}");
        assert_eq!(b.column(&id), "review", "nothing moved");
        assert_eq!(b.reviewer_of(&id).as_deref(), Some("rv-a"), "the claim is kept");
    }
    // the refusal is logged so a person sees WHY a claim sat there: on the board log
    // (`tb log`), in its own write (a refused move rolls back)
    let board_log = b.ok(&person(), "lead", &["log"], true);
    let low = board_log.to_lowercase();
    assert!(low.contains("refused") && low.contains("claim"), "{board_log}");
}

/// rv-a claims, then rv-a is DEAD (no fixture agent vouches for it): rv-b's close
/// proceeds, the stale claim is freed, and the event log says the claim was stale.
#[test]
fn a_stale_claim_is_freed_and_the_close_proceeds() {
    let b = Board::new();
    let id = b.in_review("stale: rv-a is gone", "bot-1");
    b.ok(verifier(AUUID).as_slice(), "rv-a", &["next", "--review"], true);
    assert_eq!(b.reviewer_of(&id).as_deref(), Some("rv-a"));
    b.ok(verifier(BUUID).as_slice(), "rv-b", &["done", &id], false);
    assert_eq!(b.column(&id), "done");
    let events = b.events(&id);
    let stale: Vec<&str> = events.iter().filter(|e| e["kind"] == "unclaimed").filter_map(|e| e["text"].as_str()).collect();
    assert!(stale.iter().any(|t| t.contains("stale") && t.contains("rv-a")), "{stale:?}");
    // the close itself still carries its own trace
    assert!(events.iter().any(|e| e["kind"] == "moved" && e["text"] == "review -> done" && e["actor"] == "rv-b"), "{events:?}");
}

/// The claimant itself closes its own claimed card, alive or not: unchanged behaviour.
#[test]
fn the_claimant_closes_its_own_claim() {
    let b = Board::new();
    let id = b.in_review("own claim: rv-a closes", "bot-1");
    b.ok(verifier(AUUID).as_slice(), "rv-a", &["next", "--review"], true);
    b.ok(verifier(AUUID).as_slice(), "rv-a", &["done", &id], true);
    assert_eq!(b.column(&id), "done");
}

/// `tb move ID review` frees the claim (a person's act), and any verifier then closes.
#[test]
fn a_freed_claim_closes_and_an_unclaimed_card_never_refuses() {
    let b = Board::new();
    let id = b.in_review("freed: a person releases the claim", "bot-1");
    b.ok(verifier(AUUID).as_slice(), "rv-a", &["next", "--review"], true);
    b.ok(&person(), "lead", &["move", &id, "review"], true);
    assert_eq!(b.reviewer_of(&id), None, "the claim is freed");
    // #236: an AGENT close needs a held claim — rv-b re-claims the freed card, then closes it
    b.ok(verifier(BUUID).as_slice(), "rv-b", &["claim", &id], true);
    b.ok(verifier(BUUID).as_slice(), "rv-b", &["done", &id], true);
    assert_eq!(b.column(&id), "done");
    // an UNCLAIMED review card still closes without a claim — when a PERSON asks (#236 keeps
    // that door open); an agent is refused with close_needs_claim (C5,
    // an_agent_cannot_close_an_unclaimed_card below)
    let plain = b.in_review("unclaimed: no claim at all", "bot-1");
    b.ok(&person(), "lead", &["done", &plain], true);
    assert_eq!(b.column(&plain), "done");
}

/// A person's close over a live claim keeps main's behaviour: the verifier rule scopes the
/// claim refusal to verifiers, so the close proceeds — a person is the one actor that can
/// free a claim (`tb move ID review`) and override a close.
#[test]
fn a_person_closes_over_a_live_claim() {
    let b = Board::new();
    let id = b.in_review("person: the claim is rv-a's, a person closes anyway", "bot-1");
    b.ok(verifier(AUUID).as_slice(), "rv-a", &["next", "--review"], true);
    assert_eq!(b.reviewer_of(&id).as_deref(), Some("rv-a"));
    b.ok(&person(), "lead", &["done", &id], true);
    assert_eq!(b.column(&id), "done");
}

/// C5 (#236): an AGENT closing an UNCLAIMED review card is refused `close_needs_claim` —
/// `tb claim` or `tb next --review` takes the claim first; the card stays in REVIEW and
/// keeps no reviewer. A person's close of the same card still passes (the test above pins
/// the person door via a live-claim board; this one pins the unclaimed card directly).
#[test]
fn an_agent_cannot_close_an_unclaimed_card() {
    let b = Board::new();
    let id = b.in_review("unclaimed: an agent must claim before closing", "bot-1");
    assert_eq!(b.reviewer_of(&id), None, "no claim at all");
    // the closer must be a VERIFIER (the verifier rule fires first); it holds no claim
    for args in [vec!["done", id.as_str()], vec!["move", id.as_str(), "done"]] {
        let (e, code) = b.refused(verifier(BUUID).as_slice(), "rv-b", &args, false);
        assert_eq!(code, "close_needs_claim", "{args:?}: {e}");
        assert!(e.contains("tb claim") || e.contains("tb next --review"), "{e}");
        assert_eq!(b.column(&id), "review", "nothing moved");
        assert_eq!(b.reviewer_of(&id), None, "no claim was taken");
    }
}

/// #236: `tb claim ID` takes the claim only when it is free or STALE. A claim held by a
/// LIVE verifier is refused `claimed_by_other` — the message names the claimant and the
/// person's free path. (A stale claim frees and is taken; that is
/// a_freed_claim_closes…'s lane via `tb move`, and the close-side stale test pins the
/// same World. Here the claimant is alive: the second claimant is refused.)
#[test]
fn a_second_verifier_cannot_claim_a_live_claim() {
    let b = Board::new();
    let id = b.in_review("claim: rv-b may not take rv-a's live claim", "bot-1");
    b.ok(verifier(AUUID).as_slice(), "rv-a", &["claim", &id], true);
    assert_eq!(b.reviewer_of(&id).as_deref(), Some("rv-a"));
    let (e, code) = b.refused(verifier(BUUID).as_slice(), "rv-b", &["claim", &id], true);
    assert_eq!(code, "claimed_by_other", "{e}");
    assert!(e.contains("rv-a") && e.contains(&format!("tb move {id} review")), "{e}");
    assert_eq!(b.reviewer_of(&id).as_deref(), Some("rv-a"), "the claim is kept");
    // rv-b DEAD-looking (no fixture vouches for rv-a either): liveness answers for the
    // HOLDER — with the holder dead the claim is STALE, so the claim frees and is TAKEN
    // (the hand-off `tb claim` exists for), with an `unclaimed` event naming rv-a
    b.ok(verifier(BUUID).as_slice(), "rv-b", &["claim", &id], false);
    assert_eq!(b.reviewer_of(&id).as_deref(), Some("rv-b"), "the stale claim passed to rv-b");
    let unclaimed: Vec<&str> = b.events(&id).iter().filter(|e| e["kind"] == "unclaimed").filter_map(|e| e["text"].as_str()).collect();
    assert!(unclaimed.iter().any(|t| t.contains("stale") && t.contains("rv-a")), "{unclaimed:?}");
    // re-claiming your own claim is a no-op success
    b.ok(verifier(BUUID).as_slice(), "rv-b", &["claim", &id], true);
    assert_eq!(b.reviewer_of(&id).as_deref(), Some("rv-b"));
}

/// #236: the claim lock covers every move OUT of review, not just the close. Another
/// verifier's SEND-BACK (review -> doing, and review -> todo) of a live-claimed card is
/// refused `claimed_by_other`, the card stays in REVIEW with its claim, and the refusal
/// names the claimant's own send-back path.
#[test]
fn another_verifier_cannot_send_back_a_live_claim() {
    let b = Board::new();
    let id = b.in_review("send-back: rv-b may not fail rv-a's claim", "bot-1");
    b.ok(verifier(AUUID).as_slice(), "rv-a", &["claim", &id], true);
    assert_eq!(b.reviewer_of(&id).as_deref(), Some("rv-a"));
    // review -> doing and review -> todo are BOTH the claimant's acts; one send-back rule
    // (store.rs, `target_column != "done" && c.column == "review"`) covers the two, so both
    // refusals carry the send-back form ("{r} sends it back")
    let (e, code) = b.refused(verifier(BUUID).as_slice(), "rv-b", &["move", id.as_str(), "doing", "tests are missing"], true);
    assert_eq!(code, "claimed_by_other", "{e}");
    assert!(e.contains("sends it back") && e.contains("rv-a"), "{e}");
    let (e, code) = b.refused(verifier(BUUID).as_slice(), "rv-b", &["move", id.as_str(), "todo", "the fix is wrong"], true);
    assert_eq!(code, "claimed_by_other", "{e}");
    // the same send-back refusal: `claimed_by_other` for ANY move out of review the claimant
    // did not make, so the message stays the send-back form ("{r} sends it back") — the FAIL
    // rides the same rule, and the claimant's own send-back paths are both named on the claim
    assert!(e.contains("sends it back") && e.contains("rv-a"), "{e}");
    assert_eq!(b.column(&id), "review", "nothing moved");
    assert_eq!(b.reviewer_of(&id).as_deref(), Some("rv-a"), "the claim is kept");
}

/// M4: the claimant's own close never logs "stale claim by <self>". In a fixture where
/// liveness would call the claimant DEAD (TB_REAP_FAKE_* all empty), the claimant's
/// `tb done` still proceeds and the card's events carry NO `unclaimed` event — the
/// lazy World (#237) never asks liveness for the claimant's own moves, so there is no
/// stale-claim free to log.
#[test]
fn the_claimants_own_close_logs_no_stale_claim() {
    let b = Board::new();
    let id = b.in_review("own close: no stale claim even with the claimant looking dead", "bot-1");
    b.ok(verifier(AUUID).as_slice(), "rv-a", &["claim", &id], true);
    assert_eq!(b.reviewer_of(&id).as_deref(), Some("rv-a"));
    // claimant_alive=false: every TB_REAP_FAKE_* var set to empty — nothing vouches for rv-a
    b.ok(verifier(AUUID).as_slice(), "rv-a", &["done", &id], false);
    assert_eq!(b.column(&id), "done");
    let events = b.events(&id);
    let unclaimed: Vec<&str> = events.iter().filter(|e| e["kind"] == "unclaimed").filter_map(|e| e["text"].as_str()).collect();
    assert!(unclaimed.is_empty(), "the claimant's own close logged a stale claim: {unclaimed:?}");
}
