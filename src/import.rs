//! Many cards from one file: `tb import FILE.json|-` (create) and `tb edit --from FILE.json|-`
//! (change existing cards, keyed by `id`).
//!
//! The row shape is the card object `tb show --json` and `tb board --json` already emit
//! (docs/JSON.md), so a board can be exported, edited and fed back. This module reads the
//! document and turns every row into a plan or into PROBLEMS — each naming its row, its card
//! id and its field, so a person fixing a sixty-row file never has to guess. It touches no
//! database: `store::bulk` applies a whole plan in one transaction, or nothing.
//!
//! Never forged: a row's `id` (on import), `column`, `owner`, `reviewer`, `position`, its
//! timestamps and its `events` are IGNORED, with a warning — imported cards land in TODO and
//! their history starts with the import. Unknown fields are ignored with a warning too; that
//! is what keeps tomorrow's export importable today.

use crate::store::due::DueDate;
use crate::store::BoardError;
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `tb import`: every row is a new card.
    Import,
    /// `tb edit --from`: every row changes the card its `id` names.
    Edit,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Import => "import",
            Mode::Edit => "edit",
        }
    }
    /// The command as typed, up to the file: `import` / `edit --from`.
    pub fn usage(self) -> &'static str {
        match self {
            Mode::Import => "import",
            Mode::Edit => "edit --from",
        }
    }
}

/// One thing wrong with one row. `row` counts from 1, as a person counts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Problem {
    pub row: usize,
    /// The card the row names (`edit --from`), when it names one.
    pub id: Option<i64>,
    pub field: String,
    pub problem: String,
    pub hint: String,
}

impl Problem {
    /// `row 250 (#41) due: '2026-02-30' is not a real calendar date — use YYYY-MM-DD`
    pub fn line(&self) -> String {
        let id = self.id.map(|i| format!(" (#{i})")).unwrap_or_default();
        format!("row {}{id} {}: {} — {}", self.row, self.field, self.problem, self.hint)
    }
}

/// What one row asks for. `None` = the field is absent (left alone by `edit --from`, default
/// on import); `Some(None)` = present and null (clear it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowPlan {
    pub row: usize,
    pub id: Option<i64>,
    /// The title as written; it may carry its own `tag:` and `gh#N`, as `tb add` accepts.
    pub title: Option<String>,
    pub tag: Option<Option<String>>,
    pub gh_ref: Option<Option<i64>>,
    pub description: Option<String>,
    pub due: Option<Option<DueDate>>,
    pub blocked: Option<Option<String>>,
    /// (text, done) — import only.
    pub checklist: Option<Vec<(String, bool)>>,
    /// Fields present in the row that this command does not apply (alphabetical).
    pub ignored: Vec<String>,
}

/// The fields a row may set. Everything else in a row is ignored with a warning.
const IMPORT_FIELDS: [&str; 7] = ["title", "tag", "gh_ref", "description", "due", "blocked", "checklist"];
const EDIT_FIELDS: [&str; 7] = ["id", "title", "tag", "gh_ref", "description", "due", "blocked"];

/// The rows of a document: a JSON array of cards, `{"cards": […]}`, a whole board as
/// `tb board --json` prints it (`columns.todo|doing|review|done`, in that order), or one
/// card object as `tb show --json` prints it.
pub fn rows_of(text: &str, mode: Mode, source: &str) -> Result<Vec<Value>, BoardError> {
    let again = format!("'tb {} {source} --dry-run'", mode.usage());
    let doc: Value = serde_json::from_str(text)
        .map_err(|e| BoardError(format!("{source} is not valid JSON ({e}) — fix it, then check it with {again}")))?;
    let shape = || {
        BoardError(format!(
            "{source} is not a list of cards — give a JSON array of card objects, {{\"cards\": [...]}}, or the output of 'tb board --json' / 'tb show ID --json'; then check it with {again}"
        ))
    };
    let rows = match doc {
        Value::Array(rows) => rows,
        Value::Object(mut o) => match (o.remove("cards"), o.remove("columns")) {
            (Some(Value::Array(rows)), _) => rows,
            (Some(_), _) => return Err(shape()),
            (None, Some(Value::Object(mut cols))) => {
                let mut rows = Vec::new();
                for c in crate::store::COLUMNS {
                    match cols.remove(c) {
                        Some(Value::Array(cards)) => rows.extend(cards),
                        None => {}
                        Some(_) => return Err(shape()),
                    }
                }
                rows
            }
            (None, Some(_)) => return Err(shape()),
            (None, None) if o.contains_key("title") || o.contains_key("id") => vec![Value::Object(o)],
            (None, None) => return Err(shape()),
        },
        _ => return Err(shape()),
    };
    if rows.is_empty() {
        return Err(BoardError(format!("{source} holds no cards — nothing to do; a row looks like {}", example(mode))));
    }
    Ok(rows)
}

