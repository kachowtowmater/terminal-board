//! Which cards a read shows: `tb list` and `tb board --json` take `--tag`, `--owner`,
//! `--blocked`, `--blocked-on`, `--due-before`, `--column` and `--group tag`.
//!
//! A filter REMOVES ROWS AND NOTHING ELSE. The board's order — position, or `sort due`, or
//! whatever a later setting adds — is decided in `store::order` and applied first; a filter
//! then drops cards from that sequence. So a filtered list is always the same list with rows
//! missing, never a reordered one, and a board that passes no filter is byte-identical.
//!
//! Several filters compose, and every one of them has to pass.

use crate::store::blocks::BlockCtx;
use crate::store::due;
use crate::store::{BoardError, Card, Result};

/// The filters as they were typed, before anything is checked. The command line fills this
/// in; `Filter::parse` turns it into a `Filter` or into a refusal.
#[derive(Debug, Clone, Default)]
pub struct Asked {
    pub tag: Option<String>,
    pub owner: Option<String>,
    pub blocked: bool,
    pub blocked_on: Option<String>,
    pub due_before: Option<String>,
    pub column: Option<String>,
    pub group: Option<String>,
}

/// The word that means "there is none": `--tag none` are the cards with no tag,
/// `--owner none` the cards nobody holds.
pub const NONE: &str = "none";

/// How `tb list` groups what it prints. Display only — it never reorders within a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Tag,
}

impl Group {
    pub fn parse(raw: &str) -> Result<Group> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "tag" => Ok(Group::Tag),
            other => crate::store::err(format!(
                "cannot group by '{other}' — the only grouping is 'tb list --group tag'"
            )),
        }
    }
}

/// Everything a read was asked to narrow by. Every field that is set must match.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// A tag, or `none` for the cards without one.
    pub tag: Option<String>,
    /// Who holds it, or `none` for the cards nobody holds.
    pub owner: Option<String>,
    /// Only blocked cards.
    pub blocked: bool,
    /// What a card waits on: `#7`, or a name.
    pub blocked_on: Option<String>,
    /// Cards due strictly before this calendar date. A card with no due date never matches.
    pub due_before: Option<String>,
    /// An internal column name — never a display label (see `parse`).
    pub column: Option<&'static str>,
    pub group: Option<Group>,
}

impl Filter {
    /// Is anything set? (Nothing set = today's output, untouched.)
    pub fn any(&self) -> bool {
        self.tag.is_some()
            || self.owner.is_some()
            || self.blocked
            || self.blocked_on.is_some()
            || self.due_before.is_some()
            || self.column.is_some()
    }

    /// Check the values that can be wrong before a board is read, so a typo is refused with
    /// the same wording everywhere.
    ///
    /// `--column` takes an INTERNAL name only. A label is display text a board chose (`tb
    /// config label review "WITH ATTORNEY"`), and commands take the name — the same
    /// precedence `tb move` follows, where `column_named` resolves before `column_of_label`.
    /// A label passed here is refused, and the refusal says which name to use.
    pub fn parse(asked: Asked, look: &crate::store::display::Display) -> Result<Filter> {
        let Asked { tag, owner, blocked, blocked_on, due_before, column, group } = asked;
        let column = match column.as_deref().map(str::trim) {
            None => None,
            Some(typed) => Some(crate::store::display::column_named(typed).map_err(|e| {
                // a label the board really uses: name the column it stands for
                match look.column_of_label(typed) {
                    Some(real) => BoardError(format!(
                        "'{typed}' is the label on {real} — a filter takes the column name: 'tb list --column {real}'"
                    )),
                    None => e,
                }
            })?),
        };
        if let Some(d) = due_before.as_deref().map(str::trim) {
            if due::parse_date(d).is_none() {
                return crate::store::err(format!(
                    "'{d}' is not a date — use YYYY-MM-DD, e.g. 'tb list --due-before 2026-10-09'"
                ));
            }
        }
        Ok(Filter {
            tag: tag.map(|t| t.trim().to_ascii_lowercase()),
            owner: owner.map(|o| o.trim().to_string()),
            blocked,
            blocked_on: blocked_on.map(|b| b.trim().to_string()),
            due_before: due_before.map(|d| d.trim().to_string()),
            column,
            group: group.as_deref().map(Group::parse).transpose()?,
        })
    }

