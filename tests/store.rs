use std::collections::HashSet;
use std::sync::{Arc, Barrier};
use terminal_board::store::Store;

fn temp_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("board.db");
    (dir, p)
}

/// M2: many concurrent `next` callers never receive the same card.
#[test]
fn concurrent_next_never_duplicates() {
    let (_d, path) = temp_db();
    const CARDS: usize = 40;
    const THREADS: usize = 12;
    {
        let s = Store::open(&path).unwrap();
        s.set_wip(99).unwrap(); // max; 40 cards stay under it
        for i in 0..CARDS {
            s.add(&format!("task {i}"), "", &[], "seed").unwrap();
        }
    }
    let barrier = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut s = Store::open(&path).unwrap();
                let me = format!("agent-{t}");
                barrier.wait();
                let mut got = Vec::new();
                loop {
                    match s.next(&me) {
                        Ok(c) => {
                            assert_eq!(c.owner.as_deref(), Some(me.as_str()));
                            got.push(c.id)
                        }
                        Err(e) => {
                            assert!(e.0.contains("no todo cards"), "unexpected error: {e}");
                            break;
                        }
                    }
                }
                got
            })
        })
        .collect();
    let all: Vec<i64> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
    let uniq: HashSet<i64> = all.iter().copied().collect();
    assert_eq!(all.len(), uniq.len(), "a card was handed out twice: {all:?}");
    assert_eq!(uniq.len(), CARDS, "every card handed out exactly once");
    let s = Store::open(&path).unwrap();
    assert!(s.list().unwrap().iter().all(|c| c.column == "doing"));
}

/// M2: exactly one of 8 racing `take` calls on the same card wins.
#[test]
fn concurrent_take_same_card_one_winner() {
    let (_d, path) = temp_db();
    let id = Store::open(&path).unwrap().add("contested", "", &[], "seed").unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let wins: usize = (0..8)
        .map(|t| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut s = Store::open(&path).unwrap();
                barrier.wait();
                s.take(id, &format!("a{t}")).is_ok() as usize
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .sum();
    assert_eq!(wins, 1);
}

/// M3: WIP limit refuses a take with an actionable message.
#[test]
fn wip_limit_refuses() {
    let (_d, path) = temp_db();
    let mut s = Store::open(&path).unwrap();
    s.set_wip(2).unwrap();
    for i in 0..3 {
        s.add(&format!("t{i}"), "", &[], "x").unwrap();
    }
    s.next("a").unwrap();
    s.next("b").unwrap();
    let e = s.next("c").unwrap_err().to_string();
    assert!(e.contains("doing is full (2/2: #1 a, #2 b)"), "{e}");
    assert!(e.contains("you hold none; wait, or ask one of them to finish"), "actor-aware: {e}");
    let e = s.take(3, "c").unwrap_err().to_string();
    assert!(e.contains("doing is full"), "{e}");
    let e = s.move_to(3, "doing", "c").unwrap_err().to_string();
    assert!(e.contains("doing is full"), "{e}");
    // finishing one frees a slot
    s.done(1, "a").unwrap();
    assert_eq!(s.next("c").unwrap().id, 3);
}

#[test]
fn lifecycle_and_errors() {
    let (_d, path) = temp_db();
    let mut s = Store::open(&path).unwrap();
    let id = s
        .add("widgets: gh#42 fix it", "desc", &["one".into(), "two".into()], "me")
        .unwrap();
    let c = s.card(id).unwrap();
    assert_eq!((c.tag.as_deref(), c.gh_ref, c.title.as_str()), (Some("widgets"), Some(42), "fix it"));
    let e = s.take(99, "me").unwrap_err().to_string();
    assert!(e.contains("tb list"), "{e}");
    assert_eq!(s.take(id, "me").unwrap().column, "doing");
    let e = s.take(id, "you").unwrap_err().to_string();
    assert!(e.contains("tb next"), "{e}");
    assert!(s.check(id, 2, "me").unwrap());
    assert!(!s.check(id, 2, "me").unwrap());
    assert!(s.check(id, 5, "me").unwrap_err().to_string().contains("tb show"));
    s.note(id, "halfway", "me").unwrap();
    assert_eq!(s.done(id, "me").unwrap().column, "review");
    assert_eq!(s.done(id, "you").unwrap().column, "done");
    assert!(s.done(id, "me").unwrap_err().to_string().contains("tb move"));
    let d = s.drop_card(id, "me").unwrap();
    assert_eq!((d.column.as_str(), d.owner), ("todo", None));
    // plain todo straight to done: refused — nothing reaches done except from review
    let p = s.add("plain", "", &[], "me").unwrap();
    let e = s.done(p, "me").unwrap_err();
    assert_eq!(e.1.to_string(), "not_from_review", "{e}");
    assert_eq!(s.card(p).unwrap().column, "todo");
    let snap = s.snapshot().unwrap();
    assert_eq!(snap.last_note.get(&id).map(String::as_str), Some("halfway"));
    assert!(snap.last_event_at.get(&id).is_some_and(|ts| *ts > 0), "last event ts present: {:?}", snap.last_event_at);
    let kinds: Vec<_> = s.show(id).unwrap().events.into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&"taken".to_string()) && kinds.contains(&"dropped".to_string()));
}

