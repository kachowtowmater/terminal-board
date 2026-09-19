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
    assert!(e.contains("doing is full (2/2)"), "{e}");
    assert!(e.contains("tb done"), "names the next command: {e}");
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
    // plain todo straight to done
    let p = s.add("plain", "", &[], "me").unwrap();
    assert_eq!(s.done(p, "me").unwrap().column, "done");
    let snap = s.snapshot().unwrap();
    assert_eq!(snap.last_note.get(&id).map(String::as_str), Some("halfway"));
    let kinds: Vec<_> = s.show(id).unwrap().events.into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&"taken".to_string()) && kinds.contains(&"dropped".to_string()));
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
