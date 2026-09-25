//! The board on its way OUT, for someone who will never run `tb`: `tb export --json`,
//! `tb export --csv`, `tb log` and `tb list --done --since`.
//!
//! Two readers, two shapes. **JSON** is the card object `tb show --json` already emits, with
//! EVERY event rather than the last ten, wrapped in `{v, board, exported_at, cards: […]}` —
//! `cards` is exactly what `tb import` and `tb edit --from` accept, so a board can be exported,
//! edited and fed back (there is a test for that round trip, because it is the reason both
//! exist). **CSV** is for a person opening the file in a spreadsheet, and is one-way.
//!
//! Both JSON streams go through `clean_json`, the cleaner every `--json` path uses: an export
//! is a document somebody pipes, so it never carries a control byte a terminal could act on.
//! Line breaks and tabs are text and survive, which is what keeps the round trip exact.
//!
//! Nothing here writes. Every query is a read, the board file is never opened for writing, and
//! a test checksums the file before and after.
//!
//! **Streaming.** A board with 10,000 cards and 100,000 events must not be built in memory
//! first: every row is written to the output as it is read, and only one card is held at a
//! time. The output is a `&mut dyn Write`, so the same code fills a test's `Vec<u8>` and the
//! real stdout.

use crate::contract::{self, CardJ};
use crate::store::{Card, Code, Result, Store};
use std::io::Write;

/// What `tb export` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Re-importable: the `tb show --json` card shape, every event, one JSON document.
    Json,
    /// For a spreadsheet: one row per card, or with `history` one row per event.
    Csv,
}

/// A write to the output. A reader that went away (`tb export --csv | head`) ends tb quietly
/// with exit 0 — the same rule every other `tb` command follows (see `write_stdout`); any
/// other write error is a refusal that says what to do.
fn wrote(r: std::io::Result<()>) -> Result<()> {
    match r {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => Err(crate::store::BoardError(format!(
            "could not write the export: {e} — check there is room on the disk, or write it to a file"
        ), Code::IoError)),
    }
}

/// Excel and LibreOffice treat a cell that begins `=`, `+`, `-` or `@` as a FORMULA, and a
/// card title is text somebody else typed. A leading apostrophe makes the spreadsheet show
/// the text and evaluate nothing; tab and carriage return lead the same way, so they get it
/// too. The stored card is untouched — this is a property of the CSV file, not of the board.
fn csv_guard(s: &str) -> std::borrow::Cow<'_, str> {
    match s.chars().next() {
        Some('=') | Some('+') | Some('-') | Some('@') | Some('\t') | Some('\r') => {
            std::borrow::Cow::Owned(format!("'{s}"))
        }
        _ => std::borrow::Cow::Borrowed(s),
    }
}

/// One RFC 4180 cell: `"` doubled, and anything holding a comma, a quote or a line break
/// wrapped in quotes — so a description with newlines stays ONE cell in the spreadsheet.
pub fn csv_cell(s: &str) -> String {
    let s = csv_guard(s);
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.into_owned()
    }
}

fn csv_row(out: &mut dyn Write, cells: &[String]) -> Result<()> {
    let line = cells.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(",");
    // CRLF: what RFC 4180 says, and what Excel on Windows expects
    wrote(out.write_all(line.as_bytes()).and_then(|()| out.write_all(b"\r\n")))
}

/// A unix second as a local `YYYY-MM-DD HH:MM` in the board's zone — a person reads dates in
/// their own day, not in UTC (the same zone `--due` and `--since` use).
pub fn local_time(ts: i64, tz: Option<chrono_tz::Tz>) -> String {
    use chrono::TimeZone;
    match tz {
        Some(z) => z.timestamp_opt(ts, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string()),
        None => chrono::Local.timestamp_opt(ts, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string()),
    }
    .unwrap_or_default()
}

fn num(v: Option<i64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_default()
}

fn text(v: &Option<String>) -> String {
    v.clone().unwrap_or_default()
}

/// The columns of `tb export --csv`. Named for a person, not for the database.
const CARD_COLUMNS: [&str; 23] = [
    "id", "column", "column label", "title", "tag", "github", "owner", "reviewer", "due",
    "days left", "due state", "blocked", "waiting on", "look again", "round", "position",
    "created", "in column since", "last activity", "checklist done", "checklist total",
    "checklist", "description",
];