    /// Does this card pass every filter that is set?
    pub fn keeps(&self, c: &Card, blocks: &BlockCtx) -> bool {
        if let Some(want) = &self.tag {
            let has = c.tag.as_deref().unwrap_or_default().to_ascii_lowercase();
            let ok = if want == NONE { c.tag.is_none() } else { has == *want };
            if !ok {
                return false;
            }
        }
        if let Some(want) = &self.owner {
            let ok = if want.eq_ignore_ascii_case(NONE) {
                c.owner.is_none()
            } else {
                c.owner.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(want))
            };
            if !ok {
                return false;
            }
        }
        if self.blocked && c.blocked.is_none() && c.blocked_on.is_none() && c.blocked_until.is_none() {
            return false;
        }
        if let Some(want) = &self.blocked_on {
            let info = blocks.of(c);
            // `#7` matches `--blocked-on 7` too: a person types the id either way
            let want_id = want.trim_start_matches('#');
            let ok = info.blocked_on.as_deref().is_some_and(|on| {
                on.eq_ignore_ascii_case(want) || on.trim_start_matches('#').eq_ignore_ascii_case(want_id)
            });
            if !ok {
                return false;
            }
        }
        if let Some(before) = &self.due_before {
            // a card with no due date, or with text that is not a date, is never "due before"
            let ok = c.due.as_deref().and_then(due::parse_date).is_some_and(|d| {
                due::parse_date(before).is_some_and(|b| d < b)
            });
            if !ok {
                return false;
            }
        }
        if let Some(want) = self.column {
            if c.column != want {
                return false;
            }
        }
        true
    }

    /// `cards` with everything that does not pass removed. The sequence handed in is already
    /// in the board's order, and this only takes rows out of it.
    pub fn apply<'a>(&self, cards: Vec<&'a Card>, blocks: &BlockCtx) -> Vec<&'a Card> {
        if !self.any() {
            return cards;
        }
        cards.into_iter().filter(|c| self.keeps(c, blocks)).collect()
    }

    /// The tags to print, in the order `--group tag` shows them: every tag that survived,
    /// alphabetically, and the cards with no tag last.
    pub fn groups<'a>(cards: &[&'a Card]) -> Vec<(String, Vec<&'a Card>)> {
        let mut names: Vec<String> = cards.iter().filter_map(|c| c.tag.clone()).collect();
        names.sort();
        names.dedup();
        let mut out: Vec<(String, Vec<&Card>)> = names
            .into_iter()
            .map(|t| {
                let in_tag = cards.iter().copied().filter(|c| c.tag.as_deref() == Some(t.as_str())).collect();
                (t, in_tag)
            })
            .collect();
        let untagged: Vec<&Card> = cards.iter().copied().filter(|c| c.tag.is_none()).collect();
        if !untagged.is_empty() {
            out.push((NONE.to_string(), untagged));
        }
        out
    }

    /// What a read printed nothing for: which filters were asked for, so the person can see
    /// what to loosen.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(t) = &self.tag {
            parts.push(format!("tag {t}"));
        }
        if let Some(o) = &self.owner {
            parts.push(format!("owner {o}"));
        }
        if self.blocked {
            parts.push("blocked".to_string());
        }
        if let Some(b) = &self.blocked_on {
            parts.push(format!("blocked on {b}"));
        }
        if let Some(d) = &self.due_before {
            parts.push(format!("due before {d}"));
        }
        if let Some(c) = self.column {
            parts.push(format!("column {c}"));
        }
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: i64, tag: Option<&str>, owner: Option<&str>, column: &str, due: Option<&str>) -> Card {
        Card {
            id,
            title: format!("card {id}"),
            tag: tag.map(str::to_string),
            description: String::new(),
            column: column.to_string(),
            owner: owner.map(str::to_string),
            due: due.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn nothing_set_keeps_every_card() {
        let f = Filter::default();
        assert!(!f.any());
        let c = card(1, None, None, "todo", None);
        assert!(f.keeps(&c, &BlockCtx::default()));
    }

    #[test]
    fn each_filter_on_its_own() {
        let blocks = BlockCtx::default();
        let a = card(1, Some("docs"), Some("alice"), "todo", Some("2026-10-09"));
        let b = card(2, None, None, "done", None);
        let has = |f: Filter, c: &Card| f.keeps(c, &blocks);
        assert!(has(Filter { tag: Some("docs".into()), ..Default::default() }, &a));
        assert!(!has(Filter { tag: Some("docs".into()), ..Default::default() }, &b));
        assert!(has(Filter { tag: Some(NONE.into()), ..Default::default() }, &b));
        assert!(has(Filter { owner: Some("ALICE".into()), ..Default::default() }, &a), "names match whatever the case");
        assert!(has(Filter { owner: Some("none".into()), ..Default::default() }, &b));
        assert!(has(Filter { column: Some("done"), ..Default::default() }, &b));
        assert!(!has(Filter { column: Some("done"), ..Default::default() }, &a));
        assert!(has(Filter { due_before: Some("2026-10-10".into()), ..Default::default() }, &a));
        assert!(!has(Filter { due_before: Some("2026-10-09".into()), ..Default::default() }, &a), "strictly before");
        assert!(!has(Filter { due_before: Some("2026-10-10".into()), ..Default::default() }, &b), "no due date never matches");
    }

    #[test]
    fn filters_compose() {
        let blocks = BlockCtx::default();
        let a = card(1, Some("docs"), Some("alice"), "todo", None);
        let both = Filter { tag: Some("docs".into()), owner: Some("alice".into()), ..Default::default() };
        assert!(both.keeps(&a, &blocks));
        let wrong = Filter { tag: Some("docs".into()), owner: Some("bob".into()), ..Default::default() };
        assert!(!wrong.keeps(&a, &blocks), "every filter has to pass, not any");
    }

    #[test]
    fn a_filter_only_removes_rows_it_never_reorders() {
        let blocks = BlockCtx::default();
        let cards: Vec<Card> = (1..=6)
            .map(|i| card(i, if i % 2 == 0 { Some("docs") } else { Some("ops") }, None, "todo", None))
            .collect();
        // the sequence handed in is the board's order; whatever comes back is a subsequence
        let given: Vec<&Card> = cards.iter().collect();
        let kept = Filter { tag: Some("docs".into()), ..Default::default() }.apply(given.clone(), &blocks);
        assert_eq!(kept.iter().map(|c| c.id).collect::<Vec<_>>(), [2, 4, 6]);
        let mut it = given.iter();
        for k in &kept {
            assert!(it.any(|g| g.id == k.id), "the order changed");
        }
    }

    #[test]
    fn grouping_is_alphabetical_with_the_untagged_last() {
        let cards = [
            card(1, Some("ops"), None, "todo", None),
            card(2, None, None, "todo", None),
            card(3, Some("docs"), None, "todo", None),
            card(4, Some("ops"), None, "todo", None),
        ];
        let given: Vec<&Card> = cards.iter().collect();
        let groups = Filter::groups(&given);
        let shape: Vec<(String, Vec<i64>)> =
            groups.into_iter().map(|(t, cs)| (t, cs.into_iter().map(|c| c.id).collect())).collect();
        assert_eq!(shape, [("docs".to_string(), vec![3]), ("ops".to_string(), vec![1, 4]), ("none".to_string(), vec![2])]);
    }

    #[test]
    fn a_bad_value_is_refused_before_the_board_is_read() {
        let look = crate::store::display::Display::default();
        let f = |col: Option<&str>, due: Option<&str>, group: Option<&str>| {
            Filter::parse(
                Asked {
                    due_before: due.map(str::to_string),
                    column: col.map(str::to_string),
                    group: group.map(str::to_string),
                    ..Default::default()
                },
                &look,
            )
        };
        let e = f(Some("DONE!"), None, None).unwrap_err().0;
        assert!(e.contains("unknown column 'DONE!'"), "{e}");
        let e = f(None, Some("09/10/2026"), None).unwrap_err().0;
        assert!(e.contains("is not a date") && e.contains("tb list --due-before 2026-10-09"), "{e}");
        let e = f(None, None, Some("owner")).unwrap_err().0;
        assert!(e.contains("cannot group by 'owner'"), "{e}");
        // an internal name in any case is fine
        assert_eq!(f(Some("Review"), None, None).unwrap().column, Some("review"));
    }
}
