//! Plain-text output for CLI commands and non-TTY bare runs.

use crate::store::{fmt_age, fmt_clock, Card, CardDetail, Snapshot, COLUMNS};
use crate::text::{sanitize, sanitize_lines};

/// Append one output line; `s` is shown on exactly one line, whatever the stored text holds.
fn line(out: &mut String, s: impl AsRef<str>) {
    out.push_str(&sanitize(s.as_ref()));
    out.push('\n');
}


pub fn column_header(col: &str, n: usize, wip: i64) -> String {
    match col {
        "doing" => format!("DOING ({n}/{wip})"),
        "done" => format!("DONE today ({n})"),
        _ => format!("{} ({n})", col.to_ascii_uppercase()),
    }
}

/// `column_header` for this board: its label instead of the internal name — followed by the
/// name a command accepts, `WITH REVIEWER (2) [review]` — and ` · by due` on a column the
/// board orders by due date. A board with no labels and no `sort due` gets `column_header`.
pub fn column_header_for(snap: &Snapshot, col: &str, n: usize) -> String {
    let mut h = match snap.display.label(col) {
        None => column_header(col, n, snap.wip),
        Some(l) => match col {
            "doing" => format!("{l} ({n}/{}) [{col}]", snap.wip),
            "done" => format!("{l} today ({n}) [{col}]"),
            _ => format!("{l} ({n}) [{col}]"),
        },
    };
    if snap.display.date_ordered(col) {
        h.push_str(" · by due");
    }
    h
}

/// Cut `s` to `w` chars, ending in `…` when cut.
pub fn fit(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    s.chars().take(w - 1).chain(std::iter::once('…')).collect()
}

/// Meta line as (plain part, warnings), fitted to `width` chars when joined with one space.
/// Priority when space runs out: warnings > owner > age > checklist x/y > tag. Lower-priority
/// parts are dropped whole (never cut mid-token); display order stays tag - owner - age - x/y.
pub fn meta_fit(card: &Card, snap: &Snapshot, width: usize) -> (String, String) {
    let f = meta_fit_with(card, snap, width, 0);
    (f.base.clone(), f.warnings())
}

/// The fitted meta line in its parts, for a renderer that styles each one: the plain part,
/// the due mark (`! overdue 3d`, loud; red only when overdue), the other warnings (red) and
/// the quiet marker (dim). Same fitting as `meta_fit_quiet`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetaParts {
    pub base: String,
    pub mark: String,
    pub overdue: bool,
    pub warn: String,
    pub quiet: String,
}

pub fn meta_parts(card: &Card, snap: &Snapshot, width: usize) -> MetaParts {
    let q = quiet(card, snap);
    let overdue = snap.display.due_info(card).due_state == Some("overdue");
    if !q.is_empty() {
        for form in [q.as_str(), "quiet"] {
            let f = meta_fit_with(card, snap, width, form.chars().count());
            if f.whole {
                return MetaParts { base: f.base, mark: f.mark, overdue, warn: f.warn, quiet: form.to_string() };
            }
        }
    }
    let f = meta_fit_with(card, snap, width, 0);
    MetaParts { base: f.base, mark: f.mark, overdue, warn: f.warn, quiet: String::new() }
}

/// The loud due mark of a card that is `soon` or `overdue` (never a DONE card — it has no
/// `due_state`), longest form first. Each shorter form drops whole words, never part of one;
/// the last is a bare `!`, so the mark outlives everything else on a narrow card line.
pub fn due_mark_forms(card: &Card, snap: &Snapshot) -> Vec<String> {
    due_mark_forms_on(card, &snap.display)
}

/// `due_mark_forms` against a board's look on its own (no snapshot) — for `tb show`.
pub fn due_mark_forms_on(card: &Card, look: &crate::store::display::Display) -> Vec<String> {
    let info = look.due_info(card);
    match (info.due_state, info.days_left) {
        (Some("overdue"), Some(d)) => vec![format!("! overdue {}d", -d), format!("! late {}d", -d), "! late".into(), "!".into()],
        (Some("soon"), Some(0)) => vec!["! due today".into(), "! today".into(), "!".into()],
        (Some("soon"), Some(d)) => vec![format!("! due in {d}d"), format!("! due {d}d"), format!("! {d}d"), "!".into()],
        _ => Vec::new(),
    }
}

