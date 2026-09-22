//! What a board LOOKS like — chrome, never contract.
//!
//! `card-line age|due` picks what the card line shows where the age is; `label COLUMN "TEXT"`
//! gives a column a display name; a column ordered by due date says so in its header. None of
//! it changes what a command accepts or what JSON calls a column: the internal names `todo`,
//! `doing`, `review`, `done` are the API, a label is only what a person reads. Every default
//! here is the look tb always had, so a board that sets nothing renders byte for byte as before.

use super::due::{self, DueCtx, DueInfo};
use super::{err, Card, Result, Store, COLUMNS};
use rusqlite::OptionalExtension;
use serde::Serialize;

/// Longest label, in characters. A header has to keep its count next to the name.
pub const LABEL_MAX: usize = 24;

/// What the card line shows where the age is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardLine {
    /// Time in the column (`2d`). The default; the look tb always had.
    #[default]
    Age,
    /// The due date and the days left (`due Oct 9 - 18d`); a card without a date shows its age.
    Due,
}

impl CardLine {
    pub fn as_str(self) -> &'static str {
        match self {
            CardLine::Age => "age",
            CardLine::Due => "due",
        }
    }

    pub fn parse(value: &str) -> Result<CardLine> {
        match value.trim().to_ascii_lowercase().as_str() {
            "age" => Ok(CardLine::Age),
            "due" => Ok(CardLine::Due),
            _ => err(format!("unknown card-line '{}' — use 'tb config card-line age' or 'tb config card-line due'", value.trim())),
        }
    }
}

/// The internal column name for `typed` (`REVIEW` -> `review`), or a refusal that lists them.
pub fn column_named(typed: &str) -> Result<&'static str> {
    let t = typed.trim().to_ascii_lowercase();
    COLUMNS.iter().copied().find(|c| *c == t).ok_or_else(|| {
        super::BoardError(format!(
            "unknown column '{}' — a label goes on todo, doing, review or done: 'tb config label review \"WITH REVIEWER\"'",
            typed.trim()
        ))
    })
}

/// A label as it will be stored and shown: control characters and escape sequences removed,
/// whitespace collapsed; refused when nothing is left or it is longer than `LABEL_MAX`.
pub fn clean_label(column: &str, text: &str) -> Result<String> {
    let label = crate::text::sanitize(text).split_whitespace().collect::<Vec<_>>().join(" ");
    // A label that IS a column name would put a second meaning on a word every command takes
    // (`label todo "done"`, then `tb move 1 done`): refuse it rather than have two answers to
    // one word. Internal names are the API; a label is chrome and must not look like one.
    if let Some(clash) = COLUMNS.iter().find(|c| label.trim().eq_ignore_ascii_case(c)) {
        return err(format!(
            "'{label}' is the name of a column, so it cannot be a label — every command takes {clash}; pick another word: 'tb config label {column} \"TEXT\"'"
        ));
    }
    if label.is_empty() {
        return err(format!(
            "the label is empty — set one with 'tb config label {column} \"TEXT\"' or clear it with 'tb config label {column} --off'"
        ));
    }
    let n = label.chars().count();
    if n > LABEL_MAX {
        return err(format!("that label is {n} characters, the limit is {LABEL_MAX} — shorten it: 'tb config label {column} \"TEXT\"'"));
    }
    Ok(label)
}

/// Everything the renderers need to know about this board's look. `Default` is the look tb
/// always had (and knows no "today", so it marks nothing).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Display {
    pub card_line: CardLine,
    /// Display names in `COLUMNS` order; None = the internal name in capitals, as always.
    pub labels: [Option<String>; 4],
    /// The board's today and `due-warn`, for the due mark. None = mark nothing.
    pub due: Option<DueCtx>,
    /// The board orders TODO and REVIEW by due date (`sort due`).
    pub by_due: bool,
}

impl Display {
    fn index(column: &str) -> Option<usize> {
        COLUMNS.iter().position(|c| *c == column)
    }

    /// The label a board gave `column`, if any (sanitised again: another writer may have set it).
    pub fn label(&self, column: &str) -> Option<String> {
        let raw = self.labels.get(Self::index(column)?)?.as_deref()?;
        Some(crate::text::sanitize(raw)).filter(|l| !l.trim().is_empty())
    }

    /// Every column whose label is exactly another column's internal name (only a file an
    /// older tb or another writer wrote can hold one — `config label` refuses them now).
    pub fn clashing_labels(&self) -> Vec<(&'static str, String)> {
        COLUMNS
            .iter()
            .filter_map(|c| {
                let l = self.label(c)?;
                COLUMNS.iter().any(|k| l.trim().eq_ignore_ascii_case(k)).then_some((*c, l))
            })
            .collect()
    }

