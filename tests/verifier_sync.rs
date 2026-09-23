//! tb's own GitHub sync never closes a card (store/verifier.rs): a merged PR or a closed issue
//! moves its card to REVIEW, with an event saying so, and a verifier moves it on to DONE.
use std::collections::HashMap;
use terminal_board::github::{apply_moves, plan_moves, GhSnapshot, RefState};
use terminal_board::store::Store;

#[test]
fn a_closed_issue_or_merged_pr_lands_in_review_never_done() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    s.set_wip(9).unwrap();
    let closed = s.add("web: gh#21 a closed issue", "", &[], "lead").unwrap();
    s.take(closed, "bot-1").unwrap();
    let merged = s.add("web: gh#20 a merged pr", "", &[], "lead").unwrap();
    s.take(merged, "bot-2").unwrap();
    let waiting = s.add("web: gh#22 closed while in review", "", &[], "lead").unwrap();
    s.take(waiting, "bot-3").unwrap();
    s.done(waiting, "bot-3").unwrap();
    let states: HashMap<i64, RefState> = [
        (20, RefState { closed: true, pr: true, merged: true }),
        (21, RefState { closed: true, pr: false, merged: false }),
        (22, RefState { closed: true, pr: false, merged: false }),
    ]
    .into();
    let snap = GhSnapshot { repo: "o/r".into(), ..Default::default() };
    let moves = plan_moves(&snap, &s.list().unwrap(), &states, &HashMap::new());
    let got: Vec<(i64, &str)> = moves.iter().map(|m| (m.card_id, m.to.as_str())).collect();
    assert!(got.contains(&(closed, "review")) && got.contains(&(merged, "review")), "{got:?}");
    assert!(moves.iter().all(|m| m.to != "done"), "sync never plans a move to done: {got:?}");
    assert!(moves.iter().all(|m| m.card_id != waiting), "a card already in review is left for its verifier: {got:?}");
    apply_moves(&mut s, &moves).unwrap();
    for (id, what) in [(closed, "issue gh#21 closed"), (merged, "PR gh#20 merged")] {
        assert_eq!(s.card(id).unwrap().column, "review");
        let ev = s.show(id).unwrap().events;
        assert!(ev.iter().any(|e| e.actor == "github" && e.kind == "moved" && e.text.ends_with("-> review")), "{ev:?}");
        assert!(ev.iter().any(|e| e.actor == "github" && e.text == format!("{what} → review (a verifier moves it to done)")), "{ev:?}");
    }
    // a second pass moves nothing
    assert!(plan_moves(&snap, &s.list().unwrap(), &states, &HashMap::new()).is_empty());
}