/// `Oct 9` in the board's current year, the full `2027-01-05` otherwise.
fn short_date(date: chrono::NaiveDate, today: Option<chrono::NaiveDate>) -> String {
    use chrono::Datelike;
    match today {
        Some(t) if t.year() == date.year() => date.format("%b %-d").to_string(),
        _ => date.format("%Y-%m-%d").to_string(),
    }
}

/// The meta line plus the quiet marker as (plain part, warnings, quiet), all inside `width`.
/// The marker is never cut: it is shown whole (`quiet 1h20m`), or without its duration
/// (`quiet`), or not at all. It ranks below the warnings and the owner and above the rest, so
/// tag, checklist and age make room for it first; a line too narrow for owner + `quiet` is
/// fitted exactly as if the card were not quiet.
pub fn meta_fit_quiet(card: &Card, snap: &Snapshot, width: usize) -> (String, String, String) {
    let p = meta_parts(card, snap, width);
    let warn = Fitted { mark: p.mark, warn: p.warn, ..Default::default() }.warnings();
    (p.base, warn, p.quiet)
}

/// One fitted meta line: the plain part, the due mark, the other warnings.
#[derive(Debug, Clone, Default)]
struct Fitted {
    base: String,
    mark: String,
    warn: String,
    /// It all fits and cost neither the owner nor part of a warning.
    whole: bool,
    fits: bool,
}

impl Fitted {
    /// The due mark and the other warnings as the one string older callers expect.
    fn warnings(&self) -> String {
        match (self.mark.is_empty(), self.warn.is_empty()) {
            (true, _) => self.warn.clone(),
            (false, true) => self.mark.clone(),
            (false, false) => format!("{} {}", self.mark, self.warn),
        }
    }
}

/// `meta_fit` with `reserve` chars kept free after the line (plus the space before them). The
/// flag says the reserve really fits and cost neither the owner nor part of a warning.
fn meta_fit_with(card: &Card, snap: &Snapshot, width: usize, reserve: usize) -> Fitted {
    let forms = due_mark_forms(card, snap);
    if forms.is_empty() {
        return meta_fit_once(card, snap, width, reserve, "");
    }
    // the longest form of the mark that keeps the line whole (the owner stays), else the
    // longest that fits at all. When nothing fits — another warning is too long — the mark
    // stays as long as it can while that warning keeps room to be cut, else the bare `!`.
    let tries: Vec<Fitted> = forms.iter().map(|m| meta_fit_once(card, snap, width, reserve, m)).collect();
    let len = |i: usize| forms[i].chars().count();
    let pick = tries
        .iter()
        .position(|f| f.whole)
        .or_else(|| tries.iter().position(|f| f.fits))
        .or_else(|| (0..forms.len()).find(|&i| len(i) + 1 + MIN_CUT <= width))
        .or_else(|| (0..forms.len()).find(|&i| len(i) <= width))
        .unwrap_or(forms.len() - 1);
    tries.into_iter().nth(pick).unwrap_or_default()
}

/// The shortest a cut warning may be (`x bl…`); with less room it is left out.
const MIN_CUT: usize = 5;

