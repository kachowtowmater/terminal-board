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
pub mod filter;
pub mod fsperm;
pub mod github;
pub mod herdr;
pub mod import;
pub mod machine;
pub mod notice;
pub mod plain;
pub mod roster;
pub mod setup;
pub mod store;
pub mod text;
pub mod textin;
pub mod tui;

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
pub fn resolve_actor(flag: Option<&str>) -> String {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    flag.map(str::to_string)
        .filter(|v| !v.trim().is_empty())
        .or_else(|| crate::env("AS"))
        .or_else(|| env("HERDR_AGENT_NAME"))
        .or_else(|| env("HERDR_PANE_ID").and_then(|p| herdr::agent_name_for_pane(p.trim())))
        .or_else(|| env("USER"))
        .unwrap_or_else(|| "someone".into())
}