fn example(mode: Mode) -> &'static str {
    match mode {
        Mode::Import => r#"{"title": "tag: what to do", "due": "2026-10-09"}"#,
        Mode::Edit => r#"{"id": 7, "due": "2026-10-09"}"#,
    }
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "true/false",
        Value::Number(_) => "a number",
        Value::String(_) => "text",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// Every row as a plan, and every problem found on the way (all of them, not just the first).
pub fn plan(rows: &[Value], mode: Mode) -> (Vec<RowPlan>, Vec<Problem>) {
    let mut plans = Vec::new();
    let mut problems = Vec::new();
    let mut seen: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    for (i, v) in rows.iter().enumerate() {
        let row = i + 1;
        let Value::Object(o) = v else {
            problems.push(Problem {
                row,
                id: None,
                field: "row".into(),
                problem: format!("the row is {}, not a card object", kind_of(v)),
                hint: format!("a row looks like {}", example(mode)),
            });
            continue;
        };
        let before = problems.len();
        let p = plan_row(row, o, mode, &mut problems);
        if let (Mode::Edit, Some(id)) = (mode, p.id) {
            if let Some(first) = seen.insert(id, row) {
                problems.push(Problem {
                    row,
                    id: Some(id),
                    field: "id".into(),
                    problem: format!("card #{id} is already changed by row {first}"),
                    hint: "keep one row per card".into(),
                });
            }
        }
        if problems.len() == before {
            plans.push(p);
        }
    }
    (plans, problems)
}

