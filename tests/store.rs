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
