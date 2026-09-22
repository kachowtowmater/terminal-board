//! Warnings: things tb must say without failing the command (an ignored `TB_BOARD`, a board
//! file other users can read, a backup written before a schema upgrade).
//!
//! The library pushes them here; it never prints, because the full-screen board may own the
//! terminal. The binary prints each one once on stderr (`tb: …`) and, with `--json`, adds
//! them to object-shaped output as `"warnings": ["…"]`. The field is absent when there are
//! none, so output without warnings is byte-for-byte what it was.

use std::sync::Mutex;

struct Notices {
    all: Vec<String>,
    /// How many of `all` were already handed out by `take_unprinted`.
    printed: usize,
}

static NOTICES: Mutex<Notices> = Mutex::new(Notices { all: Vec::new(), printed: 0 });

fn lock() -> std::sync::MutexGuard<'static, Notices> {
    NOTICES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Record a warning. The same text is kept once, however often it is raised (the board
/// picker and `tb boards` open every board; a background refresh reopens one).
pub fn push(msg: impl Into<String>) {
    let msg = msg.into();
    let mut n = lock();
    if !n.all.contains(&msg) {
        n.all.push(msg);
    }
}

/// Every warning raised so far, oldest first.
pub fn all() -> Vec<String> {
    lock().all.clone()
}

/// The warnings nobody printed yet; they count as printed from here on.
pub fn take_unprinted() -> Vec<String> {
    let mut n = lock();
    let new = n.all[n.printed..].to_vec();
    n.printed = n.all.len();
    new
}

/// Add `"warnings": […]` as the last key of an already pretty-printed JSON object.
///
/// Done on the text, not by parsing: tb's objects are serialized from structs in a documented
/// key order, and a round trip through a generic JSON value would re-sort every key.
/// Anything that is not a pretty-printed object (an array, a scalar) comes back unchanged —
/// those outputs have no place for a field, and the warning is on stderr.
pub fn splice(json: &str, warnings: &[String]) -> String {
    if warnings.is_empty() {
        return json.to_string();
    }
    let body = json.trim_end();
    if !body.starts_with('{') || !body.ends_with('}') {
        return json.to_string();
    }
    let list = serde_json::to_string_pretty(warnings).unwrap_or_else(|_| "[]".into()).replace('\n', "\n  ");
    let inner = body[1..body.len() - 1].trim_end();
    let tail = &json[body.len()..];
    if inner.trim().is_empty() {
        return format!("{{\n  \"warnings\": {list}\n}}{tail}");
    }
    format!("{{{inner},\n  \"warnings\": {list}\n}}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splice_keeps_the_text_and_adds_the_last_key() {
        let obj = "{\n  \"ok\": true,\n  \"card\": {\n    \"id\": 1\n  }\n}";
        assert_eq!(splice(obj, &[]), obj, "no warnings: not one byte changes");
        let out = splice(obj, &["a \"quoted\" one".into(), "two".into()]);
        assert!(out.starts_with("{\n  \"ok\": true,\n  \"card\": {\n    \"id\": 1\n  },\n  \"warnings\": [\n"), "{out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["card"]["id"], 1);
        assert_eq!(v["warnings"], serde_json::json!(["a \"quoted\" one", "two"]));
        // what it produces is exactly what the pretty printer would print for that object
        assert_eq!(out.lines().last(), Some("}"));
        assert!(out.contains("\n  \"warnings\": [\n    \"a \\\"quoted\\\" one\",\n    \"two\"\n  ]\n}"), "{out}");
    }

    #[test]
    fn splice_leaves_arrays_scalars_and_empty_objects_well_formed() {
        let w = vec!["w".to_string()];
        assert_eq!(splice("[\n  1\n]", &w), "[\n  1\n]");
        assert_eq!(splice("null", &w), "null");
        let v: serde_json::Value = serde_json::from_str(&splice("{}", &w)).unwrap();
        assert_eq!(v, serde_json::json!({"warnings": ["w"]}));
        // a trailing newline survives
        let v: serde_json::Value = serde_json::from_str(&splice("{\n  \"a\": 1\n}\n", &w)).unwrap();
        assert_eq!(v, serde_json::json!({"a": 1, "warnings": ["w"]}));
    }
}
