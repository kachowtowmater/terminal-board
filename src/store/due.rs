//! Due dates.
//!
//! A card's `due` is a LOCAL CALENDAR DATE, not an instant: tb stores the `YYYY-MM-DD` text
//! the user typed and parses it only to check that it is a real date. It is never turned into
//! a timestamp, so no time zone — UTC or any other — can move it to the day before or after.
//!
//! The one place a zone matters is "what is today?", which decides `days_left` and
//! `due_state`. Today is the calendar date of `store::now()` (so `TB_NOW` pins it) in the
//! board's zone: the `tz` setting (an IANA name), or the machine's local zone when unset.
//! `days_left` counts whole calendar days, never seconds / 86400 — a day with a DST change
//! is 23 or 25 hours long.

use super::{err, get_card, now, Card, Result, Store};
use chrono::{NaiveDate, TimeZone};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::Serialize;

/// `due-warn` when the board does not set it: a card is `soon` from 3 days before its date.
pub const DEFAULT_DUE_WARN: i64 = 3;
pub const MAX_DUE_WARN: i64 = 365;
/// The word that clears a due date (`--due none`) and stands for "no date" in the event log.
pub const NONE: &str = "none";
/// The `tz` value that clears the setting: the machine's own zone decides what today is.
pub const LOCAL: &str = "local";

/// Strict `YYYY-MM-DD`: exactly ten characters, digits and two dashes, a date that exists,
/// year 1 to 9999. Anything looser would make the stored text differ from what was typed.
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    let b = s.as_bytes();
    let shaped = b.len() == 10
        && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
    if !shaped {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if y == 0 {
        return None;
    }
    NaiveDate::from_ymd_opt(y as i32, m, d)
}

/// A validated due date: the text as typed. The only way to get one is `DueDate::parse`, and
/// `Store::set_due` takes nothing else, so the column never receives anything but a real
/// `YYYY-MM-DD` date from tb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueDate(String);

impl DueDate {
    /// `none` = clear it (`Ok(None)`); a real `YYYY-MM-DD` date = set it; anything else is
    /// refused. `example` is the command the hint shows, e.g. `tb edit 7 --due 2026-10-09`.
    pub fn parse(raw: &str, example: &str) -> Result<Option<DueDate>> {
        let t = raw.trim();
        if t.eq_ignore_ascii_case(NONE) {
            return Ok(None);
        }
        if parse_date(t).is_some() {
            return Ok(Some(DueDate(t.to_string())));
        }
        let shaped = t.len() == 10 && t.split('-').count() == 3 && t.chars().all(|c| c.is_ascii_digit() || c == '-');
        let what = if t.is_empty() {
            "the due date is empty".to_string()
        } else if shaped {
            format!("'{t}' is not a real calendar date")
        } else {
            format!("'{t}' is not a date")
        };
        err(format!("{what} — use YYYY-MM-DD, e.g. '{example}' (or --due none to clear it)"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An IANA zone name (`America/Los_Angeles`), exactly as the zone database spells it.
pub fn parse_tz(name: &str) -> Result<chrono_tz::Tz> {
    name.trim().parse::<chrono_tz::Tz>().or_else(|_| {
        err(format!(
            "unknown time zone '{}' — use an IANA name, e.g. 'tb config tz America/Los_Angeles' (or 'tb config tz local')",
            name.trim()
        ))
    })
}

/// The calendar date of the instant `now` (unix seconds) in `tz`, or in the machine's local
/// zone when the board sets none.
pub fn today(now: i64, tz: Option<chrono_tz::Tz>) -> NaiveDate {
    let date = match tz {
        Some(z) => z.timestamp_opt(now, 0).single().map(|t| t.date_naive()),
        None => chrono::Local.timestamp_opt(now, 0).single().map(|t| t.date_naive()),
    };
    // an instant outside chrono's range: fall back to the UTC date, then the epoch
    date.or_else(|| chrono::DateTime::from_timestamp(now, 0).map(|t| t.date_naive())).unwrap_or_default()
}

/// What a card's due date means today. Both fields are None when the card has no due date,
/// when its `due` text is not a `YYYY-MM-DD` date (older free text), and when the card is in
/// `done` — a finished card carries no warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct DueInfo {
    /// Whole calendar days from today to the due date: 0 = due today, negative = past.
    pub days_left: Option<i64>,
    /// `overdue` (past) · `soon` (today up to `due-warn` days ahead) · `ok`.
    pub due_state: Option<&'static str>,
}

/// Everything needed to judge a due date: the board's today and its `due-warn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueCtx {
    pub today: NaiveDate,
    pub warn: i64,
}

impl DueCtx {
    pub fn info(&self, card: &Card) -> DueInfo {
        self.of(card.due.as_deref(), &card.column)
    }

    pub fn of(&self, due: Option<&str>, column: &str) -> DueInfo {
        let Some(date) = due.and_then(parse_date).filter(|_| column != "done") else {
            return DueInfo::default();
        };
        let days = date.signed_duration_since(self.today).num_days();
        let state = if days < 0 {
            "overdue"
        } else if days <= self.warn {
            "soon"
        } else {
            "ok"
        };
        DueInfo { days_left: Some(days), due_state: Some(state) }
    }

    /// `value` with `days_left` and `due_state` added after its own fields (for `--json`).
    pub fn with<'a, T: Serialize>(&self, value: &'a T, card: &Card) -> WithDue<'a, T> {
        let DueInfo { days_left, due_state } = self.info(card);
        WithDue { inner: value, days_left, due_state }
    }
}

