//! Warnings: things tb must say without failing the command (an ignored `TB_BOARD`, a board
//! file other users can read, a backup written before a schema upgrade).
//!
//! The library pushes them here; it never prints, because the full-screen board may own the
//! terminal. The binary prints each one once on stderr (`tb: …`) and, with `--json`, adds
//! them to object-shaped output as `"warnings": ["…"]`. The field is absent when there are
//! none, so output without warnings is byte-for-byte what it was.
//!
//! Each warning carries the key of the board it concerns (its resolved file path), or no key
//! at all for one that concerns the whole command rather than any one board (an ignored
//! `TB_BOARD`). This is a process-wide static, but nothing may ever drain ANOTHER board's
//! warnings by mistake: `take_unprinted_for` only ever returns entries raised under its own
//! key, so two boards open in the same process — or, in the test binary, two unrelated tests
//! running at once, each against its own tempdir — can never see each other's warnings. A
//! path is unique per board by construction, so this needs no test-only serialization: two
//! `Store::open` calls on two different files can never collide on a key.

use std::sync::Mutex;

struct Entry {
    /// The board this warning concerns (its resolved file path), or `None` for one that
    /// concerns the whole command and no one board.
    key: Option<String>,
    msg: String,
    printed: bool,
}

struct Notices {
    entries: Vec<Entry>,
}

static NOTICES: Mutex<Notices> = Mutex::new(Notices { entries: Vec::new() });

fn lock() -> std::sync::MutexGuard<'static, Notices> {
    NOTICES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Record a warning that concerns no one board (an ignored `TB_BOARD`, say). The same text
/// under the same key is kept once, however often it is raised (the board picker and `tb
/// boards` open every board; a background refresh reopens one).
pub fn push(msg: impl Into<String>) {
    push_inner(None, msg.into());
}

/// Record a warning about the board at `key` (its resolved file path — the same string
/// `Store::path` reports for it, so `take_unprinted_for` can find it again). Everything a
/// `Store::open` raises about itself (a wide file, a schema-upgrade backup) belongs here, not
/// under `push`, so the full-screen board built on that store only ever hears about itself.
pub fn push_for(key: &str, msg: impl Into<String>) {
    push_inner(Some(key.to_string()), msg.into());
}

fn push_inner(key: Option<String>, msg: String) {
    let mut n = lock();
    if !n.entries.iter().any(|e| e.key == key && e.msg == msg) {
        n.entries.push(Entry { key, msg, printed: false });
    }
}

/// Every warning raised so far, oldest first, of any key — for `--json`'s additive
/// `warnings` field: a command runs against one board, so every warning it raised is that
/// command's business regardless of key.
pub fn all() -> Vec<String> {
    lock().entries.iter().map(|e| e.msg.clone()).collect()
}

/// The warnings nobody has printed yet, of ANY key; they count as printed from here on. For
/// the CLI's stderr dump and the full-screen board's exit-time catch-all — both cover one
/// command's whole lifetime, so every key raised during it is fair game.
pub fn take_unprinted() -> Vec<String> {
    let mut n = lock();
    let out = n.entries.iter().filter(|e| !e.printed).map(|e| e.msg.clone()).collect();
    for e in n.entries.iter_mut() {
        e.printed = true;
    }
    out
}

/// The warnings nobody has printed yet that concern the board at `key` — never another
/// board's, and never one that concerns no board at all. Only those entries count as printed
/// afterwards; everything else is left exactly as it was for its own key's next drain. This
/// is what the full-screen board's status line drains from (`App::reload`), keyed by the
/// store it is showing.
pub fn take_unprinted_for(key: &str) -> Vec<String> {
    let mut n = lock();
    let out = n.entries.iter().filter(|e| !e.printed && e.key.as_deref() == Some(key)).map(|e| e.msg.clone()).collect();
    for e in n.entries.iter_mut() {
        if !e.printed && e.key.as_deref() == Some(key) {
            e.printed = true;
        }
    }
    out
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

    /// A key isolates a drain: pushing under one key never lets another key's (or the
    /// unkeyed queue's) drain see it, and a keyed drain never marks another key's entry
    /// printed. This is the property the full-screen board's per-board status line depends
    /// on to stay safe when more than one board is open in the same process.
    #[test]
    fn a_keyed_drain_never_sees_another_keys_entry() {
        push("global notice: card-test-key-isolation-0");
        push_for("board-a", "card-test-key-isolation-a");
        push_for("board-b", "card-test-key-isolation-b");
        let a = take_unprinted_for("board-a");
        assert_eq!(a, vec!["card-test-key-isolation-a".to_string()]);
        // board-b's own entry is still there, unprinted, for its own key's drain
        let b = take_unprinted_for("board-b");
        assert_eq!(b, vec!["card-test-key-isolation-b".to_string()]);
        // a second drain of board-a sees nothing new (already printed), never board-b's
        assert!(take_unprinted_for("board-a").is_empty());
        // the unkeyed entry is untouched by either keyed drain
        assert!(all().iter().any(|m| m == "global notice: card-test-key-isolation-0"));
    }
}
