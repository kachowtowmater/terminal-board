//! Terminal Board (`tb`) — a simple terminal task board shared by people and AI agents.

// Every `println!`/`print!` in tb goes through `write_stdout` (these shadow std's): a reader
// that closes early (`tb list | head -1`, `tb config | grep -q …`) ends tb quietly with exit 0,
// as `tb watch` already does, instead of a "failed printing to stdout" panic (exit 101).
macro_rules! println {
    () => { $crate::write_stdout("\n") };
    ($($a:tt)*) => { $crate::write_stdout(&format!("{}\n", format_args!($($a)*))) };
}
macro_rules! print {
    ($($a:tt)*) => { $crate::write_stdout(&format!($($a)*)) };
}

pub mod boards;
pub mod contract;
pub mod export;
pub mod filter;
pub mod fsperm;
pub mod github;
pub mod herdr;
pub mod hooks;
pub mod import;
pub mod lock;
pub mod machine;
pub mod notice;
pub mod plain;
pub mod proc;
pub mod roster;
pub mod setup;
pub mod store;
pub mod text;
pub mod textin;
pub mod tui;
pub mod waits;

/// Convert a value to JSON with every text leaf cleaned through the display sanitiser
/// (`text::sanitize_json`) — one shared function with the screen paths, so `--json` output
/// never carries DEL, C1 controls or terminal escape sequences (see docs/JSON.md). Shape,
/// field names and key order are untouched; only `String` content changes.
pub fn clean_json<T: serde::Serialize>(v: &T) -> serde_json::Value {
    use serde_json::Value;
    enum Cleaned<'a> {
        Val(&'a Value),
        Text(&'a str),
        Seq(Vec<Cleaned<'a>>),
        Map(Vec<(String, Cleaned<'a>)>),
    }
    fn of(v: &Value) -> Cleaned<'_> {
        match v {
            Value::String(s) => Cleaned::Text(s),
            Value::Array(xs) => Cleaned::Seq(xs.iter().map(of).collect()),
            Value::Object(m) => Cleaned::Map(m.iter().map(|(k, v)| (k.clone(), of(v))).collect()),
            other => Cleaned::Val(other),
        }
    }
    fn put(c: &Cleaned<'_>, s: &mut String) {
        match c {
            Cleaned::Val(v) => s.push_str(&v.to_string()),
            Cleaned::Text(t) => {
                s.push_str(&serde_json::to_string(&text::sanitize_json(t)).unwrap_or_default())
            }
            Cleaned::Seq(xs) => {
                s.push('[');
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        s.push(',');
                    }
                    put(x, s);
                }
                s.push(']');
            }
            Cleaned::Map(entries) => {
                s.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        s.push(',');
                    }
                    s.push_str(&serde_json::to_string(k).unwrap_or_default());
                    s.push(':');
                    put(v, s);
                }
                s.push('}');
            }
        }
    }
    let value = serde_json::to_value(v).unwrap_or(Value::Null);
    let mut out = String::new();
    let cleaned = of(&value);
    put(&cleaned, &mut out);
    serde_json::from_str(&out).unwrap_or(Value::Null)
}

/// Write `s` to stdout and flush. A closed stdout (the reader went away) exits 0: the reader
/// has what it wanted. Any other write error exits 1 with a message. Never panics.
pub fn write_stdout(s: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    if let Err(e) = out.write_all(s.as_bytes()).and_then(|()| out.flush()) {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        eprintln!("tb: could not write to stdout: {e}");
        std::process::exit(1);
    }
}

/// `TB_<name>`, falling back to the pre-rename `TTYBOARD_<name>` (the new name wins).
pub fn env(name: &str) -> Option<String> {
    let get = |k: String| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    get(format!("TB_{name}")).or_else(|| get(format!("TTYBOARD_{name}")))
}

/// Actor identity: `--as`, else `TB_AS`, else `HERDR_AGENT_NAME`, else the herdr agent name
/// of this pane (`HERDR_PANE_ID`, asked from herdr), else `USER`.
///
/// Trimmed once, here, whichever source wins: every place tb later compares this actor
/// against a stored name (the card holder, `done-by`, the self-approval guard, the
/// reviewer-claim guard) then works with the same clean value, so `--as "anna "` matches
/// `--as anna` everywhere instead of only where someone remembered to trim (#103). An
/// empty-after-trim value never reaches here — the caller refuses `--as " "` up front, same
/// as `--as ""` — so trimming can never turn a real name into `""`.
pub fn resolve_actor(flag: Option<&str>) -> String {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let raw = flag
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
        .or_else(|| crate::env("AS"))
        .or_else(|| env("HERDR_AGENT_NAME"))
        .or_else(|| env("HERDR_PANE_ID").and_then(|p| herdr::agent_name_for_pane(p.trim())))
        .or_else(|| env("USER"))
        .unwrap_or_else(|| "someone".into());
    raw.trim().to_string()
}