/// `meta_fit_with` for one form of the due mark (`""` = the card has none: the line tb
/// always drew).
fn meta_fit_once(card: &Card, snap: &Snapshot, width: usize, reserve: usize, mark: &str) -> Fitted {
    let blocked = warnings(card).join(" ");
    let warn = Fitted { mark: mark.to_string(), warn: blocked.clone(), ..Default::default() }.warnings();
    let shown = &snap.display;
    // under `card-line due` a dated card shows its date where the age is, and the days left
    // while nothing is close (the mark says it once it is)
    let dated = match shown.card_line {
        crate::store::display::CardLine::Due => card.due.as_deref().and_then(crate::store::due::parse_date),
        crate::store::display::CardLine::Age => None,
    };
    // (display order, drop priority: higher = dropped first, text)
    let mut parts: Vec<(u8, String)> = Vec::new();
    if let Some(t) = &card.tag {
        parts.push((4, t.clone()));
    }
    if let Some(o) = &card.owner {
        parts.push((1, o.clone()));
    }
    if let Some(r) = &card.reviewer {
        parts.push((1, format!("review {r}")));
    }
    if let Some(date) = dated {
        parts.push((2, format!("due {}", short_date(date, shown.due.map(|d| d.today)))));
        let info = shown.due_info(card);
        if let (Some("ok"), Some(d)) = (info.due_state, info.days_left) {
            parts.push((2, format!("{d}d")));
        }
    } else {
        if card.column != "done" {
            parts.push((2, fmt_age(snap.now - card.column_since)));
        }
        if let Some(due) = &card.due {
            parts.push((3, format!("due {due}")));
        }
    }
    if let Some((d, t)) = snap.checks.get(&card.id) {
        parts.push((3, format!("{d}/{t}")));
    }
    if let Some(r) = snap.rounds.get(&card.id).filter(|_| card.column != "done") {
        parts.push((2, format!("r{r}")));
    }
    let join = |p: &[(u8, String)]| p.iter().map(|x| x.1.as_str()).collect::<Vec<_>>().join(" - ");
    let len = |p: &[(u8, String)]| {
        let b = join(p).chars().count();
        let w = warn.chars().count();
        let line = b + w + usize::from(b > 0 && w > 0);
        line + if reserve > 0 { reserve + usize::from(line > 0) } else { 0 }
    };
    while !parts.is_empty() && len(&parts) > width {
        let worst = (0..parts.len()).max_by_key(|&i| parts[i].0).unwrap_or(0);
        parts.remove(worst);
    }
    let fits = len(&parts) <= width;
    let whole = fits && (card.owner.is_none() || parts.iter().any(|p| p.0 == 1));
    let base = join(&parts);
    if mark.is_empty() {
        let warn = if base.is_empty() { fit(&warn, width) } else { warn };
        return Fitted { base, mark: String::new(), warn, whole, fits };
    }
    // with a due mark the mark is kept whole; only the other warning is cut to what is left
    let left = width.saturating_sub(mark.chars().count() + 1);
    let blocked = match (base.is_empty() && !fits, left) {
        (false, _) => blocked,
        (true, l) if l < MIN_CUT => String::new(), // no room for even `x bl…`: the mark alone
        (true, _) => fit(&blocked, left),
    };
    Fitted { base, mark: mark.to_string(), warn: blocked, whole, fits }
}

/// `tag - owner - age - x/y  ! warnings  quiet 1h20m`, unfitted (CLI output).
pub fn meta(card: &Card, snap: &Snapshot) -> String {
    let (base, warn) = meta_fit(card, snap, usize::MAX);
    let q = quiet(card, snap);
    let (base, warn) = (base, if warn.is_empty() && q.is_empty() { String::new() } else if q.is_empty() { warn } else if warn.is_empty() { q } else { format!("{warn} {q}") });
    match (base.is_empty(), warn.is_empty()) {
        (_, true) => base,
        (true, false) => warn,
        (false, false) => format!("{base}  {warn}"),
    }
}

/// A DOING card with no event for this long is quietly stale: the meta line says so in the
/// existing warning style. Fixed (documented); no setting.
pub const QUIET_SECS: i64 = 60 * 60;

/// `quiet 1h20m` for a DOING card whose last event is at least QUIET_SECS ago; empty otherwise.
/// Rendered as plain dim text (not red): red is reserved for real problems; the word is the signal.
pub fn quiet(card: &Card, snap: &Snapshot) -> String {
    if card.column != "doing" {
        return String::new();
    }
    match snap.last_event_at.get(&card.id) {
        Some(ts) if snap.now - ts >= QUIET_SECS => format!("quiet {}", fmt_age(snap.now - ts)),
        _ => String::new(),
    }
}

/// The problem markers on a card (all shown in red): only `x blocked by ...`. Quiet work is
/// NOT a problem — it renders as plain dim text (`quiet()`), so red stays reserved.
pub fn warnings(card: &Card) -> Vec<String> {
    match (&card.blocked, card.column.as_str()) {
        (Some(b), c) if c != "done" => vec![format!("x blocked by {b}")],
        _ => Vec::new(),
    }
}

pub fn card_head(card: &Card) -> String {
    match crate::store::shown_ref(card) {
        Some(n) => format!("#{} gh#{n} {}", card.id, card.title),
        None => format!("#{} {}", card.id, card.title),
    }
}

