//! Terminal Board (`tb`) — a simple terminal task board shared by people and AI agents.

pub mod boards;
pub mod contract;
pub mod github;
pub mod herdr;
pub mod plain;
pub mod setup;
pub mod store;
pub mod tui;

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
