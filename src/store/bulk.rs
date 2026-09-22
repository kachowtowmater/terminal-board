//! A whole plan of rows (`crate::import`) applied in ONE write transaction — or not at all.
//!
//! `BEGIN IMMEDIATE` takes the write lock before the first row, so a second `tb import`
//! running at the same moment waits its turn (the store's busy timeout) and then runs whole:
//! two imports never interleave, and a reader never sees half of one. Any problem that needs
//! the board to find (a row naming a card that does not exist) is collected for EVERY row and
//! the transaction is rolled back; a dry run does the same work and rolls back at the end, so
//! its report is exactly what the real run would do.
//!
//! Each field is written the way its single command writes it, with the same events:
//! `created` (+ `imported`, which says from what and by whom), `edit`, `due`, `blocked` /
//! `unblocked`. Nothing else is ever written: never a column, an owner, a position other than
//! the bottom of TODO, a timestamp or an event taken from the file.

use super::archive::holder_guard;
use super::{bottom_of, get_card, now, parse_title, raw_title, Result, Store};
use crate::import::{Change, Mode, Problem, RowPlan, RowResult};
use rusqlite::{params, TransactionBehavior};
use serde_json::{json, Value};

/// `tag: gh#N title`, the way `raw_title` writes a card back out.
fn compose(tag: Option<&str>, gh: Option<i64>, title: &str) -> String {
    let mut s = String::new();
    if let Some(t) = tag {
        s.push_str(&format!("{t}: "));
    }
    if let Some(n) = gh {
        let token = format!("gh#{n}");
        if !title.split_whitespace().any(|w| w.eq_ignore_ascii_case(&token)) {
            s.push_str(&format!("{token} "));
        }
    }
    s.push_str(title);
    s
}

fn opt(v: &Option<String>) -> Value {
    v.as_ref().map_or(Value::Null, |s| json!(s))
}

impl Store {
    /// Apply every row, or none. `Ok(Err(problems))` = refused, nothing written.
    /// `source` names the file in the `imported` event (its name, never its directory).
    pub fn bulk_apply(
        &mut self,
        mode: Mode,
        plans: &[RowPlan],
        actor: &str,
        source: &str,
        dry_run: bool,
        force: bool,
    ) -> Result<std::result::Result<Vec<RowResult>, Vec<Problem>>> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut results = Vec::new();
        let mut problems = Vec::new();
        for p in plans {
            match mode {
                Mode::Import => results.push(import_row(&tx, p, actor, source)?),
                Mode::Edit => match edit_row(&tx, p, actor, force)? {
                    Ok(r) => results.push(r),
                    Err(problem) => problems.push(problem),
                },
            }
        }
        if !problems.is_empty() {
            return Ok(Err(problems)); // the transaction rolls back as it drops
        }
        if !dry_run {
            tx.commit()?;
        }
        Ok(Ok(results))
    }
}

fn import_row(tx: &rusqlite::Transaction, p: &RowPlan, actor: &str, source: &str) -> Result<RowResult> {
    let typed = p.title.as_deref().unwrap_or_default();
    // the title may carry its own `tag:` / `gh#N`, as `tb add` accepts; a `tag` or `gh_ref`
    // field wins over what the title carries
    let (t_tag, t_gh, t_title) = parse_title(typed);
    let tag = p.tag.clone().unwrap_or(t_tag);
    let gh = p.gh_ref.unwrap_or(t_gh);
    let (tag, gh, title) = parse_title(&compose(tag.as_deref(), gh, &t_title));
    let desc = p.description.clone().unwrap_or_default();
    let t = now();
    let pos = bottom_of(tx, "todo")?;
    tx.execute(
        r#"INSERT INTO cards(title, tag, description, "column", gh_ref, created_at, column_since, position)
           VALUES (?,?,?,'todo',?,?,?,?)"#,
        params![title, tag, desc, gh, t, t, pos],
    )?;
    let id = tx.last_insert_rowid();
    Store::log(tx, id, actor, "created", "")?;
    Store::log(tx, id, actor, "imported", &format!("row {} of {source}", p.row))?;
    let mut changes = vec![Change { field: "title".into(), from: Value::Null, to: json!(title) }];
    if let Some(tag) = &tag {
        changes.push(Change { field: "tag".into(), from: Value::Null, to: json!(tag) });
    }
    if let Some(n) = gh {
        changes.push(Change { field: "gh_ref".into(), from: Value::Null, to: json!(n) });
    }
    if !desc.is_empty() {
        changes.push(Change { field: "description".into(), from: Value::Null, to: json!(desc) });
    }
    if let Some(Some(due)) = &p.due {
        tx.execute("UPDATE cards SET due=? WHERE id=?", params![due.as_str(), id])?;
        Store::log(tx, id, actor, "due", &format!("{} -> {}", super::due::NONE, due.as_str()))?;
        changes.push(Change { field: "due".into(), from: Value::Null, to: json!(due.as_str()) });
    }
    if let Some(list) = &p.checklist {
        for (i, (text, done)) in list.iter().enumerate() {
            tx.execute(
                "INSERT INTO checklist(card_id, idx, text, done) VALUES (?,?,?,?)",
                params![id, i as i64 + 1, text, *done as i64],
            )?;
        }
        if !list.is_empty() {
            let ticked = list.iter().filter(|(_, d)| *d).count();
            let said = if ticked == 0 { format!("{} items", list.len()) } else { format!("{} items ({ticked} ticked)", list.len()) };
            changes.push(Change { field: "checklist".into(), from: Value::Null, to: json!(said) });
        }
    }
    if let Some(Some(by)) = &p.blocked {
        tx.execute("UPDATE cards SET blocked=? WHERE id=?", params![by, id])?;
        Store::log(tx, id, actor, "blocked", &format!("by {by}"))?;
        changes.push(Change { field: "blocked".into(), from: Value::Null, to: json!(by) });
    }
    let title = raw_title(&get_card(tx, id)?);
    Ok(RowResult { row: p.row, id, action: "created", title, changes, ignored: p.ignored.clone() })
}