/// The whole board, one column after another (stable, greppable).
pub fn board(snap: &Snapshot) -> String {
    let mut out = String::new();
    let doing = snap.on_board("doing").len();
    line(
        &mut out,
        format!(
            "TERMINAL BOARD · {} · {} cards · doing {}/{} · {}",
            if snap.board.is_empty() { "default" } else { &snap.board },
            snap.cards.len(),
            doing,
            snap.wip,
            fmt_clock(snap.now)
        ),
    );
    for col in COLUMNS {
        let cards = snap.on_board(col);
        out.push('\n');
        line(&mut out, column_header_for(snap, col, cards.len()));
        if cards.is_empty() {
            line(&mut out, "  -");
        }
        for c in cards {
            line(&mut out, format!("  {}", card_head(c)));
            let m = meta(c, snap);
            if !m.is_empty() {
                line(&mut out, format!("      {m}"));
            }
            if c.column == "doing" {
                if let Some(n) = snap.last_note.get(&c.id) {
                    line(&mut out, format!("      \"{n}\""));
                }
            }
        }
    }
    if snap.cards.is_empty() {
        out.push('\n');
        line(&mut out, "empty board — add a card with 'tb add \"tag: title\"'");
    }
    out
}

/// One line per card.
pub fn list(snap: &Snapshot) -> String {
    let mut out = String::new();
    for col in COLUMNS {
        for c in snap.in_column(col) {
            line(&mut out, format!("{:<7} {}  [{}]", col, card_head(c), meta(c, snap)));
        }
    }
    if out.is_empty() {
        out.push_str("no cards — add one with 'tb add \"tag: title\"'\n");
    }
    out
}

pub fn detail(d: &CardDetail, now: i64) -> String {
    detail_on(d, now, &crate::store::display::Display::default())
}

/// `detail` on a board whose look is known: the due mark (`! overdue 3d`) goes on the meta
/// line, next to the block warning, exactly as it does on the board and in `tb list`.
/// A `Display::default()` (no today) marks nothing — what `tb show` printed before.
pub fn detail_on(d: &CardDetail, now: i64, look: &crate::store::display::Display) -> String {
    let c = &d.card;
    let mut out = String::new();
    line(&mut out, card_head(c));
    let mut meta = vec![c.column.clone()];
    if let Some(t) = &c.tag {
        meta.push(t.clone());
    }
    meta.push(c.owner.clone().unwrap_or_else(|| "unowned".into()));
    if let Some(r) = &c.reviewer {
        meta.push(format!("review {r}"));
    }
    meta.push(fmt_age(now - c.column_since));
    if d.round > 1 {
        meta.push(format!("r{}", d.round));
    }
    if let Some(due) = &c.due {
        meta.push(format!("due {due}"));
    }
    // the same loud mark the board and `tb list` show, in its longest form (nothing is
    // fitted here) — never on a DONE card, because it has no `due_state`
    if let Some(mark) = due_mark_forms_on(c, look).first() {
        meta.push(mark.clone());
    }
    if let Some(b) = &c.blocked {
        if c.column != "done" {
            meta.push(format!("x blocked by {b}"));
        }
    }
    line(&mut out, meta.join(" - "));
    if !c.description.is_empty() {
        out.push('\n');
        out.push_str(&sanitize_lines(&c.description));
        out.push('\n');
    }
    if !d.checklist.is_empty() {
        out.push('\n');
        for i in &d.checklist {
            line(&mut out, format!("[{}] {} {}", if i.done { "x" } else { " " }, i.idx, i.text));
        }
    }
    if !d.events.is_empty() {
        out.push('\n');
        for e in &d.events {
            line(&mut out, event_line(e));
        }
    }
    // who the names above were: only when some event on the card carries an identity
    if !d.actors.is_empty() {
        out.push('\n');
        line(&mut out, "actors:");
        for a in &d.actors {
            line(&mut out, format!("  {}", a.line()));
        }
    }
    out
}