    /// What a person reads for `column`: its label, else the internal name in capitals.
    /// This is JSON's `column_label`. `column` itself never changes.
    pub fn column_label(&self, column: &str) -> String {
        self.label(column).unwrap_or_else(|| column.to_ascii_uppercase())
    }

    /// How a message names a column: always the internal name a command accepts, with the
    /// label after it when the board shows one — `review (shown as WITH REVIEWER)`.
    pub fn typed(&self, column: &str) -> String {
        match self.label(column) {
            Some(l) => format!("{column} (shown as {l})"),
            None => column.to_string(),
        }
    }

    /// The internal column whose LABEL is `typed` — for the refusal that tells someone who
    /// typed a label which name the command wants.
    pub fn column_of_label(&self, typed: &str) -> Option<&'static str> {
        let t = typed.trim();
        // a word that is an internal column name is that column, whatever any label says —
        // the same rule the callers apply, kept here so no caller can get it wrong
        if COLUMNS.iter().any(|c| t.eq_ignore_ascii_case(c)) {
            return None;
        }
        COLUMNS.iter().copied().find(|c| self.label(c).is_some_and(|l| l.eq_ignore_ascii_case(t)))
    }

    /// Is this column ordered by due date? Its header says so.
    pub fn date_ordered(&self, column: &str) -> bool {
        self.by_due && matches!(column, "todo" | "review")
    }

    /// `days_left` / `due_state` of a card on this board's today (nothing without a today).
    pub fn due_info(&self, card: &Card) -> DueInfo {
        self.due.map(|d| d.info(card)).unwrap_or_default()
    }

    /// `value` with `column_label` added after its own fields (for `--json`).
    pub fn with<T: Serialize>(&self, value: T, card: &Card) -> WithLabel<T> {
        WithLabel { inner: value, column_label: self.column_label(&card.column) }
    }
}

/// A card-shaped value plus its column's display name.
#[derive(Debug, Serialize)]
pub struct WithLabel<T: Serialize> {
    #[serde(flatten)]
    pub inner: T,
    pub column_label: String,
}

impl Store {
    fn display_config(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM config WHERE key=?", [key], |r| r.get(0)).optional()?)
    }

    pub fn card_line(&self) -> Result<CardLine> {
        Ok(self.display_config("card-line")?.and_then(|v| CardLine::parse(&v).ok()).unwrap_or_default())
    }

    pub fn set_card_line(&self, value: &str) -> Result<CardLine> {
        let v = CardLine::parse(value)?;
        self.set_config("card-line", v.as_str())?;
        Ok(v)
    }

    /// The label of `column` (an internal name), None when the board shows the plain name.
    pub fn label(&self, column: &str) -> Result<Option<String>> {
        let column = column_named(column)?;
        Ok(self.display_config(&format!("label.{column}"))?.filter(|l| !l.trim().is_empty()))
    }

    /// Set (`Some`) or clear (`None`) a column's display label. Returns the label now shown.
    pub fn set_label(&self, column: &str, text: Option<&str>) -> Result<Option<String>> {
        let column = column_named(column)?;
        match text {
            Some(t) => {
                let label = clean_label(column, t)?;
                self.set_config(&format!("label.{column}"), &label)?;
                Ok(Some(label))
            }
            None => {
                self.conn.execute("DELETE FROM config WHERE key=?", [format!("label.{column}")])?;
                Ok(None)
            }
        }
    }

    /// Does the board order TODO and REVIEW by due date? Read from the `sort` setting by its
    /// stored value, so this module needs nothing from the ordering code.
    pub fn sorted_by_due(&self) -> Result<bool> {
        Ok(self.display_config("sort")?.is_some_and(|v| v.trim().eq_ignore_ascii_case("due")))
    }

    /// The board's look, read once per render.
    pub fn display(&self) -> Result<Display> {
        let mut labels: [Option<String>; 4] = Default::default();
        for (i, c) in COLUMNS.iter().enumerate() {
            labels[i] = self.label(c)?;
        }
        Ok(Display { card_line: self.card_line()?, labels, due: Some(self.due_ctx()?), by_due: self.sorted_by_due()? })
    }

    /// The look settings this board has SET, for the `tb config` listing (a board that sets
    /// none lists none; `tb config card-line` / `tb config label COLUMN` read one either way).
    pub fn display_settings(&self) -> Result<Vec<(String, String)>> {
        let mut v = Vec::new();
        if self.display_config("card-line")?.is_some() {
            v.push(("card-line".to_string(), self.card_line()?.as_str().to_string()));
        }
        for c in COLUMNS {
            if let Some(l) = self.label(c)? {
                v.push((format!("label.{c}"), l));
            }
        }
        Ok(v)
    }