/// A card-shaped value plus the two derived due fields. `store::Card` itself is a database
/// row and stays one.
#[derive(Debug, Serialize)]
pub struct WithDue<'a, T: Serialize> {
    #[serde(flatten)]
    pub inner: &'a T,
    pub days_left: Option<i64>,
    pub due_state: Option<&'static str>,
}

impl Store {
    fn due_config(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM config WHERE key=?", [key], |r| r.get(0)).optional()?)
    }

    /// The board's zone; None = the machine's local zone (also for a stored name this
    /// build's zone database does not know).
    pub fn tz(&self) -> Result<Option<chrono_tz::Tz>> {
        Ok(self.due_config("tz")?.and_then(|v| v.parse().ok()))
    }

    /// `America/Los_Angeles` sets the zone; `local` clears it. Returns the value now in force.
    pub fn set_tz(&self, value: &str) -> Result<String> {
        if value.trim().eq_ignore_ascii_case(LOCAL) {
            self.conn.execute("DELETE FROM config WHERE key='tz'", [])?;
            return Ok(LOCAL.to_string());
        }
        let name = parse_tz(value)?.name().to_string();
        self.set_config("tz", &name)?;
        Ok(name)
    }

    /// How many days ahead a due date starts to count as `soon`.
    pub fn due_warn(&self) -> Result<i64> {
        let v = self.due_config("due-warn")?.and_then(|v| v.parse::<i64>().ok());
        Ok(v.filter(|n| (0..=MAX_DUE_WARN).contains(n)).unwrap_or(DEFAULT_DUE_WARN))
    }

    pub fn set_due_warn(&self, n: i64) -> Result<()> {
        if !(0..=MAX_DUE_WARN).contains(&n) {
            return err(format!("due-warn must be 0-{MAX_DUE_WARN} days — try 'tb config due-warn 3'"));
        }
        self.set_config("due-warn", &n.to_string())
    }

    /// The board's today and `due-warn`, read once per command.
    pub fn due_ctx(&self) -> Result<DueCtx> {
        Ok(DueCtx { today: today(now(), self.tz()?), warn: self.due_warn()? })
    }

    /// The due settings this board has SET, for the `tb config` listing. A board that sets
    /// none lists none, so its listing is unchanged; `tb config tz` reads one, default included.
    pub fn due_settings(&self) -> Result<Vec<(String, String)>> {
        let mut v = Vec::new();
        if self.due_config("tz")?.is_some() {
            v.push(("tz".to_string(), self.tz()?.map_or_else(|| LOCAL.to_string(), |z| z.name().to_string())));
        }
        if self.due_config("due-warn")?.is_some() {
            v.push(("due-warn".to_string(), self.due_warn()?.to_string()));
        }
        Ok(v)
    }