/// The columns of `tb export --csv --history`.
const EVENT_COLUMNS: [&str; 6] = ["when", "card", "card title", "who", "what", "detail"];

/// A card's checklist as one cell: `[x] draft` per line, so a spreadsheet shows it whole.
fn checklist_cell(c: &CardJ) -> String {
    c.checklist.iter().map(|i| format!("[{}] {}", if i.done { "x" } else { " " }, i.text)).collect::<Vec<_>>().join("\n")
}

/// `tb export`. Streams: one card (or one event) is held at a time, whatever the board's size.
pub fn export(store: &Store, out: &mut dyn Write, format: Format, history: bool) -> Result<()> {
    let tz = store.tz()?;
    match format {
        Format::Csv if history => events_csv(store, out, tz),
        Format::Csv => cards_csv(store, out, tz),
        Format::Json => json(store, out),
    }
}

fn cards_csv(store: &Store, out: &mut dyn Write, tz: Option<chrono_tz::Tz>) -> Result<()> {
    // a byte-order mark: without it Excel reads a UTF-8 file as the local code page and every
    // accent, dash and non-Latin name comes out mangled. Readers that do not want one use
    // `--json`, which never has it.
    wrote(out.write_all("\u{feff}".as_bytes()))?;
    csv_row(out, &CARD_COLUMNS.iter().map(|s| s.to_string()).collect::<Vec<_>>())?;
    let (due, display, blocks) = (store.due_ctx()?, store.display()?, store.block_ctx()?);
    for card in store.list()? {
        let c = contract::card_with(store, &card, &due, &display, &blocks)?;
        let done = c.checklist.iter().filter(|i| i.done).count();
        csv_row(
            out,
            &[
                c.id.to_string(),
                c.column.clone(),
                c.column_label.clone(),
                c.title.clone(),
                text(&c.tag),
                num(c.gh_ref),
                text(&c.owner),
                text(&c.reviewer),
                text(&c.due),
                num(c.days_left),
                c.due_state.unwrap_or_default().to_string(),
                text(&c.blocked),
                text(&c.blocked_on),
                text(&c.blocked_until),
                c.round.to_string(),
                c.position.to_string(),
                local_time(c.created_at, tz),
                local_time(c.column_since, tz),
                local_time(c.last_event_at, tz),
                done.to_string(),
                c.checklist.len().to_string(),
                checklist_cell(&c),
                c.description.clone(),
            ],
        )?;
    }
    wrote(out.flush())
}

fn events_csv(store: &Store, out: &mut dyn Write, tz: Option<chrono_tz::Tz>) -> Result<()> {
    wrote(out.write_all("\u{feff}".as_bytes()))?;
    csv_row(out, &EVENT_COLUMNS.iter().map(|s| s.to_string()).collect::<Vec<_>>())?;
    let titles = store.card_titles()?;
    store.for_each_event(0, &mut |e| {
        let title = titles.get(&e.card_id).cloned().unwrap_or_default();
        csv_row(
            out,
            &[
                local_time(e.ts, tz),
                format!("#{}", e.card_id),
                title,
                e.actor.clone(),
                e.kind.clone(),
                e.text.clone(),
            ],
        )
    })?;
    wrote(out.flush())
}

/// `{v, board, exported_at, cards: […]}` — `cards` is the array `tb import` and
/// `tb edit --from` read, and each card is the `tb show --json` object with every event.
fn json(store: &Store, out: &mut dyn Write) -> Result<()> {
    let (due, display, blocks) = (store.due_ctx()?, store.display()?, store.block_ctx()?);
    let head = serde_json::json!({
        "v": contract::SCHEMA_VERSION,
        "board": store.name,
        "exported_at": crate::store::now(),
        "tz": store.tz()?.map(|z| z.name().to_string()),
    });
    let head = serde_json::to_string_pretty(&crate::clean_json(&head)).unwrap_or_else(|_| "{}".into());
    // open the object by hand so the cards can be streamed into it one at a time
    let head = head.trim_end().trim_end_matches('}').trim_end();
    wrote(out.write_all(head.as_bytes()))?;
    wrote(out.write_all(b",\n  \"cards\": [\n"))?;
    let mut first = true;
    for card in store.list()? {
        let mut c = contract::card_with(store, &card, &due, &display, &blocks)?;
        // the WHOLE history, not the last ten a live reader gets
        c.events = store.all_events_of(card.id)?;
        let text = serde_json::to_string_pretty(&crate::clean_json(&c)).unwrap_or_else(|_| "null".into());
        let indented: String = text.lines().map(|l| format!("    {l}\n")).collect();
        wrote(out.write_all(if first { b"" } else { b",\n" }))?;
        first = false;
        wrote(out.write_all(indented.trim_end_matches('\n').as_bytes()))?;
    }
    wrote(out.write_all(b"\n  ]\n}\n"))?;
    wrote(out.flush())
}