#[test]
fn gh_ref_parses_case_insensitively() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    for (title, want) in [
        ("widgets: gh#42 fix it", Some(42)),
        ("widgets: GH#43 fix it", Some(43)),
        ("widgets: Gh#44 fix it", Some(44)),
        ("widgets: gH#45 fix it", Some(45)),
        ("widgets: gh fix it", None),
        ("widgets: gh#notanumber fix it", None),
    ] {
        let id = s.add(title, "", &[], "me").unwrap();
        assert_eq!(s.card(id).unwrap().gh_ref, want, "{title}");
        s.delete_card(id, "me").unwrap();
    }
    // edit keeps the same rule
    let id = s.add("widgets: no ref", "", &[], "me").unwrap();
    s.edit(id, Some("widgets: GH#99 moved"), None, "me", None).unwrap();
    assert_eq!(s.card(id).unwrap().gh_ref, Some(99));
}

#[test]
fn done_clears_a_block_and_records_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = s.add("plain: blocked in review", "", &[], "me").unwrap();
    s.move_to(id, "review", "me").unwrap();
    s.block(id, Some("#9"), "me").unwrap();
    assert_eq!(s.card(id).unwrap().blocked.as_deref(), Some("#9"));
    let c = s.done(id, "rev").unwrap();
    assert_eq!(c.column, "done");
    assert_eq!(c.blocked, None, "the block must not survive into done");
    let d = s.show(id).unwrap();
    let kinds: Vec<_> = d.events.iter().map(|e| (e.kind.as_str(), e.text.as_str())).collect();
    assert!(kinds.contains(&("unblocked", "cleared on done")), "{kinds:?}");
    // a todo→doing move keeps the block behaviour untouched (blocks clear only via block --clear)
    let id2 = s.add("plain: blocked still", "", &[], "me").unwrap();
    s.block(id2, Some("#1"), "me").unwrap();
    s.take(id2, "me").unwrap();
    assert_eq!(s.card(id2).unwrap().blocked.as_deref(), Some("#1"));
}

#[test]
fn done_approve_records_without_moving() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let id = s.add("plain: awaiting merge", "", &[], "me").unwrap();
    s.take(id, "me").unwrap();
    s.done(id, "me").unwrap();
    assert_eq!(s.card(id).unwrap().column, "review");
    let _ = s.note_kind(id, "rev", "approved (the card stays in review; done waits for the merge)", "approved");
    assert_eq!(s.card(id).unwrap().column, "review", "approval must not move the card");
    let ev = s.show(id).unwrap().events;
    let a = ev.iter().find(|e| e.kind == "approved").unwrap();
    assert_eq!(a.actor, "rev");
    assert!(a.text.contains("approved"), "{a:?}");
}