    /// Set (`Some`) or clear (`None`) a card's due date. Logged as a `due` event, `OLD -> NEW`.
    /// Setting the date a card already has changes nothing and logs nothing (returns false),
    /// so re-running a bulk re-date is harmless.
    pub fn set_due(&mut self, id: i64, due: Option<&DueDate>, actor: &str) -> Result<bool> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = get_card(&tx, id)?.due;
        let new = due.map(DueDate::as_str);
        if old.as_deref() == new {
            return Ok(false);
        }
        tx.execute("UPDATE cards SET due=? WHERE id=?", params![new, id])?;
        let text = format!("{} -> {}", old.as_deref().unwrap_or(NONE), new.unwrap_or(NONE));
        Self::log(&tx, id, actor, "due", &text)?;
        tx.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        parse_date(s).unwrap_or_else(|| panic!("{s}"))
    }

    fn utc(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp()
    }

    #[test]
    fn only_a_strict_real_date_parses() {
        assert_eq!(parse_date("2026-10-09"), NaiveDate::from_ymd_opt(2026, 10, 9));
        assert_eq!(parse_date("2028-02-29"), NaiveDate::from_ymd_opt(2028, 2, 29), "leap day");
        for bad in [
            "", "2026-02-30", "2027-02-29", "2026-13-01", "2026-00-10", "2026-10-00", "0000-01-01", "2026-1-5",
            "26-10-09", "2026/10/09", "10/09/2026", "2026-10-09 ", " 2026-10-09", "2026-10-09T00:00:00Z", "tomorrow",
            "+2026-10-9", "２０２６-10-09",
        ] {
            assert_eq!(parse_date(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_due_date_keeps_the_text_as_typed_and_none_clears() {
        assert_eq!(DueDate::parse("2026-10-09", "x").unwrap().unwrap().as_str(), "2026-10-09");
        assert_eq!(DueDate::parse(" 2026-10-09 ", "x").unwrap().unwrap().as_str(), "2026-10-09", "shell padding only");
        assert_eq!(DueDate::parse("none", "x").unwrap(), None);
        assert_eq!(DueDate::parse("NONE", "x").unwrap(), None);
        let e = DueDate::parse("2026-02-30", "tb edit 7 --due 2026-10-09").unwrap_err().to_string();
        assert_eq!(
            e,
            "'2026-02-30' is not a real calendar date — use YYYY-MM-DD, e.g. 'tb edit 7 --due 2026-10-09' (or --due none to clear it)"
        );
        let e = DueDate::parse("friday", "tb edit 7 --due 2026-10-09").unwrap_err().to_string();
        assert!(e.starts_with("'friday' is not a date — use YYYY-MM-DD"), "{e}");
        assert!(DueDate::parse("", "x").unwrap_err().to_string().starts_with("the due date is empty — "));
    }

    #[test]
    fn today_is_the_date_in_the_named_zone_not_in_utc() {
        let la: chrono_tz::Tz = "America/Los_Angeles".parse().unwrap();
        let akl: chrono_tz::Tz = "Pacific/Auckland".parse().unwrap();
        let kol: chrono_tz::Tz = "Asia/Kolkata".parse().unwrap();
        // one instant, three calendar dates
        let t = utc("2026-10-09T06:30:00Z");
        assert_eq!(today(t, Some(la)), d("2026-10-08"), "23:30 PDT the day before");
        assert_eq!(today(t, Some(chrono_tz::UTC)), d("2026-10-09"));
        assert_eq!(today(t, Some(akl)), d("2026-10-09"), "19:30 NZDT");
        assert_eq!(today(utc("2026-10-09T18:29:59Z"), Some(kol)), d("2026-10-09"), "23:59:59 IST");
        assert_eq!(today(utc("2026-10-09T18:30:00Z"), Some(kol)), d("2026-10-10"), "00:00:00 IST");
        // local midnight in Los Angeles is 07:00 UTC in summer and 08:00 UTC in winter
        assert_eq!(today(utc("2026-10-09T06:59:59Z"), Some(la)), d("2026-10-08"));
        assert_eq!(today(utc("2026-10-09T07:00:00Z"), Some(la)), d("2026-10-09"));
        assert_eq!(today(utc("2026-12-09T07:59:59Z"), Some(la)), d("2026-12-08"));
        assert_eq!(today(utc("2026-12-09T08:00:00Z"), Some(la)), d("2026-12-09"));
    }

    #[test]
    fn days_left_counts_calendar_days_across_a_dst_change() {
        let la: chrono_tz::Tz = "America/Los_Angeles".parse().unwrap();
        // 2026-03-08 is 23 hours long in Los Angeles. From 23:30 on the 7th to the start of
        // the 9th is 23.5 hours — less than one 24-hour block — but it is two days on a calendar.
        let late_on_the_7th = utc("2026-03-08T07:30:00Z");
        let ctx = DueCtx { today: today(late_on_the_7th, Some(la)), warn: 3 };
        assert_eq!(ctx.today, d("2026-03-07"));
        assert_eq!(ctx.of(Some("2026-03-09"), "todo").days_left, Some(2));
        // 2026-11-01 is 25 hours long: at 23:30 on the 1st a seconds/86400 count from the
        // start of the 31st already says "2 days" — the calendar says 1
        let late_on_the_1st = utc("2026-11-02T07:30:00Z");
        let ctx = DueCtx { today: today(late_on_the_1st, Some(la)), warn: 3 };
        assert_eq!(ctx.today, d("2026-11-01"));
        assert_eq!(ctx.of(Some("2026-10-31"), "todo").days_left, Some(-1));
        assert_eq!(ctx.of(Some("2026-11-02"), "todo").days_left, Some(1));
    }

    #[test]
    fn due_state_boundaries() {
        let ctx = DueCtx { today: d("2026-10-09"), warn: 3 };
        let st = |due: &str| ctx.of(Some(due), "todo");
        assert_eq!(st("2026-10-08"), DueInfo { days_left: Some(-1), due_state: Some("overdue") });
        assert_eq!(st("2026-10-09"), DueInfo { days_left: Some(0), due_state: Some("soon") }, "due today is not overdue");
        assert_eq!(st("2026-10-12"), DueInfo { days_left: Some(3), due_state: Some("soon") });
        assert_eq!(st("2026-10-13"), DueInfo { days_left: Some(4), due_state: Some("ok") });
        let zero = DueCtx { today: d("2026-10-09"), warn: 0 };
        assert_eq!(zero.of(Some("2026-10-09"), "todo").due_state, Some("soon"));
        assert_eq!(zero.of(Some("2026-10-10"), "todo").due_state, Some("ok"));
        // nothing to say: no date, older free text, a finished card
        assert_eq!(ctx.of(None, "todo"), DueInfo::default());
        assert_eq!(ctx.of(Some("Sep 22"), "todo"), DueInfo::default());
        assert_eq!(ctx.of(Some("2026-10-01"), "done"), DueInfo::default());
        assert_eq!(ctx.of(Some("2026-10-01"), "review").due_state, Some("overdue"));
    }
}