fn plan_row(row: usize, o: &Map<String, Value>, mode: Mode, problems: &mut Vec<Problem>) -> RowPlan {
    let mut p = RowPlan { row, ..Default::default() };
    let known: &[&str] = if mode == Mode::Import { &IMPORT_FIELDS } else { &EDIT_FIELDS };
    p.ignored = o.keys().filter(|k| !known.contains(&k.as_str())).cloned().collect();
    // the id first, so every later problem on this row can name its card
    if mode == Mode::Edit {
        match o.get("id") {
            Some(Value::Number(n)) if n.as_i64().is_some_and(|n| n > 0) => p.id = n.as_i64(),
            Some(other) => problems.push(Problem {
                row,
                id: None,
                field: "id".into(),
                problem: format!("id is {}, not a card number", shown(other)),
                hint: "use the number 'tb list' shows, e.g. \"id\": 7".into(),
            }),
            None => problems.push(Problem {
                row,
                id: None,
                field: "id".into(),
                problem: "the row has no id, so it names no card".into(),
                hint: "add \"id\": N (see 'tb list' for ids), or create cards with 'tb import'".into(),
            }),
        }
    }
    let id = p.id;
    let mut bad = |field: &str, problem: String, hint: &str| {
        problems.push(Problem { row, id, field: field.into(), problem, hint: hint.into() });
    };
    match o.get("title") {
        Some(Value::String(t)) if !t.trim().is_empty() => p.title = Some(t.trim().to_string()),
        Some(Value::String(_)) => bad("title", "the title is empty".into(), "give it words, e.g. \"title\": \"tag: what to do\""),
        Some(other) => bad("title", format!("the title is {}, not text", kind_of(other)), "e.g. \"title\": \"tag: what to do\""),
        None if mode == Mode::Import => bad("title", "the row has no title".into(), "every new card needs one, e.g. \"title\": \"tag: what to do\""),
        None => {}
    }
    match o.get("tag") {
        None => {}
        Some(Value::Null) => p.tag = Some(None),
        Some(Value::String(t)) if t.trim().is_empty() => p.tag = Some(None),
        Some(Value::String(t)) => {
            let t = t.trim();
            let ok = t.len() <= 20 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if ok {
                p.tag = Some(Some(t.to_ascii_lowercase()));
            } else {
                bad("tag", format!("'{t}' cannot be a tag"), "a tag is up to 20 letters, digits, '-' or '_' (or null for none)");
            }
        }
        Some(other) => bad("tag", format!("the tag is {}, not text", kind_of(other)), "e.g. \"tag\": \"docs\" (or null for none)"),
    }
    match o.get("gh_ref") {
        None => {}
        Some(Value::Null) => p.gh_ref = Some(None),
        Some(Value::Number(n)) if n.as_i64().is_some_and(|n| n > 0) => p.gh_ref = Some(n.as_i64()),
        Some(other) => bad("gh_ref", format!("gh_ref is {}, not an issue number", shown(other)), "e.g. \"gh_ref\": 315 (or null for none)"),
    }
    match o.get("description") {
        None => {}
        Some(Value::Null) => p.description = Some(String::new()),
        Some(Value::String(d)) => p.description = Some(d.trim().to_string()),
        Some(other) => bad("description", format!("the description is {}, not text", kind_of(other)), "e.g. \"description\": \"Done = …\""),
    }
    match o.get("due") {
        None => {}
        Some(Value::Null) => p.due = Some(None),
        Some(Value::String(d)) => match DueDate::parse(d, "") {
            Ok(date) => p.due = Some(date),
            Err(e) => {
                // the single-card refusal, word for word: "'X' is not a real calendar date — use …"
                let what = e.0.split(" — ").next().unwrap_or(&e.0).to_string();
                bad("due", what, "use YYYY-MM-DD, e.g. \"due\": \"2026-10-09\" (or null for none)");
            }
        },
        Some(other) => bad("due", format!("the due date is {}, not text", kind_of(other)), "use YYYY-MM-DD, e.g. \"due\": \"2026-10-09\" (or null for none)"),
    }
    match o.get("blocked") {
        None => {}
        Some(Value::Null) => p.blocked = Some(None),
        Some(Value::String(b)) => {
            // the same normalisation `tb block` applies
            let b = b.trim().trim_start_matches("by ").trim();
            p.blocked = Some((!b.is_empty()).then(|| b.to_string()));
        }
        Some(other) => bad("blocked", format!("blocked is {}, not text", kind_of(other)), "say what blocks it, e.g. \"blocked\": \"#7\" (or null for none)"),
    }
    // a title may carry its own `tag:` and `gh#N` (as `tb add` accepts): a field that says
    // something else is a contradiction, never a silent winner
    if let Some(t) = &p.title {
        let (t_tag, t_gh, _) = crate::store::parse_title(t);
        if let (Some(in_title), Some(field)) = (&t_tag, &p.tag) {
            if field.as_ref() != Some(in_title) {
                let said = field.as_ref().map_or("null".to_string(), |f| format!("'{f}'"));
                bad("tag", format!("the title carries the tag '{in_title}' but tag is {said}"), "make them agree, or leave the tag out of one of them");
            }
        }
        if let (Some(in_title), Some(field)) = (t_gh, p.gh_ref) {
            if field != Some(in_title) {
                let said = field.map_or("null".to_string(), |n| n.to_string());
                bad("gh_ref", format!("the title carries gh#{in_title} but gh_ref is {said}"), "make them agree, or leave it out of one of them");
            }
        }
    }
    if mode == Mode::Import {
        match o.get("checklist") {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) => {
                let mut list = Vec::new();
                for (n, item) in items.iter().enumerate() {
                    let field = format!("checklist[{}]", n + 1);
                    let hint = "an item is text, or {\"text\": \"…\", \"done\": false}";
                    match item {
                        Value::String(t) if !t.trim().is_empty() => list.push((t.trim().to_string(), false)),
                        Value::Object(it) => match (it.get("text"), it.get("done")) {
                            (Some(Value::String(t)), done) if !t.trim().is_empty() && matches!(done, None | Some(Value::Bool(_))) => {
                                list.push((t.trim().to_string(), done.and_then(Value::as_bool).unwrap_or(false)))
                            }
                            (Some(Value::String(t)), _) if t.trim().is_empty() => bad(&field, "the item is empty".into(), hint),
                            (Some(Value::String(_)), Some(d)) => bad(&field, format!("done is {}, not true/false", kind_of(d)), hint),
                            _ => bad(&field, "the item has no text".into(), hint),
                        },
                        Value::String(_) => bad(&field, "the item is empty".into(), hint),
                        other => bad(&field, format!("the item is {}, not text", kind_of(other)), hint),
                    }
                }
                p.checklist = Some(list);
            }
            Some(other) => bad("checklist", format!("the checklist is {}, not a list", kind_of(other)), "e.g. \"checklist\": [\"draft\", \"review\"]"),
        }
    }
    p
}

