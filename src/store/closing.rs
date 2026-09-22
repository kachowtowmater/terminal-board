//! Who may close a card, who has checked it, and the tag the user chose.
//!
//! **`done-by` is an honest-mistake stop, not security.** A board can name who closes its
//! cards (`tb config done-by anna,ben`), and tb then refuses to move a card into DONE as
//! anybody else. Names in tb are SELF-ASSERTED — `--as` is whatever the caller types — so
//! this stops a slip, exactly like the never-self-verify rule it sits next to. It stops
//! nobody who means to get past it: `--as anna` is all it takes, and `--force` is logged and
//! open to everyone. A board that needs real authority needs it outside tb (file
//! permissions, a repo, a person). The docs say this in the same words.
//!
//! `tb done ID --approve` records that somebody checked a card and leaves it in REVIEW. It
//! works on every card, not only one linked to a GitHub issue; the card's author still
//! cannot record their own approval, and `done-by` does not gate it — noting "I checked
//! this" is not closing it.
//!
//! An explicit `--tag KEY` is the tag the user chose, instead of the one tb guesses from a
//! `tag:` prefix. It may hold digits, spaces and hyphens, which a guessed tag may not, so a
//! title like `due 10/9 (file by 10/6): …` can carry a real tag.

use super::{err, Card, Connection, Event, Result, Store};
use rusqlite::OptionalExtension;

/// Longest tag, in characters — the width a card line budgets for one.
pub const TAG_MAX: usize = 20;

/// `--tag KEY` cleaned: lower-cased, whitespace collapsed. Letters, digits, spaces, hyphens
/// and underscores; `none` clears it. Anything else is refused with the command to run.
pub fn clean_tag(raw: &str, example: &str) -> Result<Option<String>> {
    let tag = crate::text::sanitize(raw).split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    if tag == "none" {
        return Ok(None);
    }
    if tag.is_empty() {
        return err(format!("the tag is empty — give one, e.g. '{example}' (or --tag none to clear it)"));
    }
    let n = tag.chars().count();
    if n > TAG_MAX {
        return err(format!("that tag is {n} characters, the limit is {TAG_MAX} — shorten it: '{example}'"));
    }
    if let Some(bad) = tag.chars().find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == ' ')) {
        return err(format!(
            "a tag holds letters, digits, spaces, hyphens and underscores — '{bad}' is none of those: '{example}'"
        ));
    }
    Ok(Some(tag))
}

/// Could this tag have been guessed from a `tag:` title prefix? Only such a tag is rebuilt
/// into the raw title the edit form shows, and only such a tag is re-read from an edited
/// title — so an explicit tag (`00-key 2`) is never eaten by editing the title.
pub fn from_a_title_prefix(tag: &str) -> bool {
    super::parse_title(&format!("{tag}: x")).0.as_deref() == Some(tag)
}

/// The names that may close a card, as stored. Empty = anyone, which is the default.
pub fn parse_names(list: &str) -> Vec<String> {
    list.split(',').map(|n| n.trim()).filter(|n| !n.is_empty()).map(|n| n.to_string()).collect()
}

/// Everyone who recorded `tb done ID --approve` on a card, oldest first, without repeats.
pub fn approved_by(events: &[Event]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for e in events.iter().filter(|e| e.kind == "approved") {
        if !v.iter().any(|a| a.eq_ignore_ascii_case(&e.actor)) {
            v.push(e.actor.clone());
        }
    }
    v
}

/// The `done-by` list on `conn` (inside the transition's transaction).
pub(super) fn done_by_of(conn: &Connection) -> Result<Vec<String>> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM config WHERE key='done-by'", [], |r| r.get(0)).optional()?;
    Ok(v.map(|v| parse_names(&v)).unwrap_or_default())
}

/// The refusal when `actor` may not close a card. Names the list and the way through, and
/// says what this is: a stop for an honest mistake, not a lock.
pub(super) fn not_allowed(id: i64, actor: &str, names: &[String]) -> super::BoardError {
    super::BoardError(format!(
        "only {} may close a card on this board ({actor} is not on the list) — ask one of them to run 'tb done {id}', or 'tb done {id} --force' if you mean it (logged)",
        names.join(" or ")
    ))
}

/// May `actor` move a card into DONE? `github` is exempt: its moves are evidence-driven
/// automation (a merged PR), not a person closing a card — the same exemption the holder
/// rule makes.
pub(super) fn may_close(conn: &Connection, actor: &str) -> Result<Option<Vec<String>>> {
    let names = done_by_of(conn)?;
    if names.is_empty() || actor == "github" || names.iter().any(|n| n.eq_ignore_ascii_case(actor)) {
        return Ok(None);
    }
    Ok(Some(names))
}

impl Store {
    /// The names that may close a card; empty = anyone (the default).
    pub fn done_by(&self) -> Result<Vec<String>> {
        done_by_of(&self.conn)
    }

    /// `anna,ben` sets the list; None clears it. Returns the list now in force.
    pub fn set_done_by(&self, list: Option<&str>) -> Result<Vec<String>> {
        let Some(list) = list else {
            self.conn.execute("DELETE FROM config WHERE key='done-by'", [])?;
            return Ok(Vec::new());
        };
        let names = parse_names(list);
        if names.is_empty() {
            return err(
                "say who may close a card — 'tb config done-by anna,ben', or 'tb config done-by --off' to let anyone".to_string(),
            );
        }
        for n in &names {
            if n.chars().count() > 32 || n.contains(char::is_whitespace) && n.split_whitespace().count() > 4 {
                return err(format!("'{n}' does not look like a name — 'tb config done-by anna,ben'"));
            }
        }
        self.set_config("done-by", &names.join(","))?;
        Ok(names)
    }

