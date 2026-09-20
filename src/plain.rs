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
    let (base, warn, _) = meta_fit_with(card, snap, width, 0);
    (base, warn)
}

/// The meta line plus the quiet marker as (plain part, warnings, quiet), all inside `width`.
/// The marker is never cut: it is shown whole (`quiet 1h20m`), or without its duration
/// (`quiet`), or not at all. It ranks below the warnings and the owner and above the rest, so
/// tag, checklist and age make room for it first; a line too narrow for owner + `quiet` is
/// fitted exactly as if the card were not quiet.
pub fn meta_fit_quiet(card: &Card, snap: &Snapshot, width: usize) -> (String, String, String) {
    let q = quiet(card, snap);
    if !q.is_empty() {
        for form in [q.as_str(), "quiet"] {
            let (base, warn, whole) = meta_fit_with(card, snap, width, form.chars().count());
            if whole {
                return (base, warn, form.to_string());
            }
        }
    }
    let (base, warn) = meta_fit(card, snap, width);
    (base, warn, String::new())
}

/// `meta_fit` with `reserve` chars kept free after the line (plus the space before them). The
/// flag says the reserve really fits and cost neither the owner nor part of a warning.
fn meta_fit_with(card: &Card, snap: &Snapshot, width: usize, reserve: usize) -> (String, String, bool) {
    let warn = warnings(card).join(" ");
    // (display order, drop priority: higher = dropped first, text)
    let mut parts: Vec<(u8, String)> = Vec::new();
    if let Some(t) = &card.tag {
        parts.push((4, t.clone()));
    }
    if let Some(o) = &card.owner {
        parts.push((1, o.clone()));
    }
    if card.column != "done" {
        parts.push((2, fmt_age(snap.now - card.column_since)));
    }
    if let Some(due) = &card.due {
        parts.push((3, format!("due {due}")));
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
    let whole = len(&parts) <= width && (card.owner.is_none() || parts.iter().any(|p| p.0 == 1));
    let base = join(&parts);
    let warn = if base.is_empty() { fit(&warn, width) } else { warn };
    (base, warn, whole)
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
        line(&mut out, column_header(col, cards.len(), snap.wip));
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
    let c = &d.card;
    let mut out = String::new();
    line(&mut out, card_head(c));
    let mut meta = vec![c.column.clone()];
    if let Some(t) = &c.tag {
        meta.push(t.clone());
    }
    meta.push(c.owner.clone().unwrap_or_else(|| "unowned".into()));
    meta.push(fmt_age(now - c.column_since));
    if d.round > 1 {
        meta.push(format!("r{}", d.round));
    }
    if let Some(due) = &c.due {
        meta.push(format!("due {due}"));
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
