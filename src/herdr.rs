//! Agents panel data from `herdr pane list` (best effort, never fatal).

use crate::store::Card;
use serde_json::Value;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    /// Display name: herdr agent `name`, else first `·` label segment without ` (...)`, else pane id.
    pub name: String,
    /// The herdr agent name, when the agent has one (matched first against card owners).
    pub agent_name: Option<String>,
    /// Harness kind (`claude`, `aider`, ...).
    pub harness: String,
    pub status: String,
    pub pane_id: String,
    /// Current job: last `·` segment of the pane label (when the label has several).
    pub job: Option<String>,
    /// Fallback names a card owner may match (label segment, its first word, terminal title).
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentsState {
    /// Not probed yet.
    Pending,
    Unavailable(String),
    Agents(Vec<Agent>),
}

fn eq_ci(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

impl Agent {
    /// Does this agent own `card` by any of its names (case-insensitive)?
    pub fn owns(&self, card: &Card) -> bool {
        let Some(owner) = card.owner.as_deref() else { return false };
        self.agent_name.iter().any(|n| eq_ci(n, owner))
            || std::iter::once(&self.name)
                .chain(self.aliases.iter())
                .chain(std::iter::once(&self.pane_id))
                .any(|n| eq_ci(n, owner))
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.status.as_str(), "idle" | "done")
    }
}

/// The agent owning `card`: an exact herdr agent-name match wins over label/title aliases.
pub fn find_owner<'a>(agents: &'a [Agent], card: &Card) -> Option<&'a Agent> {
    let owner = card.owner.as_deref()?;
    agents
        .iter()
        .find(|a| a.agent_name.as_deref().is_some_and(|n| eq_ci(n, owner)))
        .or_else(|| agents.iter().find(|a| a.owns(card)))
}

/// `lead (claude)` -> `lead`
fn strip_paren(s: &str) -> &str {
    let s = s.trim();
    match (s.ends_with(')'), s.rfind(" (")) {
        (true, Some(i)) if i > 0 => s[..i].trim(),
        _ => s,
    }
}

fn sget(p: &Value, k: &str) -> Option<String> {
    p.get(k).and_then(Value::as_str).map(str::to_string).filter(|v| !v.trim().is_empty())
}

fn build(name: Option<String>, harness: String, status: Option<String>, pane_id: String, pane: Option<&Value>) -> Agent {
    let label = pane.and_then(|p| sget(p, "label")).unwrap_or_default();
    let segs: Vec<&str> = label.split('·').map(str::trim).collect();
    let first = strip_paren(segs.first().copied().unwrap_or("")).to_string();
    let job = (segs.len() > 1).then(|| segs[segs.len() - 1].to_string()).filter(|j| !j.is_empty());
    let mut aliases = Vec::new();
    if !first.is_empty() {
        aliases.push(first.clone());
        if let Some(w) = first.split_whitespace().next() {
            aliases.push(w.to_string());
        }
    }
    if let Some(t) = pane.and_then(|p| sget(p, "terminal_title_stripped")) {
        aliases.push(t);
    }
    let display = name.clone().or_else(|| (!first.is_empty()).then(|| first.clone())).unwrap_or_else(|| pane_id.clone());
    Agent {
        name: display,
        agent_name: name,
        harness,
        status: status.unwrap_or_else(|| "unknown".into()),
        pane_id,
        job,
        aliases,
    }
}

fn array<'a>(v: &'a Value, ptr: &str) -> Result<&'a Vec<Value>, String> {
    v.pointer(ptr).and_then(Value::as_array).ok_or_else(|| format!("herdr json has no {ptr}"))
}

