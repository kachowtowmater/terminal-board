//! The ONE ordering of cards.
//!
//! Everything that lists, draws or hands out cards sorts them with `cmp` — `tb next`,
//! `tb next --review`, `tb list`, `tb board`, every `--json` board and the full-screen board —
//! so they can never disagree about which card comes first. There is no second ordering
//! anywhere (no `ORDER BY` decides a queue): a card `tb next` hands out is, by construction,
//! the first unblocked card of the TODO column everyone is looking at.
//!
//! `sort position` (the default) is the order tb always had: `position`, then id.
//! `sort due` orders TODO and REVIEW by due date, nearest first, so an overdue card comes
//! before everything; cards without a date come after every dated card; and cards with the
//! same date — or with none — keep their `position` order, then id. The order is therefore
//! total and deterministic, and it never looks at the clock: dates compare as dates, so
//! "today" (the board's `tz`, `TB_NOW`) cannot change which card is next.
//! DOING stays in position order (it is work in hand, not a queue) and DONE stays newest
//! first: a finished card's date never orders anything.

use super::{Code, due, err, Card, Result, Store, COLUMNS};
use rusqlite::{Connection, OptionalExtension};
use std::cmp::Ordering;

/// The board's `sort` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// By `position` (top first) — a priority queue. The default; exactly tb's order so far.
    #[default]
    Position,
    /// TODO and REVIEW by due date, nearest first — a deadline queue.
    Due,
}

impl Sort {
    pub fn as_str(self) -> &'static str {
        match self {
            Sort::Position => "position",
            Sort::Due => "due",
        }
    }

    pub fn parse(value: &str) -> Result<Sort> {
        match value.trim().to_ascii_lowercase().as_str() {
            "position" => Ok(Sort::Position),
            "due" => Ok(Sort::Due),
            _ => err(format!("unknown sort '{}' — use 'tb config sort position' or 'tb config sort due'", value.trim()), Code::InvalidValue),
        }
    }

    /// Does the due date order this column? Only TODO and REVIEW, and only under `sort due`.
    pub fn by_date(self, column: &str) -> bool {
        self == Sort::Due && matches!(column, "todo" | "review")
    }
}

/// `position`, then id: tb's order so far, and the tie-break of every other order. It is also
/// what `tb prio` edits, whatever the board sorts by.
pub fn by_position(a: &Card, b: &Card) -> Ordering {
    (a.position, a.id).cmp(&(b.position, b.id))
}

/// The order of two cards OF THE SAME COLUMN under `sort`.
pub fn cmp(sort: Sort, a: &Card, b: &Card) -> Ordering {
    if a.column == "done" {
        // newest first; the due date of a finished card orders nothing
        return (std::cmp::Reverse(a.column_since), a.id).cmp(&(std::cmp::Reverse(b.column_since), b.id));
    }
    if !sort.by_date(&a.column) {
        return by_position(a, b);
    }
    // a date is a `YYYY-MM-DD` due date; older free text in the column counts as no date
    let date = |c: &Card| c.due.as_deref().and_then(due::parse_date);
    match (date(a), date(b)) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| by_position(a, b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => by_position(a, b),
    }
}

/// The order of two cards anywhere on the board: column by column (todo, doing, review,
/// done), then `cmp`.
pub fn cmp_board(sort: Sort, a: &Card, b: &Card) -> Ordering {
    let col = |c: &Card| COLUMNS.iter().position(|k| *k == c.column).unwrap_or(COLUMNS.len());
    col(a).cmp(&col(b)).then_with(|| cmp(sort, a, b))
}

/// The board's `sort`, read on `conn` (inside the claim transaction for `tb next`).
pub(super) fn sort_of(conn: &Connection) -> Result<Sort> {
    let v: Option<String> = conn.query_row("SELECT value FROM config WHERE key='sort'", [], |r| r.get(0)).optional()?;
    Ok(v.and_then(|s| Sort::parse(&s).ok()).unwrap_or_default())
}

impl Store {
    /// The board's `sort`; `position` when unset (or when the stored value is not one tb knows).
    pub fn sort(&self) -> Result<Sort> {
        sort_of(&self.conn)
    }

    pub fn set_sort(&self, value: &str) -> Result<Sort> {
        let sort = Sort::parse(value)?;
        self.set_config("sort", sort.as_str())?;
        Ok(sort)
    }

    /// `sort` for the `tb config` listing — listed once the board sets it, so a board that
    /// sets nothing lists exactly what it always did. `tb config sort` reads it either way.
    pub fn sort_settings(&self) -> Result<Vec<(String, String)>> {
        let set: Option<String> =
            self.conn.query_row("SELECT value FROM config WHERE key='sort'", [], |r| r.get(0)).optional()?;
        Ok(match set {
            Some(_) => vec![("sort".to_string(), self.sort()?.as_str().to_string())],
            None => Vec::new(),
        })
    }

    /// Where card `id` shows in its column under the board's order: (place, cards in the
    /// column), 1-based. For `tb prio`, which edits `position` — only the tie-break of a
    /// due-sorted column — and has to say where the card actually ended up.
    pub fn place(&self, id: i64) -> Result<(usize, usize)> {
        let snap = self.snapshot()?;
        let card = self.card(id)?;
        let col = snap.in_column(&card.column);
        let at = col.iter().position(|c| c.id == id).map_or(0, |i| i + 1);
        Ok((at, col.len()))
    }
}