/// `tb log`: the board's whole history, oldest first, from `since` (unix seconds, 0 = all).
/// Streams the same way `export` does. Card events are interleaved with the board's own log
/// (a move's `moved-out` on the board a card left, a WIP change, …) — `card_id` (JSON) or the
/// id column (plain text) marks each row as one or the other; see `Store::for_each_log_event`
/// and #106.
pub fn log(store: &Store, out: &mut dyn Write, since: i64, json_out: bool) -> Result<()> {
    use crate::store::LogEvent;
    let tz = store.tz()?;
    // the identity behind each event's `actor_id` (the trace: who, in which harness, model,
    // role, session and machine) — read once per id, however many events share it
    let mut known: std::collections::HashMap<i64, Option<crate::store::actors::Actor>> = std::collections::HashMap::new();
    let mut identity = |id: Option<i64>| -> Option<crate::store::actors::Actor> {
        let id = id?;
        known.entry(id).or_insert_with(|| store.actor_by_id(id).ok().flatten()).clone()
    };
    if json_out {
        wrote(out.write_all(b"[\n"))?;
        let mut first = true;
        store.for_each_log_event(since, &mut |e| {
            let who = identity(match &e {
                LogEvent::Card(e) => e.actor_id,
                LogEvent::Board { actor_id, .. } => *actor_id,
            });
            let mut line = match &e {
                LogEvent::Card(e) => serde_json::json!({
                    "v": contract::SCHEMA_VERSION,
                    "ts": e.ts,
                    "card_id": e.card_id,
                    "actor": e.actor,
                    "actor_id": e.actor_id,
                    "kind": e.kind,
                    "text": e.text,
                    "ancestry": e.ancestry,
                }),
                LogEvent::Board { ts, actor, kind, text, actor_id, ancestry } => serde_json::json!({
                    "v": contract::SCHEMA_VERSION,
                    "ts": ts,
                    "card_id": null,
                    "actor": actor,
                    "actor_id": actor_id,
                    "kind": kind,
                    "text": text,
                    "ancestry": ancestry,
                }),
            };
            // additive: the whole identity inline, as `tb watch --events` carries it (a log
            // is a flat list with no `actors[]` to look an id up in); null when none is known
            line["identity"] = serde_json::to_value(&who).unwrap_or(serde_json::Value::Null);
            let text = serde_json::to_string(&crate::clean_json(&line)).unwrap_or_else(|_| "null".into());
            let r = out.write_all(if first { b"  " } else { b",\n  " }).and_then(|()| out.write_all(text.as_bytes()));
            first = false;
            wrote(r)
        })?;
        wrote(out.write_all(b"\n]\n"))?;
        return wrote(out.flush());
    }
    let titles = store.card_titles()?;
    let mut any = false;
    store.for_each_log_event(since, &mut |e| {
        any = true;
        let (ts, id_col, actor, kind, tail) = match &e {
            LogEvent::Card(e) => {
                let title = titles.get(&e.card_id).cloned().unwrap_or_default();
                let mut detail = if e.text.is_empty() { String::new() } else { format!(": {}", e.text) };
                // a move into DONE carries the identity behind the name — the trace of who
                // closed the card (store/verifier.rs); other events keep their one short line
                if e.kind == "moved" && e.text.ends_with("-> done") {
                    if let Some(a) = identity(e.actor_id) {
                        detail.push_str(&format!(" (by {})", a.line()));
                    }
                }
                (e.ts, format!("#{}", e.card_id), e.actor.as_str(), e.kind.as_str(), format!("{}{}", crate::plain::fit(&title, 28), detail))
            }
            LogEvent::Board { ts, actor, kind, text, .. } => (*ts, "board".to_string(), actor.as_str(), kind.as_str(), text.clone()),
        };
        let line = crate::text::sanitize(&format!(
            "{}  {:<5} {:<10} {:<9} {}",
            local_time(ts, tz),
            id_col,
            crate::plain::fit(actor, 10),
            kind,
            tail
        ));
        wrote(out.write_all(line.as_bytes()).and_then(|()| out.write_all(b"\n")))
    })?;
    if !any {
        wrote(out.write_all(b"no history yet\n"))?;
    }
    wrote(out.flush())
}