/// A value, short enough for one line of a report.
fn shown(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 40 {
        format!("{}…", s.chars().take(39).collect::<String>())
    } else {
        s
    }
}

/// One change to one field, for the report.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Change {
    pub field: String,
    pub from: Value,
    pub to: Value,
}

/// What happened (or, in a dry run, would happen) to one row.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RowResult {
    pub row: usize,
    pub id: i64,
    /// `created` · `changed` · `unchanged`
    pub action: &'static str,
    pub title: String,
    pub changes: Vec<Change>,
    pub ignored: Vec<String>,
}

impl RowResult {
    /// `row 3  #43 changed  due none -> 2026-10-09 · description edited`
    pub fn line(&self, dry_run: bool) -> String {
        let verb = match (self.action, dry_run) {
            ("created", true) => "would create",
            ("changed", true) => "would change",
            (a, _) => a,
        };
        let text = |v: &Value| match v {
            Value::Null => "none".to_string(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let what: Vec<String> = self
            .changes
            .iter()
            // a new card's title line already shows its tag, link and title
            .filter(|c| self.action != "created" || !matches!(c.field.as_str(), "title" | "tag" | "gh_ref" | "description"))
            .map(|c| match c.field.as_str() {
                "description" => "description edited".to_string(),
                "checklist" => format!("checklist {}", text(&c.to)),
                "blocked" if self.action == "created" => format!("blocked by {}", text(&c.to)),
                f if self.action == "created" => format!("{f} {}", text(&c.to)),
                f => format!("{f} {} -> {}", text(&c.from), text(&c.to)),
            })
            .collect();
        let mut line = format!("row {:<4} #{} {verb}", self.row, self.id);
        if self.action == "created" {
            line.push_str(&format!("  {}", self.title));
        }
        if !what.is_empty() {
            line.push_str(&format!("  {}", what.join(" · ")));
        }
        line
    }
}

/// One warning per ignored field, with how many rows carried it — never a line per row.
pub fn ignored_warnings(results: &[RowResult], mode: Mode) -> Vec<String> {
    let mut count: Vec<(String, usize)> = Vec::new();
    for r in results {
        for f in &r.ignored {
            match count.iter_mut().find(|(k, _)| k == f) {
                Some((_, n)) => *n += 1,
                None => count.push((f.clone(), 1)),
            }
        }
    }
    if count.is_empty() {
        return Vec::new();
    }
    let fields: Vec<String> = count.iter().map(|(k, n)| format!("{k} ({n})")).collect();
    let why = match mode {
        Mode::Import => "new cards always land in TODO with a new id, and history is never imported",
        Mode::Edit => "'tb edit --from' changes title, tag, gh_ref, description, due and blocked only",
    };
    vec![format!("ignored fields, with the number of rows that had them: {} — {why}", fields.join(", "))]
}

/// The `--json` answer for a run that went through (or a dry run that would).
pub fn report_json(mode: Mode, source: &str, dry_run: bool, results: &[RowResult], warnings: &[String]) -> Value {
    let ids = |action: &str| -> Vec<i64> { results.iter().filter(|r| r.action == action).map(|r| r.id).collect() };
    let mut v = json!({
        "ok": true,
        "command": mode.as_str(),
        "source": source,
        "dry_run": dry_run,
        "rows": results,
        "warnings": warnings,
    });
    match mode {
        Mode::Import => v["created"] = json!(ids("created")),
        Mode::Edit => {
            v["changed"] = json!(ids("changed"));
            v["unchanged"] = json!(ids("unchanged"));
        }
    }
    v
}

/// The summary of a refused run: what, then what to do — the usual error shape.
pub fn refusal(mode: Mode, source: &str, problems: &[Problem]) -> String {
    let n = problems.len();
    format!(
        "{n} problem{} in {source} — nothing was written; fix {} and check again with 'tb {} {source} --dry-run'",
        if n == 1 { "" } else { "s" },
        if n == 1 { "it" } else { "them" },
        mode.usage()
    )
}

/// A run that was refused and has ALREADY printed its report: `main` exits 1 without printing
/// the error again (the `--json` refusal carries the per-row problems, not just a summary).
pub const REPORTED: &str = "\u{0}reported";

/// A bulk command, read and checked before any board is opened (or created).
#[derive(Debug)]
pub struct Request {
    pub mode: Mode,
    /// The file as typed (for hints) and its bare name (for the `imported` event).
    pub shown: String,
    pub name: String,
    pub dry_run: bool,
    /// Edit cards other people hold (`tb edit --from … --force`), logged per card.
    pub force: bool,
    pub plans: Vec<RowPlan>,
    pub problems: Vec<Problem>,
}

impl Request {
    /// Read `file` (`-` = standard input) through the bounded reader — UTF-8, at most
    /// `textin::MAX_DOC_BYTES`, never a terminal — and plan every row.
    pub fn read(mode: Mode, file: &std::path::Path, dry_run: bool, force: bool) -> Result<Request, BoardError> {
        let text = crate::textin::read_up_to(file, mode.usage(), crate::textin::MAX_DOC_BYTES)?;
        let stdin = file.as_os_str() == "-";
        let shown = if stdin { "-".to_string() } else { file.display().to_string() };
        let name = if stdin {
            "standard input".to_string()
        } else {
            file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| shown.clone())
        };
        let rows = rows_of(&text, mode, &shown)?;
        let (plans, problems) = plan(&rows, mode);
        Ok(Request { mode, shown, name, dry_run, force, plans, problems })
    }

    /// Will this run change the board? (A dry run, or a file with problems, never creates one.)
    pub fn will_write(&self) -> bool {
        !self.dry_run && self.problems.is_empty()
    }
}

/// Apply a request to `store` and print the per-row report (plain, or one JSON object).
/// `hinted` puts an explicitly named board into the commands a hint names.
pub fn run(
    store: &mut crate::store::Store,
    req: Request,
    actor: &str,
    json_out: bool,
    hinted: &dyn Fn(&str) -> String,
) -> Result<(), BoardError> {
    let Request { mode, shown, name, dry_run, force, plans, mut problems } = req;
    // rows that are fine on their own are still checked against the board, so ONE pass names
    // every problem in the file; with any problem at all, this is a dry run
    let refused = !problems.is_empty();
    let results = match store.bulk_apply(mode, &plans, actor, &name, dry_run || refused, force)? {
        Ok(results) => results,
        Err(found) => {
            problems.extend(found);
            Vec::new()
        }
    };
    if !problems.is_empty() {
        problems.sort_by_key(|p| p.row);
        let summary = refusal(mode, &shown, &problems);
        if json_out {
            let (error, hint) = summary.split_once(" — ").unwrap_or((&summary, ""));
            let rows: Vec<Value> = problems
                .iter()
                .map(|p| json!({"row": p.row, "id": p.id, "field": p.field, "problem": p.problem, "hint": hinted(&p.hint)}))
                .collect();
            let v = json!({"ok": false, "error": error, "hint": hinted(hint), "command": mode.as_str(), "source": shown, "dry_run": dry_run, "problems": rows});
            println!(
                "{}",
                serde_json::to_string_pretty(&crate::clean_json(&v)).unwrap_or_default()
            );
            return Err(BoardError(REPORTED.to_string()));
        }
        for p in &problems {
            eprintln!("{}", crate::text::sanitize(&hinted(&p.line())));
        }
        return Err(BoardError(summary));
    }
    let warnings = ignored_warnings(&results, mode);
    if json_out {
        let report = report_json(mode, &shown, dry_run, &results, &warnings);
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::clean_json(&report)).unwrap_or_default()
        );
        return Ok(());
    }
    for r in &results {
        println!("{}", crate::text::sanitize(&r.line(dry_run)));
    }
    let count = |a: &str| results.iter().filter(|r| r.action == a).count();
    let would = if dry_run { "would be " } else { "" };
    let summary = match mode {
        Mode::Import => {
            let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
            let range = match (ids.first(), ids.last()) {
                (Some(a), Some(b)) if a != b => format!("#{a}–#{b}"),
                (Some(a), _) => format!("#{a}"),
                _ => String::new(),
            };
            format!("{} card{} {would}imported into TODO ({range}) from {name}", ids.len(), if ids.len() == 1 { "" } else { "s" })
        }
        Mode::Edit => format!("{} of {} cards {would}changed ({} unchanged) from {name}", count("changed"), results.len(), count("unchanged")),
    };
    println!("{}", crate::text::sanitize(&summary));
    for w in &warnings {
        eprintln!("{}", crate::text::sanitize(&format!("tb: {w}")));
    }
    if dry_run {
        println!("dry run — nothing was written; run it for real with {}", hinted(&format!("'tb {} {shown}'", mode.usage())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_come_in_four_shapes() {
        let card = r#"{"title": "a: one"}"#;
        for doc in [format!("[{card}]"), format!(r#"{{"cards": [{card}]}}"#), format!(r#"{{"v":1,"columns":{{"todo":[{card}],"done":[]}}}}"#), card.to_string()] {
            assert_eq!(rows_of(&doc, Mode::Import, "f.json").unwrap().len(), 1, "{doc}");
        }
        for (doc, want) in [("[", "is not valid JSON"), ("7", "is not a list of cards"), (r#"{"cards": 7}"#, "is not a list of cards"), ("{}", "is not a list of cards"), ("[]", "holds no cards")] {
            let e = rows_of(doc, Mode::Import, "f.json").unwrap_err().0;
            assert!(e.starts_with("f.json ") && e.contains(want) && e.contains(" — "), "{doc}: {e}");
        }
        // a board's columns come out in board order
        let board = r#"{"columns":{"done":[{"title":"d"}],"todo":[{"title":"t"}],"review":[{"title":"r"}],"doing":[{"title":"g"}]}}"#;
        let titles: Vec<String> = rows_of(board, Mode::Import, "f").unwrap().iter().map(|r| r["title"].as_str().unwrap().to_string()).collect();
        assert_eq!(titles, ["t", "g", "r", "d"]);
    }

    #[test]
    fn every_problem_names_its_row_id_and_field() {
        let rows: Vec<Value> = serde_json::from_str(
            r#"[{"id": 1, "due": "2026-10-09"}, 7, {"id": 2, "due": "2026-02-30", "tag": "two words"}, {"due": null}, {"id": "x"}, {"id": 1, "title": ""}]"#,
        )
        .unwrap();
        let (plans, problems) = plan(&rows, Mode::Edit);
        assert_eq!(plans.len(), 1);
        let got: Vec<(usize, Option<i64>, &str)> = problems.iter().map(|p| (p.row, p.id, p.field.as_str())).collect();
        assert_eq!(got, [(2, None, "row"), (3, Some(2), "tag"), (3, Some(2), "due"), (4, None, "id"), (5, None, "id"), (6, Some(1), "title"), (6, Some(1), "id")]);
        assert_eq!(problems[2].line(), "row 3 (#2) due: '2026-02-30' is not a real calendar date — use YYYY-MM-DD, e.g. \"due\": \"2026-10-09\" (or null for none)");
        assert!(problems[6].problem.contains("already changed by row 1"), "{:?}", problems[6]);
    }

    #[test]
    fn absent_null_and_present_are_three_different_things() {
        let rows: Vec<Value> = serde_json::from_str(r#"[{"id": 3, "due": null, "blocked": "by #7", "column": "done", "events": [], "zz": 1}]"#).unwrap();
        let (plans, problems) = plan(&rows, Mode::Edit);
        assert!(problems.is_empty(), "{problems:?}");
        let p = &plans[0];
        assert_eq!((p.due.clone(), p.blocked.clone(), p.title.clone(), p.tag.clone()), (Some(None), Some(Some("#7".into())), None, None));
        assert_eq!(p.ignored, ["column", "events", "zz"]);
    }
}
