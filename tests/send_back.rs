//! v2: sending a REVIEW card back — store, full-screen board and GitHub sync rules.
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use terminal_board::github::{parse_prs, plan_moves, GhSnapshot, Pr};
use terminal_board::herdr::AgentsState;
use terminal_board::store::Store;
use terminal_board::tui::{App, Mode};

fn in_review(s: &mut Store, title: &str) -> i64 {
    let id = s.add(title, "", &[], "lead").unwrap();
    s.take(id, "bot-1").unwrap();
    assert_eq!(s.done(id, "bot-1").unwrap().column, "review");
    id
}

#[test]
fn store_send_back_rules() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = in_review(&mut s, "fix the thing");
    // review -> doing without a reason is refused on every store path
    let e = s.move_to(id, "doing", "rev").unwrap_err().to_string();
    assert!(e.starts_with("say why it goes back"), "{e}");
    assert!(s.send_back(id, " ", "rev").is_err());
    assert_eq!(s.card(id).unwrap().column, "review");
    assert_eq!(s.snapshot().unwrap().rounds.get(&id), None);
    // with a reason: same owner, `moved` then `returned`, round 2
    let c = s.send_back(id, "  tests fail  ", "rev").unwrap();
    assert_eq!((c.column.as_str(), c.owner.as_deref()), ("doing", Some("bot-1")));
    let d = s.show(id).unwrap();
    let tail: Vec<(&str, &str, &str)> =
        d.events.iter().rev().take(2).rev().map(|e| (e.kind.as_str(), e.actor.as_str(), e.text.as_str())).collect();
    assert_eq!(tail, [("moved", "rev", "review -> doing"), ("returned", "rev", "tests fail")]);
    assert_eq!(d.round, 2);
    assert_eq!(s.snapshot().unwrap().rounds.get(&id), Some(&2));
    // the owner sends it to review again; the author is still the worker, not the reviewer
    s.done(id, "bot-1").unwrap();
    assert_eq!(s.author(id).unwrap().as_deref(), Some("bot-1"));
    let t = s.returned_at().unwrap();
    assert!(t.contains_key(&id) && t.len() == 1);
    // a reason with any other move is refused
    let other = s.add("other", "", &[], "lead").unwrap();
    assert!(s.move_opts(other, "doing", "rev", false, Some("why")).is_err());
    assert!(s.move_opts(id, "done", "rev", false, Some("why")).is_err());
    assert_eq!(s.card(id).unwrap().column, "review");
}

fn pr(updated: &str) -> Pr {
    Pr {
        number: 30,
        title: "fix".into(),
        head_ref: "x".into(),
        is_draft: false,
        review: "-".into(),
        ci: "ok".into(),
        created_at: "2026-09-18T08:00:00Z".into(),
        author: "bot".into(),
        closes: vec![10],
        updated_at: updated.into(),
    }
}

#[test]
fn sync_plan_respects_a_return_until_the_pr_is_updated() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = in_review(&mut s, "gh#10 linked");
    s.send_back(id, "tests fail", "rev").unwrap();
    let back = s.returned_at().unwrap()[&id];
    let plan = |updated: &str, s: &Store| {
        let snap = GhSnapshot { repo: "o/r".into(), prs: vec![pr(updated)], ..Default::default() };
        plan_moves(&snap, &s.list().unwrap(), &HashMap::new(), &s.returned_at().unwrap())
    };
    let at = |t: i64| chrono::DateTime::from_timestamp(t, 0).unwrap().to_rfc3339();
    // updated before, or at the same second as, the return: no move
    assert!(plan(&at(back - 60), &s).is_empty());
    assert!(plan(&at(back), &s).is_empty());
    // no update time known (an older cache): no move
    assert!(plan("", &s).is_empty());
    // updated after the return: forward to review again
    let m = plan(&at(back + 1), &s);
    assert_eq!((m.len(), m[0].card_id, m[0].to.as_str()), (1, id, "review"));
    // a card that was never returned moves as before, whatever the update time
    let fresh = s.add("gh#10 never returned", "", &[], "lead").unwrap();
    s.take(fresh, "bot-2").unwrap();
    let m = plan("", &s);
    assert_eq!(m.iter().map(|m| m.card_id).collect::<Vec<_>>(), [fresh]);
}

#[test]
fn pr_list_reads_updated_at() {
    let prs = parse_prs(r#"[{"number":1,"title":"t","headRefName":"b","isDraft":false,"reviewDecision":"","statusCheckRollup":[],"createdAt":"2026-09-18T08:00:00Z","updatedAt":"2026-09-18T09:30:00Z","author":{"login":"a"}}]"#).unwrap();
    assert_eq!(prs[0].updated_at, "2026-09-18T09:30:00Z");
    // older caches without the field still load
    let old: GhSnapshot = serde_json::from_str(r#"{"repo":"o/r","fetched_at":1,"issues_open":0,"prs":[{"number":1,"title":"t","head_ref":"b","is_draft":false,"review":"-","ci":"-","created_at":"x","author":"a"}],"issues":[],"merged_today":[],"main_ci":null}"#).unwrap();
    assert_eq!(old.prs[0].updated_at, "");
}

#[test]
fn tui_asks_why_before_sending_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
    let id = in_review(&mut s, "fix the thing");
    let mut app = App::new(s.snapshot().unwrap(), "rev");
    app.agents = AgentsState::Unavailable("x".into());
    app.reload(&s);
    app.focus_card(id);
    // shift+left on a REVIEW card asks for the reason; esc leaves it in review
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT), &mut s);
    assert_eq!(app.mode, Mode::SendBack { id, buf: String::new() });
    app.handle_key(key(KeyCode::Esc), &mut s);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(s.card(id).unwrap().column, "review");
    // `<` asks too; enter on an empty reason does nothing
    app.focus_card(id);
    app.handle_key(key(KeyCode::Char('<')), &mut s);
    assert!(matches!(app.mode, Mode::SendBack { .. }), "{:?}", app.mode);
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert!(matches!(app.mode, Mode::SendBack { .. }));
    for c in "no test".chars() {
        app.handle_key(key(KeyCode::Char(c)), &mut s);
    }
    app.handle_key(key(KeyCode::Enter), &mut s);
    assert_eq!(app.mode, Mode::Normal);
    let c = s.card(id).unwrap();
    assert_eq!((c.column.as_str(), c.owner.as_deref()), ("doing", Some("bot-1")));
    let last = s.show(id).unwrap().events.pop().unwrap();
    assert_eq!((last.kind.as_str(), last.text.as_str()), ("returned", "no test"));
    assert_eq!(app.status.as_ref().unwrap().0, format!("#{id} sent back to bot-1"));
}