/// The cards `tb list --done [--since DATE]` shows: DONE, finished at/after `since`
/// (unix seconds), newest first — the same order the board's DONE column uses.
pub fn done_since(store: &Store, since: i64) -> Result<Vec<Card>> {
    let mut v: Vec<Card> = store.list()?.into_iter().filter(|c| c.column == "done" && c.column_since >= since).collect();
    v.sort_by(|a, b| b.column_since.cmp(&a.column_since).then(a.id.cmp(&b.id)));
    Ok(v)
}

/// A `--since` value as a unix second: a calendar DATE is local midnight in the board's zone
/// (so a day boundary is the person's day, never a UTC instant), and a plain number is the
/// unix second `tb watch --since` already takes.
pub fn since_value(raw: &str, tz: Option<chrono_tz::Tz>, what: &str) -> Result<i64> {
    use chrono::TimeZone;
    let t = raw.trim();
    if let Ok(n) = t.parse::<i64>() {
        if n >= 0 && t.len() != 8 {
            return Ok(n);
        }
    }
    let Some(date) = crate::store::due::parse_date(t) else {
        return crate::store::err(format!(
            "'{t}' is not a date — use YYYY-MM-DD, e.g. '{what} 2026-10-09' (or a unix second)"
        ), Code::InvalidValue);
    };
    let midnight = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    let ts = match tz {
        Some(z) => z.from_local_datetime(&midnight).earliest().map(|t| t.timestamp()),
        None => chrono::Local.from_local_datetime(&midnight).earliest().map(|t| t.timestamp()),
    };
    // a midnight that does not exist locally (a spring-forward zone) starts at the next
    // instant that does, never at UTC midnight
    Ok(ts.unwrap_or_else(|| midnight.and_utc().timestamp()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_that_would_be_a_formula_is_neutralised() {
        for bad in ["=1+1", "+1", "-1", "@SUM(A1)", "\tx", "\rx"] {
            let cell = csv_cell(bad);
            assert!(cell.starts_with('\'') || cell.starts_with("\"'"), "{bad} -> {cell}");
        }
        // and a title that merely CONTAINS one is left alone
        assert_eq!(csv_cell("a = b"), "a = b");
        assert_eq!(csv_cell("2026-10-09"), "2026-10-09");
    }

    #[test]
    fn quoting_follows_rfc_4180() {
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_cell("one\ntwo"), "\"one\ntwo\"");
        assert_eq!(csv_cell("crlf\r\nhere"), "\"crlf\r\nhere\"");
        // a formula guard inside a quoted cell keeps both
        assert_eq!(csv_cell("=a,b"), "\"'=a,b\"");
    }

    #[test]
    fn a_since_date_is_local_midnight_not_utc() {
        let la: chrono_tz::Tz = "America/Los_Angeles".parse().unwrap();
        let ts = since_value("2026-10-09", Some(la), "tb log --since").unwrap();
        // 2026-10-09 00:00 in Los Angeles is 07:00 UTC that day
        assert_eq!(chrono::DateTime::from_timestamp(ts, 0).unwrap().format("%Y-%m-%dT%H:%MZ").to_string(), "2026-10-09T07:00Z");
        // a unix second passes through
        assert_eq!(since_value("1789763036", Some(la), "x").unwrap(), 1789763036);
        let e = since_value("09/10/2026", Some(la), "tb log --since").unwrap_err().0;
        assert!(e.contains("is not a date") && e.contains("tb log --since 2026-10-09"), "{e}");
    }
}