fn edit_row(
    tx: &rusqlite::Transaction,
    p: &RowPlan,
    actor: &str,
    force: bool,
) -> Result<std::result::Result<RowResult, Problem>> {
    let id = p.id.unwrap_or_default();
    let c = match get_card(tx, id) {
        Ok(c) => c,
        Err(_) => {
            return Ok(Err(Problem {
                row: p.row,
                id: Some(id),
                field: "id".into(),
                problem: format!("no card #{id}"),
                hint: "see 'tb list' for ids".into(),
            }))
        }
    };
    // THE holder rule, the one a single `tb edit` uses: a DOING card somebody else holds is
    // not edited from a file either. A refusal is a row problem, so the WHOLE file is refused
    // and nothing is written — a bulk edit is all or nothing, and "3 rows skipped" is not.
    let forced = match holder_guard(tx, &c, actor, force, "edit it") {
        Ok(forced) => forced,
        Err(e) => {
            // "#3 is held by alice — your cards: … · to edit it anyway use --force (logged)"
            let (what, hint) = e.0.split_once(" — ").unwrap_or((e.0.as_str(), "use --force (logged)"));
            return Ok(Err(Problem {
                row: p.row,
                id: Some(id),
                field: "id".into(),
                problem: what.trim().trim_start_matches(&format!("#{id} ")).to_string(),
                hint: hint.trim().replace("--force", "'tb edit --from FILE --force'"),
            }));
        }
    };
    let mut changes = Vec::new();
    let mut edited = Vec::new();
    // title / tag / gh_ref are one stored triple, written together as `tb edit --title` does.
    // A title without its own `tag:` keeps the card's tag (say "tag": null to drop it); an
    // absent gh_ref keeps the old link, as it does for a single edit.
    if p.title.is_some() || p.tag.is_some() || p.gh_ref.is_some() {
        let (t_tag, t_gh, t_title) = match &p.title {
            Some(t) => parse_title(t),
            None => (None, None, c.title.clone()),
        };
        let tag = p.tag.clone().unwrap_or(t_tag.or_else(|| c.tag.clone()));
        let gh = p.gh_ref.unwrap_or(t_gh.or(c.gh_ref));
        let (tag, gh, title) = parse_title(&compose(tag.as_deref(), gh, &t_title));
        if (&title, &tag, gh) != (&c.title, &c.tag, c.gh_ref) {
            tx.execute("UPDATE cards SET title=?, tag=?, gh_ref=? WHERE id=?", params![title, tag, gh, id])?;
            edited.push("title");
            if title != c.title {
                changes.push(Change { field: "title".into(), from: json!(c.title), to: json!(title) });
            }
            if tag != c.tag {
                changes.push(Change { field: "tag".into(), from: opt(&c.tag), to: opt(&tag) });
            }
            if gh != c.gh_ref {
                changes.push(Change { field: "gh_ref".into(), from: json!(c.gh_ref), to: json!(gh) });
            }
        }
    }
    if let Some(d) = &p.description {
        if *d != c.description {
            tx.execute("UPDATE cards SET description=? WHERE id=?", params![d, id])?;
            edited.push("description");
            changes.push(Change { field: "description".into(), from: json!(c.description), to: json!(d) });
        }
    }
    if !edited.is_empty() {
        Store::log(tx, id, actor, "edit", &format!("{} edited", edited.join(" and ")))?;
    }
    if let Some(due) = &p.due {
        let new = due.as_ref().map(|d| d.as_str());
        if c.due.as_deref() != new {
            tx.execute("UPDATE cards SET due=? WHERE id=?", params![new, id])?;
            let none = super::due::NONE;
            Store::log(tx, id, actor, "due", &format!("{} -> {}", c.due.as_deref().unwrap_or(none), new.unwrap_or(none)))?;
            changes.push(Change { field: "due".into(), from: opt(&c.due), to: json!(new) });
        }
    }
    if let Some(by) = &p.blocked {
        if *by != c.blocked {
            tx.execute("UPDATE cards SET blocked=? WHERE id=?", params![by, id])?;
            match by {
                Some(r) => Store::log(tx, id, actor, "blocked", &format!("by {r}"))?,
                None => Store::log(tx, id, actor, "unblocked", "")?,
            }
            changes.push(Change { field: "blocked".into(), from: opt(&c.blocked), to: opt(by) });
        }
    }
    if let (Some(owner), false) = (&forced, changes.is_empty()) {
        // the same `force` event a single `tb edit --force` writes
        Store::log(tx, id, actor, "force", &format!("edited #{id} held by {owner}"))?;
    }
    let action = if changes.is_empty() { "unchanged" } else { "changed" };
    let title = raw_title(&get_card(tx, id)?);
    Ok(Ok(RowResult { row: p.row, id, action, title, changes, ignored: p.ignored.clone() }))
}