    /// `done-by` for the `tb config` listing — listed once the board sets it, so a board that
    /// sets nothing lists exactly what it always did.
    pub fn closing_settings(&self) -> Result<Vec<(String, String)>> {
        let names = self.done_by()?;
        Ok(if names.is_empty() { Vec::new() } else { vec![("done-by".to_string(), names.join(","))] })
    }

    /// Record that `actor` checked card `id`: an `approved` event; the card does not move.
    /// Refused when the card is not in REVIEW, and when the actor did the work themselves.
    pub fn approve(&self, id: i64, actor: &str) -> Result<Card> {
        let c = self.card(id)?;
        if c.column != "review" {
            return err(format!(
                "#{id} is not in review — an approval records a review pass; move it there first: 'tb move {id} review'"
            ));
        }
        if self.author(id)?.is_some_and(|a| a.eq_ignore_ascii_case(actor)) {
            return err(format!("you did this work — ask another person or agent to approve #{id}"));
        }
        // the text says what happened; what the card waits for next depends on the card
        let waits = if c.gh_ref.is_some() { "done waits for the merge" } else { "it stays in review" };
        self.note_kind(id, actor, &format!("checked by {actor} ({waits})"), "approved")?;
        self.card(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(actor: &str, kind: &str) -> Event {
        Event { card_id: 1, ts: 0, actor: actor.into(), kind: kind.into(), text: String::new(), actor_id: None }
    }

    #[test]
    fn a_tag_may_hold_digits_spaces_and_hyphens() {
        for (raw, want) in [
            ("filing", Some("filing")),
            ("  FILING  ", Some("filing")),
            ("00-key 2", Some("00-key 2")),
            ("client-a  matter 7", Some("client-a matter 7")),
            ("none", None),
            ("NONE", None),
        ] {
            assert_eq!(clean_tag(raw, "x").unwrap().as_deref(), want, "{raw:?}");
        }
        let e = clean_tag("", "tb add \"title\" --tag filing").unwrap_err().to_string();
        assert!(e.starts_with("the tag is empty — ") && e.contains("--tag none"), "{e}");
        let e = clean_tag(&"x".repeat(21), "tb add \"title\" --tag filing").unwrap_err().to_string();
        assert!(e.starts_with("that tag is 21 characters, the limit is 20"), "{e}");
        for bad in ["a:b", "a/b", "a.b", "a,b", "a#b"] {
            let e = clean_tag(bad, "x").unwrap_err().to_string();
            assert!(e.contains("letters, digits, spaces, hyphens and underscores"), "{bad}: {e}");
        }
        assert!(clean_tag("a\x1b[31mb", "x").unwrap().as_deref() == Some("ab"), "control characters are removed");
    }

    #[test]
    fn only_a_guessable_tag_is_rebuilt_into_a_title() {
        for yes in ["filing", "widgets", "ops2", "a-b", "a_b"] {
            assert!(from_a_title_prefix(yes), "{yes}");
        }
        // exactly what the client found: these can never come from a `tag:` prefix
        for no in ["00-key 2", "client-a matter 7", "a b"] {
            assert!(!from_a_title_prefix(no), "{no}");
        }
    }

    #[test]
    fn done_by_is_a_list_of_names_matched_without_case() {
        assert_eq!(parse_names(" anna , ben ,, "), ["anna", "ben"]);
        assert!(parse_names(" , ").is_empty());
        let e = not_allowed(7, "carol", &["anna".into(), "ben".into()]).to_string();
        assert_eq!(
            e,
            "only anna or ben may close a card on this board (carol is not on the list) — ask one of them to run 'tb done 7', or 'tb done 7 --force' if you mean it (logged)"
        );
    }

    /// The sync moves a card on evidence (a merged PR), not as a person closing it — the
    /// same exemption the holder rule makes. The CLI refuses the name `github` outright, so
    /// this path is reachable only from tb's own sync code.
    #[test]
    fn the_github_sync_is_exempt_from_done_by() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("b.db")).unwrap();
        s.set_done_by(Some("anna")).unwrap();
        let id = s.add("widgets: gh#7 fix it", "", &[], "bob").unwrap();
        s.take(id, "bob").unwrap();
        s.done(id, "bob").unwrap(); // -> review
        let e = s.move_to(id, "done", "carol").unwrap_err().to_string();
        assert!(e.starts_with("only anna may close a card on this board"), "{e}");
        assert_eq!(s.card(id).unwrap().column, "review");
        s.move_to(id, "done", "github").unwrap();
        assert_eq!(s.card(id).unwrap().column, "done");
    }

    #[test]
    fn approved_by_lists_each_checker_once_oldest_first() {
        let events = [ev("anna", "approved"), ev("bob", "note"), ev("BEN", "approved"), ev("anna", "approved")];
        assert_eq!(approved_by(&events), ["anna", "BEN"]);
        assert!(approved_by(&[ev("anna", "note")]).is_empty());
    }
}