impl super::Snapshot {
    /// The cards of `tb list --json`. Under `sort position` that array is in id order, as it
    /// always was; under `sort due` it is in the board's order — column by column, each as
    /// `tb list` prints it — so it cannot disagree with `tb next` about what comes first.
    pub fn listed(&self) -> Vec<&Card> {
        let mut v: Vec<&Card> = self.cards.iter().collect();
        if self.sort != Sort::Position {
            v.sort_by(|a, b| cmp_board(self.sort, a, b));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: i64, column: &str, position: i64, due: Option<&str>) -> Card {
        Card {
            id,
            title: format!("card {id}"),
            tag: None,
            description: String::new(),
            column: column.into(),
            owner: None,
            due: due.map(str::to_string),
            gh_ref: None,
            created_at: 0,
            column_since: id * 10,
            blocked: None,
            blocked_on: None,
            blocked_until: None,
            position,
            reviewer: None,
        }
    }

    fn ids(sort: Sort, cards: &[Card]) -> Vec<i64> {
        let mut v: Vec<&Card> = cards.iter().collect();
        v.sort_by(|a, b| cmp(sort, a, b));
        v.iter().map(|c| c.id).collect()
    }

    #[test]
    fn position_is_position_then_id_whatever_the_dates_say() {
        let cards = [
            card(1, "todo", 2, Some("2026-01-01")),
            card(2, "todo", 0, None),
            card(3, "todo", 1, Some("2030-01-01")),
            card(4, "todo", 1, Some("2020-01-01")), // same position as #3: id decides
        ];
        assert_eq!(ids(Sort::Position, &cards), [2, 3, 4, 1]);
        assert_eq!(Sort::default(), Sort::Position);
    }

    #[test]
    fn due_is_nearest_first_then_undated_and_ties_keep_position() {
        let cards = [
            card(1, "todo", 0, None),               // undated, top position
            card(2, "todo", 1, Some("2026-10-09")), // same date as #4 and #6
            card(3, "todo", 2, Some("2026-03-01")), // the earliest: first, however far down it sits
            card(4, "todo", 3, Some("2026-10-09")),
            card(5, "todo", 4, Some("Sep 22")), // older free text counts as no date
            card(6, "todo", 0, Some("2026-10-09")), // same date, higher position than #2: before it
            card(7, "todo", 5, Some("2027-01-01")),
            card(8, "todo", 0, None), // same position as #1: id decides
        ];
        assert_eq!(ids(Sort::Due, &cards), [3, 6, 2, 4, 7, 1, 8, 5]);
        // the same cards in REVIEW order the same way; in DOING the date orders nothing
        let review: Vec<Card> = cards.iter().cloned().map(|c| Card { column: "review".into(), ..c }).collect();
        assert_eq!(ids(Sort::Due, &review), [3, 6, 2, 4, 7, 1, 8, 5]);
        let doing: Vec<Card> = cards.iter().cloned().map(|c| Card { column: "doing".into(), ..c }).collect();
        assert_eq!(ids(Sort::Due, &doing), ids(Sort::Position, &doing));
        assert_eq!(ids(Sort::Due, &doing), [1, 6, 8, 2, 3, 4, 5, 7]);
    }

    #[test]
    fn done_is_newest_first_under_either_sort_and_its_dates_order_nothing() {
        let done = [
            card(1, "done", 0, Some("2026-01-01")),
            card(2, "done", 1, None),
            card(3, "done", 2, Some("2020-01-01")),
        ];
        assert_eq!(ids(Sort::Due, &done), [3, 2, 1], "column_since, newest first");
        assert_eq!(ids(Sort::Position, &done), [3, 2, 1]);
    }

    #[test]
    fn the_order_is_total_so_any_shuffle_sorts_the_same() {
        let cards: Vec<Card> = (1..=30)
            .map(|i| {
                let due = match i % 4 {
                    0 => None,
                    1 => Some("2026-10-09"),
                    2 => Some("2026-10-10"),
                    _ => Some("not a date"),
                };
                card(i, "todo", i % 3, due)
            })
            .collect();
        let want = ids(Sort::Due, &cards);
        for step in [7, 11, 13, 17] {
            let shuffled: Vec<Card> = (0..30).map(|i| cards[(i * step) % 30].clone()).collect();
            assert_eq!(ids(Sort::Due, &shuffled), want, "step {step}");
        }
        for w in want.windows(2) {
            let (a, b) = (&cards[w[0] as usize - 1], &cards[w[1] as usize - 1]);
            assert_eq!(cmp(Sort::Due, a, b), Ordering::Less, "#{} before #{}", a.id, b.id);
            assert_eq!(cmp(Sort::Due, b, a), Ordering::Greater);
        }
    }

    #[test]
    fn across_the_board_columns_come_first() {
        let cards = [card(1, "done", 0, None), card(2, "review", 0, Some("2026-01-01")), card(3, "todo", 5, None), card(4, "doing", 0, None)];
        let mut v: Vec<&Card> = cards.iter().collect();
        v.sort_by(|a, b| cmp_board(Sort::Due, a, b));
        assert_eq!(v.iter().map(|c| c.id).collect::<Vec<_>>(), [3, 4, 2, 1]);
    }

    #[test]
    fn sort_parses_and_refuses_with_the_command_to_run() {
        assert_eq!(Sort::parse("due").unwrap(), Sort::Due);
        assert_eq!(Sort::parse(" Position ").unwrap(), Sort::Position);
        let e = Sort::parse("date").unwrap_err().to_string();
        assert_eq!(e, "unknown sort 'date' — use 'tb config sort position' or 'tb config sort due'");
    }
}