pub fn event_line(e: &crate::store::Event) -> String {
    let t = fmt_clock(e.ts);
    match e.kind.as_str() {
        "note" => format!("{t} {}: {}", e.actor, e.text),
        "taken" => format!("{t} taken by {}", e.actor),
        "created" => format!("{t} added by {}", e.actor),
        _ if e.text.is_empty() => format!("{t} {} {}", e.kind, e.actor),
        _ => format!("{t} {} {}: {}", e.actor, e.kind, e.text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::display::{CardLine, Display};
    use crate::store::due::DueCtx;

    fn card(due: Option<&str>, column: &str) -> Card {
        Card {
            id: 7,
            title: "send the renewal".into(),
            tag: Some("permits".into()),
            description: String::new(),
            column: column.into(),
            owner: Some("alice".into()),
            due: due.map(str::to_string),
            gh_ref: None,
            created_at: 0,
            column_since: 1_000_000 - 2 * 86400,
            blocked: None,
            position: 0,
            reviewer: None,
        }
    }

    fn snap(card_line: CardLine) -> Snapshot {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 9);
        let display = Display { card_line, due: today.map(|today| DueCtx { today, warn: 3 }), ..Default::default() };
        let mut s = Snapshot { now: 1_000_000, wip: 3, display, ..Default::default() };
        s.checks.insert(7, (1, 4));
        s
    }

    /// The layout FUNCTION, snapshotted: one card line at every width it can be given.
    fn table(c: &Card, s: &Snapshot, widths: &[usize]) -> Vec<String> {
        widths
            .iter()
            .map(|w| {
                let (base, warn) = meta_fit(c, s, *w);
                let line = [base, warn].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ");
                assert!(line.chars().count() <= *w, "width {w}: {line:?} is {} wide", line.chars().count());
                line
            })
            .collect()
    }

    #[test]
    fn without_a_today_or_a_date_the_line_is_the_one_tb_always_drew() {
        let mut s = snap(CardLine::Age);
        s.display = Display::default();
        assert_eq!(meta(&card(Some("2026-10-05"), "todo"), &s), "permits - alice - 2d - due 2026-10-05 - 1/4");
        assert_eq!(meta(&card(None, "todo"), &snap(CardLine::Due)), "permits - alice - 2d - 1/4", "undated: its age");
        assert!(due_mark_forms(&card(Some("2026-10-05"), "todo"), &s).is_empty());
    }

    #[test]
    fn the_mark_is_loud_for_soon_and_overdue_and_never_on_done() {
        let s = snap(CardLine::Age);
        assert_eq!(meta(&card(Some("2026-10-06"), "todo"), &s), "permits - alice - 2d - due 2026-10-06 - 1/4  ! overdue 3d");
        assert_eq!(meta(&card(Some("2026-10-09"), "doing"), &s), "permits - alice - 2d - due 2026-10-09 - 1/4  ! due today");
        assert_eq!(meta(&card(Some("2026-10-12"), "review"), &s), "permits - alice - 2d - due 2026-10-12 - 1/4  ! due in 3d");
        assert_eq!(meta(&card(Some("2026-10-13"), "todo"), &s), "permits - alice - 2d - due 2026-10-13 - 1/4", "ok: no mark");
        assert_eq!(meta(&card(Some("2026-10-06"), "done"), &s), "permits - alice - due 2026-10-06 - 1/4", "done: never");
        let mut blocked = card(Some("2026-10-06"), "todo");
        blocked.blocked = Some("#3".into());
        assert_eq!(meta(&blocked, &s), "permits - alice - 2d - due 2026-10-06 - 1/4  ! overdue 3d x blocked by #3");
    }

    #[test]
    fn card_line_due_shows_the_date_and_the_days_left_where_the_age_was() {
        let s = snap(CardLine::Due);
        assert_eq!(meta(&card(Some("2026-10-27"), "todo"), &s), "permits - alice - due Oct 27 - 18d - 1/4");
        assert_eq!(meta(&card(Some("2027-01-05"), "todo"), &s), "permits - alice - due 2027-01-05 - 88d - 1/4", "another year: in full");
        assert_eq!(meta(&card(Some("2026-10-11"), "todo"), &s), "permits - alice - due Oct 11 - 1/4  ! due in 2d", "close: the mark counts the days");
        assert_eq!(meta(&card(Some("2026-10-01"), "done"), &s), "permits - alice - due Oct 1 - 1/4");
        assert_eq!(meta(&card(Some("Sep 22"), "todo"), &s), "permits - alice - 2d - due Sep 22 - 1/4", "free text: as before");
    }

    #[test]
    fn a_narrow_line_gives_up_whole_parts_and_the_mark_outlives_the_age() {
        let s = snap(CardLine::Age);
        let c = card(Some("2026-10-06"), "todo");
        let widths = [60, 48, 40, 30, 24, 20, 18, 14, 12, 8, 6, 3, 1];
        assert_eq!(
            table(&c, &s, &widths),
            [
                "permits - alice - 2d - due 2026-10-06 - 1/4 ! overdue 3d",
                "alice - 2d - due 2026-10-06 - 1/4 ! overdue 3d", // the tag goes first, as it always did
                "alice - 2d - due 2026-10-06 ! overdue 3d",
                "alice - 2d ! overdue 3d",
                "alice - 2d ! overdue 3d",
                "alice ! overdue 3d",
                "alice ! overdue 3d",
                "alice ! late",
                "alice ! late",
                "alice !",
                "! late",
                "!",
                "!",
            ]
        );
        // under card-line due the date goes the way the age does; the mark is still there
        let s = snap(CardLine::Due);
        assert_eq!(
            table(&c, &s, &[40, 30, 22, 18, 12, 9, 5]),
            ["alice - due Oct 6 - 1/4 ! overdue 3d", "alice - due Oct 6 ! overdue 3d", "alice ! overdue 3d", "alice ! overdue 3d", "alice ! late", "alice !", "!"]
        );
        // every form of every mark is made of whole words
        for due in ["2026-10-06", "2026-10-09", "2026-10-11"] {
            let forms = due_mark_forms(&card(Some(due), "todo"), &s);
            assert_eq!(forms.last().map(String::as_str), Some("!"));
            for pair in forms.windows(2) {
                assert!(pair[1].chars().count() < pair[0].chars().count(), "each form is shorter than the last: {pair:?}");
            }
        }
    }

    #[test]
    fn a_blocked_overdue_card_keeps_its_mark_whole_and_cuts_only_the_other_warning() {
        let s = snap(CardLine::Age);
        let mut c = card(Some("2026-10-06"), "todo");
        c.blocked = Some("the signed copy from the other side".into());
        for w in 1..70 {
            let p = meta_parts(&c, &s, w);
            assert!(due_mark_forms(&c, &s).contains(&p.mark), "width {w}: the mark {:?} is not one of its whole forms", p.mark);
            let line = [p.base.as_str(), p.mark.as_str(), p.warn.as_str()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ");
            assert!(line.chars().count() <= w, "width {w}: {line:?}");
            assert!(p.warn.is_empty() || p.warn.starts_with("x b"), "width {w}: {:?}", p.warn);
        }
        assert_eq!(meta_parts(&c, &s, 69).warn, "x blocked by the signed copy from the other side");
        let p = meta_parts(&c, &s, 30);
        assert_eq!((p.base.as_str(), p.mark.as_str(), p.warn.as_str()), ("", "! overdue 3d", "x blocked by the…"));
        let p = meta_parts(&c, &s, 12);
        assert_eq!((p.mark.as_str(), p.warn.as_str()), ("! late", "x bl…"));
        assert_eq!(meta_parts(&c, &s, 4).mark, "!");
    }

    #[test]
    fn labelled_headers_name_the_column_to_type() {
        let mut s = snap(CardLine::Age);
        assert_eq!(column_header_for(&s, "review", 2), "REVIEW (2)");
        assert_eq!(column_header_for(&s, "doing", 1), "DOING (1/3)");
        assert_eq!(column_header_for(&s, "done", 4), "DONE today (4)");
        s.display.labels = [Some("INTAKE".into()), None, Some("WITH REVIEWER".into()), Some("FILED".into())];
        s.display.by_due = true;
        assert_eq!(column_header_for(&s, "todo", 5), "INTAKE (5) [todo] · by due");
        assert_eq!(column_header_for(&s, "doing", 1), "DOING (1/3)");
        assert_eq!(column_header_for(&s, "review", 2), "WITH REVIEWER (2) [review] · by due");
        assert_eq!(column_header_for(&s, "done", 4), "FILED today (4) [done]");
    }
}