#[test]
fn moves_keep_blocks_except_into_done() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    // a DOING card blocked on an external wait: finishing it to REVIEW keeps the block
    let id = s.add("plain: blocked while working", "", &[], "me").unwrap();
    s.take(id, "me").unwrap();
    s.block(id, Some("waiting on API key"), "me").unwrap();
    s.done(id, "me").unwrap();
    let c = s.card(id).unwrap();
    assert_eq!(c.column, "review");
    assert_eq!(c.blocked.as_deref(), Some("waiting on API key"), "DOING->REVIEW keeps the block");
    // and reaching DONE clears it
    s.done_forced(id, "rev").unwrap();
    assert_eq!(s.card(id).unwrap().blocked, None, "DONE clears the block");
    // a TODO -> REVIEW move keeps a block too
    let id2 = s.add("plain: blocked in todo", "", &[], "me").unwrap();
    s.block(id2, Some("#9"), "me").unwrap();
    s.move_to(id2, "review", "me").unwrap();
    assert_eq!(s.card(id2).unwrap().blocked.as_deref(), Some("#9"));
}

#[test]
fn only_a_leading_gh_ref_is_taken_out_of_the_title() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    // leading ref: as today — moved out, becomes the link
    let id = s.add("fix: gh#12 port the fix", "", &[], "me").unwrap();
    let c = s.card(id).unwrap();
    assert_eq!((c.gh_ref, c.title.as_str(), c.tag.as_deref()), (Some(12), "port the fix", Some("fix")));
    s.delete_card(id, "me").unwrap();
    // mid-title ref: the words STAY, the link is still set
    let id = s.add("fix: port gh#12 fix to gh#13 as well", "", &[], "me").unwrap();
    let c = s.card(id).unwrap();
    assert_eq!(c.gh_ref, Some(12), "the first ref sets the link");
    assert_eq!(c.title, "port gh#12 fix to gh#13 as well", "mid-title wording survives");
    s.delete_card(id, "me").unwrap();
    // two refs, no leading one: link from the first, both stay in the text
    let id = s.add("fix: port gh#12 fix to gh#13", "", &[], "me").unwrap();
    let c = s.card(id).unwrap();
    assert_eq!(c.gh_ref, Some(12));
    assert_eq!(c.title, "port gh#12 fix to gh#13");
    s.delete_card(id, "me").unwrap();
    // uppercase mid-title ref still links
    let id = s.add("fix: port GH#12 fix", "", &[], "me").unwrap();
    let c = s.card(id).unwrap();
    assert_eq!(c.gh_ref, Some(12));
    assert_eq!(c.title, "port GH#12 fix");
}

#[test]
fn a_ref_already_in_the_title_is_not_shown_twice() {
    use terminal_board::store::{parse_title, raw_title, shown_ref};
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("b.db")).unwrap();
    let mid = s.add("fix: port gh#12 fix to gh#13 as well", "", &[], "me").unwrap();
    let upper = s.add("fix: port GH#14 fix", "", &[], "me").unwrap();
    let lead = s.add("fix: gh#15 port the fix", "", &[], "me").unwrap();
    let (m, u, l) = (s.card(mid).unwrap(), s.card(upper).unwrap(), s.card(lead).unwrap());
    assert_eq!((shown_ref(&m), shown_ref(&u), shown_ref(&l)), (None, None, Some(15)));
    let list = terminal_board::plain::list(&s.snapshot().unwrap());
    assert!(list.contains(&format!("#{mid} port gh#12 fix to gh#13 as well")), "{list}");
    assert!(list.contains(&format!("#{upper} port GH#14 fix")), "{list}");
    assert!(list.contains(&format!("#{lead} gh#15 port the fix")), "a leading ref keeps its prefix:\n{list}");
    assert!(!list.contains("gh#12 port gh#12"), "{list}");
    // what `edit` pre-fills parses back to the same card
    for c in [&m, &u, &l] {
        let (tag, gh, title) = parse_title(&raw_title(c));
        assert_eq!((tag, gh, title), (c.tag.clone(), c.gh_ref, c.title.clone()), "{}", raw_title(c));
    }
}
