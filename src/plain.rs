//! Plain-text output for CLI commands and non-TTY bare runs.

use crate::store::{fmt_age, fmt_clock, Card, CardDetail, Snapshot, COLUMNS};
use std::fmt::Write;


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
    let warn = warnings(card).join(" ");
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
    if card.column != "done" {
        parts.push((2, fmt_age(snap.now - card.column_since)));
    }
    if let Some(due) = &card.due {
        parts.push((3, format!("due {due}")));
    }
    if let Some((d, t)) = snap.checks.get(&card.id) {
        parts.push((3, format!("{d}/{t}")));
    }
    let join = |p: &[(u8, String)]| p.iter().map(|x| x.1.as_str()).collect::<Vec<_>>().join(" - ");
    let len = |p: &[(u8, String)]| {
        let b = join(p).chars().count();
        let w = warn.chars().count();
        b + w + usize::from(b > 0 && w > 0)
    };
    while !parts.is_empty() && len(&parts) > width {
        let worst = (0..parts.len()).max_by_key(|&i| parts[i].0).unwrap_or(0);
        parts.remove(worst);
    }
    let base = join(&parts);
    let warn = if base.is_empty() { fit(&warn, width) } else { warn };
    (base, warn)
}

/// `tag - owner - age - x/y  ! warnings`, unfitted (CLI output).
pub fn meta(card: &Card, snap: &Snapshot) -> String {
    let (base, warn) = meta_fit(card, snap, usize::MAX);
    match (base.is_empty(), warn.is_empty()) {
        (_, true) => base,
        (true, false) => warn,
        (false, false) => format!("{base}  {warn}"),
    }
}

/// The problem markers on a card (all shown in red): only `x blocked by ...`.
pub fn warnings(card: &Card) -> Vec<String> {
    match (&card.blocked, card.column.as_str()) {
        (Some(b), c) if c != "done" => vec![format!("x blocked by {b}")],
        _ => Vec::new(),
    }
}

pub fn card_head(card: &Card) -> String {
    match card.gh_ref {
        Some(n) => format!("#{} gh#{n} {}", card.id, card.title),
        None => format!("#{} {}", card.id, card.title),
    }
}

/// The whole board, one column after another (stable, greppable).
pub fn board(snap: &Snapshot) -> String {
    let mut out = String::new();
    let doing = snap.on_board("doing").len();
    let _ = writeln!(
        out,
        "TERMINAL BOARD · {} · {} cards · doing {}/{} · {}",
        if snap.board.is_empty() { "default" } else { &snap.board },
        snap.cards.len(),
        doing,
        snap.wip,
        fmt_clock(snap.now)
    );
    for col in COLUMNS {
        let cards = snap.on_board(col);
        let _ = writeln!(out, "\n{}", column_header(col, cards.len(), snap.wip));
        if cards.is_empty() {
            let _ = writeln!(out, "  -");
        }
        for c in cards {
            let _ = writeln!(out, "  {}", card_head(c));
            let m = meta(c, snap);
            if !m.is_empty() {
                let _ = writeln!(out, "      {m}");
            }
            if c.column == "doing" {
                if let Some(n) = snap.last_note.get(&c.id) {
                    let _ = writeln!(out, "      \"{n}\"");
                }
            }
        }
    }
    if snap.cards.is_empty() {
        let _ = writeln!(out, "\nempty board — add a card with 'tb add \"tag: title\"'");
    }
    out
}

/// One line per card.
pub fn list(snap: &Snapshot) -> String {
    let mut out = String::new();
    for col in COLUMNS {
        for c in snap.in_column(col) {
            let _ = writeln!(out, "{:<7} {}  [{}]", col, card_head(c), meta(c, snap));
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
    let _ = writeln!(out, "{}", card_head(c));
    let mut meta = vec![c.column.clone()];
    if let Some(t) = &c.tag {
        meta.push(t.clone());
    }
    meta.push(c.owner.clone().unwrap_or_else(|| "unowned".into()));
    if let Some(r) = &c.reviewer {
        meta.push(format!("review {r}"));
    }
    meta.push(fmt_age(now - c.column_since));
    if let Some(due) = &c.due {
        meta.push(format!("due {due}"));
    }
    if let Some(b) = &c.blocked {
        meta.push(format!("x blocked by {b}"));
    }
    let _ = writeln!(out, "{}", meta.join(" - "));
    if !c.description.is_empty() {
        let _ = writeln!(out, "\n{}", c.description);
    }
    if !d.checklist.is_empty() {
        let _ = writeln!(out);
        for i in &d.checklist {
            let _ = writeln!(out, "[{}] {} {}", if i.done { "x" } else { " " }, i.idx, i.text);
        }
    }
    if !d.events.is_empty() {
        let _ = writeln!(out);
        for e in &d.events {
            let _ = writeln!(out, "{}", event_line(e));
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