/// Primary: `herdr agent list` (has agent `name`), joined by pane_id to `herdr pane list` for labels.
pub fn parse_agents(agents_json: &str, panes_json: Option<&str>) -> Result<Vec<Agent>, String> {
    let v: Value = serde_json::from_str(agents_json).map_err(|e| format!("bad herdr json: {e}"))?;
    let agents = array(&v, "/result/agents")?;
    let panes_v: Option<Value> = panes_json.and_then(|j| serde_json::from_str(j).ok());
    let panes: &[Value] = panes_v
        .as_ref()
        .and_then(|p| p.pointer("/result/panes"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    Ok(agents
        .iter()
        .map(|a| {
            let pane_id = sget(a, "pane_id").unwrap_or_default();
            let pane = panes.iter().find(|p| sget(p, "pane_id").as_deref() == Some(pane_id.as_str()));
            build(
                sget(a, "name"),
                sget(a, "agent").unwrap_or_else(|| "?".into()),
                sget(a, "agent_status"),
                pane_id,
                pane.or(Some(a)),
            )
        })
        .collect())
}

/// Fallback when `herdr agent list` fails: panes with an `agent` field.
pub fn parse_panes(json: &str) -> Result<Vec<Agent>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad herdr json: {e}"))?;
    Ok(array(&v, "/result/panes")?
        .iter()
        .filter_map(|p| {
            let harness = sget(p, "agent")?;
            Some(build(
                sget(p, "name"),
                harness,
                sget(p, "agent_status"),
                sget(p, "pane_id").unwrap_or_default(),
                Some(p),
            ))
        })
        .collect())
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

pub fn herdr_enabled() -> bool {
    if crate::env("NO_HERDR").is_some() {
        return false;
    }
    std::env::var("HERDR_ENV").is_ok_and(|v| v == "1") || on_path("herdr")
}

/// Run `herdr <args>` with a 1s timeout; None on any failure.
fn run_herdr(args: &[&str]) -> Option<String> {
    let bin = std::env::var("HERDR_BIN_PATH").ok().filter(|p| !p.is_empty());
    let mut child = Command::new(bin.as_deref().unwrap_or("herdr"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(None) if start.elapsed() < Duration::from_secs(1) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

/// The herdr agent name of `pane_id` in `herdr agent list` output (None if unnamed or absent).
pub fn name_for_pane(agents_json: &str, pane_id: &str) -> Option<String> {
    parse_agents(agents_json, None)
        .ok()?
        .into_iter()
        .find(|a| a.pane_id == pane_id)
        .and_then(|a| a.agent_name)
}

/// Ask herdr which agent runs in `pane_id` (best effort: None when herdr is off or silent).
pub fn agent_name_for_pane(pane_id: &str) -> Option<String> {
    if pane_id.is_empty() || !herdr_enabled() {
        return None;
    }
    name_for_pane(&run_herdr(&["agent", "list"])?, pane_id)
}

/// `herdr agent list` joined to `herdr pane list`; falls back to pane list alone.
pub fn probe() -> AgentsState {
    let gone = || AgentsState::Unavailable("herdr not available".into());
    if !herdr_enabled() {
        return gone();
    }
    let panes = run_herdr(&["pane", "list"]);
    if let Some(agents) = run_herdr(&["agent", "list"]) {
        if let Ok(a) = parse_agents(&agents, panes.as_deref()) {
            return AgentsState::Agents(a);
        }
    }
    match panes.as_deref().map(parse_panes) {
        Some(Ok(a)) => AgentsState::Agents(a),
        _ => gone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[
      {"name":"bot","agent":"aider","agent_status":"working","pane_id":"w:p2"},
      {"agent":"claude","agent_status":"done","pane_id":"w:p1","terminal_title_stripped":"lead"},
      {"name":"rev","agent":"claude","agent_status":"idle","pane_id":"w:p3"},
      {"agent":"aider","agent_status":"blocked","pane_id":"w:p9"}
    ]}}"#;
    pub const PANES: &str = r#"{"id":"cli:pane:list","result":{"panes":[
      {"agent":"claude","agent_status":"done","label":"lead (claude)","pane_id":"w:p1","terminal_title_stripped":"lead"},
      {"agent":"aider","agent_status":"working","label":"builder 2 · model-x · aider · fix login","pane_id":"w:p2"},
      {"agent":"claude","agent_status":"idle","label":"reviewer · model-y · claude · review #315","pane_id":"w:p3"},
      {"agent_status":"unknown","label":"board · auto-checks","pane_id":"w:p4"}
    ]}}"#;

    fn card(owner: &str) -> Card {
        Card {
            id: 1,
            title: "t".into(),
            tag: None,
            description: String::new(),
            column: "doing".into(),
            owner: Some(owner.into()),
            due: None,
            gh_ref: None,
            created_at: 0,
            column_since: 0,
            blocked: None,
            position: 0,
            reviewer: None,
        }
    }

    #[test]
    fn parses_agent_list_joined_to_panes() {
        let a = parse_agents(AGENTS, Some(PANES)).unwrap();
        assert_eq!(a.len(), 4);
        assert_eq!((a[0].name.as_str(), a[0].harness.as_str(), a[0].status.as_str()), ("bot", "aider", "working"));
        assert_eq!(a[0].job.as_deref(), Some("fix login"));
        assert_eq!(a[1].name, "lead", "no name -> label segment without (claude)");
        assert_eq!(a[1].agent_name, None);
        assert_eq!(a[1].job, None, "single-segment label has no job");
        assert_eq!(a[2].name, "rev");
        assert_eq!(a[2].job.as_deref(), Some("review #315"));
        assert_eq!(a[3].name, "w:p9", "no name, no label -> pane id");
        assert_eq!(a[3].status, "blocked");
    }

    #[test]
    fn agent_list_without_panes_still_works() {
        let a = parse_agents(AGENTS, None).unwrap();
        assert_eq!(a[0].name, "bot");
        assert_eq!(a[1].name, "w:p1");
    }

    #[test]
    fn owner_matching_prefers_name() {
        let a = parse_agents(AGENTS, Some(PANES)).unwrap();
        assert_eq!(find_owner(&a, &card("BOT")).unwrap().pane_id, "w:p2");
        assert_eq!(find_owner(&a, &card("rev")).unwrap().pane_id, "w:p3");
        assert_eq!(find_owner(&a, &card("lead")).unwrap().pane_id, "w:p1");
        assert_eq!(find_owner(&a, &card("builder")).unwrap().pane_id, "w:p2", "label first word");
        assert!(find_owner(&a, &card("nobody")).is_none());
    }

    #[test]
    fn pane_list_fallback() {
        let a = parse_panes(PANES).unwrap();
        assert_eq!(a.len(), 3, "pane without agent is skipped");
        assert_eq!(a[0].name, "lead");
        assert_eq!(a[1].name, "builder 2");
    }

    #[test]
    fn name_for_pane_uses_the_agent_name_only() {
        assert_eq!(name_for_pane(AGENTS, "w:p2").as_deref(), Some("bot"));
        assert_eq!(name_for_pane(AGENTS, "w:p3").as_deref(), Some("rev"));
        assert_eq!(name_for_pane(AGENTS, "w:p1"), None, "unnamed agent: no guess from titles");
        assert_eq!(name_for_pane(AGENTS, "w:p7"), None);
        assert_eq!(name_for_pane("not json", "w:p2"), None);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_agents("not json", None).is_err());
        assert!(parse_agents("{}", None).is_err());
        assert!(parse_panes("{}").is_err());
    }
}