    /// A stored `tz` this build's zone database does not know (tb then falls back to the
    /// machine's zone — and must say so rather than do it silently).
    pub fn unknown_tz(&self) -> Result<Option<String>> {
        Ok(self.display_config("tz")?.filter(|v| due::parse_tz(v).is_err()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_cleaned_and_bounded() {
        assert_eq!(clean_label("review", "  WITH   REVIEWER ").unwrap(), "WITH REVIEWER");
        assert_eq!(clean_label("review", "WITH\x1b[31m RED\x07\tTAB").unwrap(), "WITH RED TAB", "escape sequences and controls go");
        assert_eq!(clean_label("done", "審査 完了 ✅").unwrap(), "審査 完了 ✅");
        let e = clean_label("review", " \x1b[0m ").unwrap_err().to_string();
        assert!(e.starts_with("the label is empty — ") && e.contains("'tb config label review --off'"), "{e}");
        let e = clean_label("todo", &"x".repeat(25)).unwrap_err().to_string();
        assert_eq!(e, "that label is 25 characters, the limit is 24 — shorten it: 'tb config label todo \"TEXT\"'");
        assert!(clean_label("todo", &"x".repeat(24)).is_ok());
    }

    #[test]
    fn a_label_can_never_be_a_column_name_and_never_shadows_one() {
        // end (b): setting such a label is refused, whatever the case or padding
        for text in ["done", "DONE", " Done ", "todo", "doing", "review"] {
            let e = clean_label("todo", text).unwrap_err().to_string();
            assert!(e.contains("is the name of a column, so it cannot be a label"), "{text:?}: {e}");
            assert!(e.contains("'tb config label todo \"TEXT\"'"), "{text:?}: {e}");
        }
        assert!(clean_label("todo", "DONE DEALS").is_ok(), "only the whole label clashes");
        // end (c): a file that already holds one (an older tb) never shadows the real column
        let mut d = Display { labels: [Some("done".into()), Some("review".into()), None, None], ..Default::default() };
        assert_eq!(d.clashing_labels(), [("todo", "done".to_string()), ("doing", "review".to_string())]);
        for name in ["done", "DONE", " review ", "todo", "doing"] {
            assert_eq!(d.column_of_label(name), None, "{name:?} is a column, not a label");
        }
        // a label that is not a column name still resolves
        d.labels[2] = Some("WITH REVIEWER".into());
        assert_eq!(d.column_of_label("with reviewer"), Some("review"));
    }

    #[test]
    fn a_label_is_chrome_and_messages_keep_the_name_to_type() {
        let mut d = Display::default();
        assert_eq!(d.column_label("review"), "REVIEW");
        assert_eq!(d.typed("review"), "review");
        d.labels[2] = Some("WITH REVIEWER".into());
        assert_eq!(d.column_label("review"), "WITH REVIEWER");
        assert_eq!(d.typed("review"), "review (shown as WITH REVIEWER)");
        assert_eq!(d.column_of_label(" with reviewer "), Some("review"));
        assert_eq!(d.column_of_label("review"), None, "an internal name is not a label");
        assert_eq!(d.column_label("todo"), "TODO");
        // a label another writer stored raw is still cleaned on the way out
        d.labels[0] = Some("IN\x1b[2JBOX".into());
        assert_eq!(d.column_label("todo"), "INBOX");
    }

    #[test]
    fn only_todo_and_review_are_ever_date_ordered() {
        let d = Display { by_due: true, ..Default::default() };
        assert!(d.date_ordered("todo") && d.date_ordered("review"));
        assert!(!d.date_ordered("doing") && !d.date_ordered("done"));
        assert!(!Display::default().date_ordered("todo"));
    }

    #[test]
    fn column_names_and_card_line_values_are_refused_with_the_command_to_run() {
        assert_eq!(column_named(" Review ").unwrap(), "review");
        let e = column_named("with reviewer").unwrap_err().to_string();
        assert!(e.starts_with("unknown column 'with reviewer' — ") && e.contains("'tb config label review"), "{e}");
        assert_eq!(CardLine::parse("DUE").unwrap(), CardLine::Due);
        let e = CardLine::parse("date").unwrap_err().to_string();
        assert_eq!(e, "unknown card-line 'date' — use 'tb config card-line age' or 'tb config card-line due'");
    }
}
