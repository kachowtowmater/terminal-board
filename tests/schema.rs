//! docs/SCHEMA.md is a supported read-only interface: the test fails when a column exists
//! in the database but is not documented there (and when a documented one disappears).
use std::path::Path;
use terminal_board::store::Store;

fn tables_of(conn: &rusqlite::Connection) -> Vec<String> {
    let mut st = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap();
    st.query_map([], |r| r.get::<_, String>(0)).unwrap().map(|r| r.unwrap()).collect()
}

fn cols_of(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut st = conn
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}') ORDER BY cid"))
        .unwrap();
    st.query_map([], |r| r.get::<_, String>(0)).unwrap().map(|r| r.unwrap()).collect()
}

/// "### table" then a `| column |` row per documented column.
fn documented(path: &Path, table: &str) -> Option<Vec<String>> {
    let md = std::fs::read_to_string(path).unwrap_or_default(); // missing doc = nothing documented
    let start = md.find(&format!("### {table}\n"))?;
    let rest = &md[start..];
    let end = rest.find("\n### ").map(|i| i + 1).unwrap_or(rest.len());
    let section = &rest[..end];
    Some(
        section
            .lines()
            .skip_while(|l| !l.starts_with("| column |"))
            .skip(2) // header + separator
            .take_while(|l| l.starts_with('|'))
            .map(|l| l.split('|').nth(1).unwrap_or("").trim().trim_matches('`').to_string())
            .filter(|c| !c.is_empty())
            .collect(),
    )
}

#[test]
fn every_column_of_every_table_is_documented_in_schema_md() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("b.db");
    {
        let s = Store::open(&db).unwrap();
        s.add("seed: one card", "", &["a check".to_string()], "me").unwrap();
    }
    let md = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/SCHEMA.md");
    // read the board the way SCHEMA.md tells outside readers to: its own read-only connection
    let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let conn = &conn;
    for t in tables_of(conn) {
        let actual = cols_of(conn, &t);
        let doc = documented(&md, &t).unwrap_or_else(|| panic!("table {t} exists but has no '### {t}' section in docs/SCHEMA.md"));
        for c in &actual {
            assert!(
                doc.iter().any(|d| d == c || d.split(' ').next() == Some(c.as_str())),
                "column `{t}.{c}` is not documented in docs/SCHEMA.md — document it (docs are a supported interface)"
            );
        }
        for d in &doc {
            let name = d.split_whitespace().next().unwrap().trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
            assert!(
                actual.contains(&name.to_string()),
                "docs/SCHEMA.md documents `{t}.{name}` but the table has no such column — update the doc"
            );
        }
    }
}
