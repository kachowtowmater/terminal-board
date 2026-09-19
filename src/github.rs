//! GitHub factory snapshot via the `gh` CLI (auth is gh's business; we never touch tokens).
//! `TTYBOARD_GH` points at another binary (tests use a fake script).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Cache younger than this is served as-is; the TUI refreshes on the same period.
pub const MAX_AGE_SECS: i64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pr {
    pub number: i64,
    pub title: String,
    pub head_ref: String,
    pub is_draft: bool,
    /// `ok` (approved), `chg` (changes requested) or `-`
    pub review: String,
    /// `ok`, `FAIL`, `run` or `-`
    pub ci: String,
    pub created_at: String,
    pub author: String,
    /// Issues this PR closes (`closingIssuesReferences`).
    #[serde(default)]
    pub closes: Vec<i64>,
    /// Last update (RFC 3339; empty in caches written before it was fetched).
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Issue {
    pub number: i64,
    pub title: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Merged {
    pub number: i64,
    pub title: String,
    pub merged_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MainCi {
    /// `ok`, `FAIL`, `run` or `-`
    pub state: String,
    pub workflow: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GhSnapshot {
    pub repo: String,
    pub fetched_at: i64,
    pub issues_open: i64,
    pub prs: Vec<Pr>,
    pub issues: Vec<Issue>,
    pub merged_today: Vec<Merged>,
    pub main_ci: Option<MainCi>,
}

/// What the UI/CLI shows: the last good snapshot (if any) plus the last error (if any), and
/// how many refreshes failed in a row (the UI goes red only after 3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GhView {
    pub repo: Option<String>,
    pub snap: Option<GhSnapshot>,
    pub error: Option<String>,
    pub fails: i64,
}

/// Consecutive `gh` refresh failures after which the header turns red.
pub const RED_AFTER_FAILS: i64 = 3;

/// Header suffix for the GITHUB panel title: quiet words, red only after RED_AFTER_FAILS.
/// `(error, fails)` -> `(suffix, red)`; `(None, _)` -> `(empty, false)`.
/// Error text (lower-case) that means the network is down or flaky, not that gh is broken.
const OFFLINE: [&str; 14] = [
    "timed out",
    "timeout",
    "tls handshake",
    "connection refused",
    "temporary failure",
    "getaddrinfo",
    "error connecting to",
    "no such host",
    "network is unreachable",
    "no route to host",
    "connection reset",
    "broken pipe",
    "i/o timeout",
    "check your internet connection",
];

pub fn sync_suffix(error: Option<&str>, fails: i64) -> (String, bool) {
    match error {
        None => (String::new(), false),
        Some(e) => {
            // gh relays Go network errors, which are lower-case; compare lower-case anyway
            let e = e.to_ascii_lowercase();
            let word = if OFFLINE.iter().any(|w| e.contains(w)) { "offline, retrying" } else { "gh error" };
            (format!(" · {word}"), fails >= RED_AFTER_FAILS)
        }
    }
}

const BAD: [&str; 6] = ["FAILURE", "ERROR", "CANCELLED", "TIMED_OUT", "ACTION_REQUIRED", "STARTUP_FAILURE"];

/// statusCheckRollup -> `ok` / `FAIL` / `run` / `-`.
pub fn rollup(checks: &Value) -> String {
    let Some(arr) = checks.as_array().filter(|a| !a.is_empty()) else { return "-".into() };
    let s = |c: &Value, k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_ascii_uppercase();
    let mut running = false;
    for c in arr {
        let (status, conclusion, state) = (s(c, "status"), s(c, "conclusion"), s(c, "state"));
        if BAD.contains(&conclusion.as_str()) || BAD.contains(&state.as_str()) {
            return "FAIL".into();
        }
        let check_run_pending = !status.is_empty() && status != "COMPLETED";
        let status_pending = state == "PENDING" || state == "EXPECTED";
        if check_run_pending || status_pending {
            running = true;
        }
    }
    if running { "run" } else { "ok" }.into()
}

pub fn review(decision: Option<&str>) -> String {
    match decision.unwrap_or("") {
        "APPROVED" => "ok",
        "CHANGES_REQUESTED" => "chg",
        _ => "-",
    }
    .into()
}

fn arr(json: &str, what: &str) -> Result<Vec<Value>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad {what} json: {e}"))?;
    v.as_array().cloned().ok_or_else(|| format!("{what}: expected a JSON array"))
}

fn st(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn names(v: &Value, k: &str, field: &str) -> Vec<String> {
    v.get(k)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| st(x, field)).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

/// `gh pr list --json number,title,headRefName,isDraft,reviewDecision,statusCheckRollup,createdAt,author`
pub fn parse_prs(json: &str) -> Result<Vec<Pr>, String> {
    let mut v: Vec<Pr> = arr(json, "pr list")?
        .iter()
        .map(|p| Pr {
            number: p.get("number").and_then(Value::as_i64).unwrap_or(0),
            title: st(p, "title"),
            head_ref: st(p, "headRefName"),
            is_draft: p.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
            review: review(p.get("reviewDecision").and_then(Value::as_str)),
            ci: rollup(p.get("statusCheckRollup").unwrap_or(&Value::Null)),
            created_at: st(p, "createdAt"),
            author: p.get("author").map(|a| st(a, "login")).unwrap_or_default(),
            closes: p
                .get("closingIssuesReferences")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|i| i.get("number").and_then(Value::as_i64)).collect())
                .unwrap_or_default(),
            updated_at: st(p, "updatedAt"),
        })
        .collect();
    v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(v)
}

/// `gh issue list --json number,title,labels,assignees,createdAt`
pub fn parse_issues(json: &str) -> Result<Vec<Issue>, String> {
    let mut v: Vec<Issue> = arr(json, "issue list")?
        .iter()
        .map(|i| Issue {
            number: i.get("number").and_then(Value::as_i64).unwrap_or(0),
            title: st(i, "title"),
            labels: names(i, "labels", "name"),
            assignees: names(i, "assignees", "login"),
            created_at: st(i, "createdAt"),
        })
        .collect();
    v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(v)
}

/// `gh pr list --state merged --json number,title,mergedAt`
pub fn parse_merged(json: &str) -> Result<Vec<Merged>, String> {
    Ok(arr(json, "merged pr list")?
        .iter()
        .map(|m| Merged {
            number: m.get("number").and_then(Value::as_i64).unwrap_or(0),
            title: st(m, "title"),
            merged_at: st(m, "mergedAt"),
        })
        .collect())
}

/// `gh run list --branch main --limit 1 --json status,conclusion,workflowName,createdAt`
pub fn parse_main_ci(json: &str) -> Result<Option<MainCi>, String> {
    Ok(arr(json, "run list")?.first().map(|r| {
        let (status, conclusion) = (st(r, "status").to_ascii_uppercase(), st(r, "conclusion").to_ascii_uppercase());
        let state = if status != "COMPLETED" {
            "run"
        } else if BAD.contains(&conclusion.as_str()) {
            "FAIL"
        } else if conclusion.is_empty() {
            "-"
        } else {
            "ok"
        };
        MainCi { state: state.into(), workflow: st(r, "workflowName"), created_at: st(r, "createdAt") }
    }))
}

/// `owner/repo`, each part `[A-Za-z0-9_.-]+`.
pub fn valid_repo(repo: &str) -> bool {
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    matches!(repo.split_once('/'), Some((o, r)) if ok(o) && ok(r))
}

fn gh_bin() -> String {
    crate::env("GH").unwrap_or_else(|| "gh".into())
}

/// Run gh with a timeout; Ok(stdout) or Err(short error).
fn gh(args: &[&str]) -> Result<String, String> {
    let mut child = Command::new(gh_bin())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run gh ({e}) — install GitHub CLI and 'gh auth login'"))?;
    let mut out = child.stdout.take().ok_or("no stdout")?;
    let mut err = child.stderr.take().ok_or("no stderr")?;
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < CALL_TIMEOUT => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("gh {} timed out", args.first().unwrap_or(&"")));
            }
        }
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        let line = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("gh failed").trim();
        Err(line.chars().take(80).collect())
    }
}

/// Fetch a full snapshot (5 gh calls). Any failure -> Err(short message).
pub fn fetch(repo: &str, now: i64) -> Result<GhSnapshot, String> {
    use chrono::{Local, TimeZone};
    if !valid_repo(repo) {
        return Err(format!("bad repo '{repo}' — use 'tb config github owner/repo'"));
    }
    let today = Local
        .timestamp_opt(now, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    let prs = parse_prs(&gh(&[
        "pr", "list", "-R", repo, "--state", "open", "--limit", "20", "--json",
        "number,title,headRefName,isDraft,reviewDecision,statusCheckRollup,createdAt,author,closingIssuesReferences,updatedAt",
    ])?)?;
    let issues = parse_issues(&gh(&[
        "issue", "list", "-R", repo, "--state", "open", "--limit", "20", "--json",
        "number,title,labels,assignees,createdAt",
    ])?)?;
    let q = format!("search/issues?q=repo:{repo}+is:issue+is:open&per_page=1");
    let total = gh(&["api", &q, "--jq", ".total_count"])?;
    let issues_open = total.trim().parse::<i64>().map_err(|_| format!("bad issue count '{}'", total.trim()))?;
    let merged_today = parse_merged(&gh(&[
        "pr", "list", "-R", repo, "--state", "merged", "--search", &format!("merged:>={today}"),
        "--json", "number,title,mergedAt", "--limit", "50",
    ])?)?;
    let main_ci = parse_main_ci(&gh(&[
        "run", "list", "-R", repo, "--branch", "main", "--limit", "1", "--json",
        "status,conclusion,workflowName,createdAt",
    ])?)?;
    Ok(GhSnapshot { repo: repo.into(), fetched_at: now, issues_open, prs, issues, merged_today, main_ci })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoEntry {
    pub name_with_owner: String,
    pub description: String,
    pub pushed_at: String,
    pub is_private: bool,
    /// From the user's own `gh repo list` (vs an org's).
    #[serde(default)]
    pub own: bool,
}

/// `gh repo list --json nameWithOwner,description,pushedAt,isPrivate`
pub fn parse_repo_list(json: &str) -> Result<Vec<RepoEntry>, String> {
    Ok(arr(json, "repo list")?
        .iter()
        .map(|r| RepoEntry {
            name_with_owner: st(r, "nameWithOwner"),
            description: st(r, "description"),
            pushed_at: st(r, "pushedAt"),
            is_private: r.get("isPrivate").and_then(Value::as_bool).unwrap_or(false),
            own: false,
        })
        .filter(|r| !r.name_with_owner.is_empty())
        .collect())
}

/// Friendlier wording for the common gh failures.
fn explain(e: String) -> String {
    let l = e.to_ascii_lowercase();
    if l.contains("auth login") || l.contains("not logged") || l.contains("authentication") {
        "gh not logged in — run 'gh auth login'".into()
    } else {
        e
    }
}

const REPO_FIELDS: &str = "nameWithOwner,description,pushedAt,isPrivate";

/// The user's repos plus their orgs' repos, most recently pushed first.
pub fn list_repos() -> Result<Vec<RepoEntry>, String> {
    let mut all = parse_repo_list(&gh(&["repo", "list", "--limit", "200", "--json", REPO_FIELDS]).map_err(explain)?)?;
    for r in &mut all {
        r.own = true;
    }
    if let Ok(orgs) = gh(&["api", "user/orgs", "--jq", ".[].login"]) {
        for org in orgs.lines().map(str::trim).filter(|o| !o.is_empty()) {
            if let Ok(j) = gh(&["repo", "list", org, "--limit", "100", "--json", REPO_FIELDS]) {
                all.extend(parse_repo_list(&j).unwrap_or_default());
            }
        }
    }
    all.sort_by(|a, b| b.pushed_at.cmp(&a.pushed_at).then(a.name_with_owner.cmp(&b.name_with_owner)));
    all.dedup_by(|a, b| a.name_with_owner == b.name_with_owner);
    Ok(all)
}

/// Check a typed `owner/repo` exists (and is visible) via `gh repo view`.
pub fn check_repo(repo: &str) -> Result<String, String> {
    if !valid_repo(repo) {
        return Err(format!("'{repo}' is not owner/repo"));
    }
    let out = gh(&["repo", "view", repo, "--json", "nameWithOwner"]).map_err(|e| explain(format!("{repo}: {e}")))?;
    let v: Value = serde_json::from_str(&out).map_err(|_| format!("{repo}: unexpected gh output"))?;
    let name = st(&v, "nameWithOwner");
    if name.is_empty() {
        Err(format!("{repo}: not found"))
    } else {
        Ok(name)
    }
}

/// Picker grouping: own account(s) first, then orgs alphabetically; each group newest-pushed first.
pub fn group_repos<'a>(repos: impl IntoIterator<Item = &'a RepoEntry>) -> Vec<(String, Vec<&'a RepoEntry>)> {
    let mut groups: Vec<(String, bool, Vec<&RepoEntry>)> = Vec::new();
    for r in repos {
        let owner = r.name_with_owner.split('/').next().unwrap_or("").to_string();
        match groups.iter_mut().find(|g| g.0 == owner) {
            Some(g) => {
                g.1 |= r.own;
                g.2.push(r);
            }
            None => groups.push((owner, r.own, vec![r])),
        }
    }
    groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase())));
    groups
        .into_iter()
        .map(|(o, _, mut v)| {
            v.sort_by(|a, b| b.pushed_at.cmp(&a.pushed_at));
            (o, v)
        })
        .collect()
}

/// Fuzzy (in-order subsequence, case-insensitive) match on `owner/name`.
pub fn fuzzy(hay: &str, needle: &str) -> bool {
    let mut h = hay.chars().flat_map(char::to_lowercase);
    needle.chars().flat_map(char::to_lowercase).all(|n| h.any(|c| c == n))
}

/// Picker row text: `owner/name  (private)  pushed 2d  description`.
pub fn repo_row(r: &RepoEntry, now: i64) -> String {
    let private = if r.is_private { "  (private)" } else { "" };
    let pushed = age_of(&r.pushed_at, now).map(|a| format!("  pushed {}", crate::store::coarse_age(a))).unwrap_or_default();
    let desc = if r.description.is_empty() { String::new() } else { format!("  {}", r.description) };
    crate::text::sanitize(&format!("{}{private}{pushed}{desc}", r.name_with_owner))
}

/// Open/closed state of an issue or PR number (REST `repos/R/issues/N` covers both).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RefState {
    pub closed: bool,
    pub pr: bool,
    pub merged: bool,
}

/// Parse `gh api repos/R/issues/N`.
pub fn parse_ref_state(json: &str) -> Result<RefState, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad issue json: {e}"))?;
    let pr = v.get("pull_request").is_some_and(|p| !p.is_null());
    let merged = v
        .get("pull_request")
        .and_then(|p| p.get("merged_at"))
        .is_some_and(|m| m.as_str().is_some_and(|s| !s.is_empty()));
    Ok(RefState { closed: st(&v, "state") == "closed", pr, merged })
}

/// At most this many per-number state checks per sync.
pub const MAX_STATE_CHECKS: usize = 20;

/// Board gh_refs (not done) that the open lists can't vouch for: they may be closed/merged.
pub fn needs_state(s: &GhSnapshot, cards: &[crate::store::Card]) -> Vec<i64> {
    let mut v: Vec<i64> = cards
        .iter()
        .filter(|c| c.column != "done")
        .filter_map(|c| c.gh_ref)
        .filter(|n| !s.issues.iter().any(|i| i.number == *n) && !s.prs.iter().any(|p| p.number == *n))
        .collect();
    v.sort_unstable();
    v.dedup();
    v.truncate(MAX_STATE_CHECKS);
    v
}

/// Look up states with `gh api repos/R/issues/N` (failures are skipped).
pub fn fetch_states(repo: &str, nums: &[i64]) -> std::collections::HashMap<i64, RefState> {
    nums.iter()
        .filter_map(|n| {
            let out = gh(&["api", &format!("repos/{repo}/issues/{n}")]).ok()?;
            parse_ref_state(&out).ok().map(|s| (*n, s))
        })
        .collect()
}

/// A card GitHub evidence moves forward.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AutoMove {
    pub card_id: i64,
    pub gh_ref: i64,
    pub from: String,
    pub to: String,
    pub text: String,
}

/// Forward-only moves for cards with gh_ref: an open linked PR -> review (from todo/doing);
/// a merged PR or a closed issue -> done. Never backwards. A card a reviewer sent back
/// (`returned` = card id -> time of its last return) stays in DOING until its PR is
/// updated after that return.
pub fn plan_moves(
    s: &GhSnapshot,
    cards: &[crate::store::Card],
    states: &std::collections::HashMap<i64, RefState>,
    returned: &std::collections::HashMap<i64, i64>,
) -> Vec<AutoMove> {
    let mut out = Vec::new();
    for c in cards.iter().filter(|c| c.column != "done") {
        let Some(n) = c.gh_ref else { continue };
        let mv = |to: &str, text: String| AutoMove { card_id: c.id, gh_ref: n, from: c.column.clone(), to: to.into(), text };
        if let Some(st) = states.get(&n) {
            if st.pr && st.merged {
                out.push(mv("done", format!("github: PR #{n} merged → done")));
                continue;
            }
            if !st.pr && st.closed {
                out.push(mv("done", format!("github: issue #{n} closed → done")));
                continue;
            }
        }
        if c.column == "todo" || c.column == "doing" {
            // An UNOWNED card stays put: sync never moves work nobody took into REVIEW —
            // there it would have no owner and no author, so anyone could approve it and
            // nobody would be accountable. (Doing cards always have an owner; the hole is
            // a todo card linked to an issue that already has an open PR.)
            if c.owner.is_some() {
                let own_pr = s.prs.iter().find(|p| p.number == n);
                let linked = s.prs.iter().find(|p| p.closes.contains(&n)).or_else(|| s.prs.iter().find(|p| branch_matches(&p.head_ref, n)));
                if let Some(p) = own_pr.or(linked) {
                    let updated_since_return = match returned.get(&c.id) {
                        None => true,
                        Some(t) => unix_time(&p.updated_at).is_some_and(|u| u > *t),
                    };
                    if updated_since_return {
                        out.push(mv("review", format!("github: PR #{} open → review", p.number)));
                    }
                }
            }
        }
    }
    out
}

/// Is issue/PR `n` open according to the snapshot? (Evidence gate for a manual done.)
pub fn still_open(s: &GhSnapshot, n: i64) -> bool {
    s.issues.iter().any(|i| i.number == n) || s.prs.iter().any(|p| p.number == n)
}

/// Apply planned moves as actor `github`, logging the reason on each card.
pub fn apply_moves(store: &mut crate::store::Store, moves: &[AutoMove]) -> crate::store::Result<()> {
    for m in moves {
        store.move_to(m.card_id, &m.to, "github")?;
        store.note_kind(m.card_id, "github", &m.text, "github")?;
    }
    Ok(())
}

/// Seconds since an RFC 3339 timestamp (None if unparseable).
pub fn age_of(ts: &str, now: i64) -> Option<i64> {
    unix_time(ts).map(|t| now - t)
}

/// An RFC 3339 timestamp as Unix seconds (None if unparseable).
pub fn unix_time(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|t| t.timestamp())
}

/// `issues 42 open · PRs 5 open (1 draft) · merged today 4 · main CI ok` as (text, main_ci_failed).
pub fn summary(s: &GhSnapshot) -> (String, String) {
    let drafts = s.prs.iter().filter(|p| p.is_draft).count();
    let draft = if drafts > 0 { format!(" ({drafts} draft)") } else { String::new() };
    let head = format!(
        "issues {} open · PRs {} open{draft} · merged today {} · main CI ",
        s.issues_open,
        s.prs.len(),
        s.merged_today.len()
    );
    (head, s.main_ci.as_ref().map(|c| c.state.clone()).unwrap_or_else(|| "-".into()))
}

/// Panel title: drop a short `area: ` prefix, anything after ` — `, and trailing `(...)`.
pub fn short_title(t: &str) -> String {
    let mut t = t.trim();
    if let Some(i) = t.find(": ") {
        if i <= 30 {
            t = t[i + 2..].trim();
        }
    }
    if let Some(i) = t.find(" — ") {
        t = t[..i].trim();
    }
    while t.ends_with(')') {
        match t.rfind('(') {
            Some(i) if i > 0 => t = t[..i].trim_end(),
            _ => break,
        }
    }
    t.to_string()
}

/// Does `branch` name issue `num`? `(^|/)(fix|feat)?[/-]?<num>([-/]|$)`
pub fn branch_matches(branch: &str, num: i64) -> bool {
    let n = num.to_string();
    let mut from = 0;
    while let Some(off) = branch[from..].find(&n) {
        let i = from + off;
        let after = &branch[i + n.len()..];
        let end_ok = after.is_empty() || after.starts_with('-') || after.starts_with('/');
        let before = &branch[..i];
        let seg_ok = |b: &str| b.is_empty() || b.ends_with('/');
        let strip_kind = |b: &str| -> bool {
            seg_ok(b) || ["fix", "feat"].iter().any(|k| b.strip_suffix(k).is_some_and(seg_ok))
        };
        let start_ok = strip_kind(before)
            || before.strip_suffix('/').is_some_and(strip_kind)
            || before.strip_suffix('-').is_some_and(strip_kind);
        if end_ok && start_ok {
            return true;
        }
        from = i + 1;
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StateKind {
    Pr,
    InProgress,
    OnBoard,
    Unclaimed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IssueRow {
    pub number: i64,
    pub title: String,
    pub labels: Vec<String>,
    pub created_at: String,
    pub kind: StateKind,
    /// `PR #335 FAIL`, `in progress`, `on board`, `unclaimed`
    pub state: String,
    /// CI of the linked PR (for colouring `FAIL`)
    pub pr_ci: Option<String>,
    pub who: String,
}

/// The factory view of a snapshot against this board's cards.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Factory {
    /// PRs first, then in progress, on board, unclaimed; newest first within each.
    pub issues: Vec<IssueRow>,
    /// Per open PR (same order as the snapshot): `(issue, who)` when linked.
    pub pr_links: Vec<Option<(i64, String)>>,
    pub unclaimed: usize,
    pub new_today: usize,
    pub failing: usize,
    pub running: usize,
    pub drafts: usize,
}

fn local_date(ts: i64) -> Option<chrono::NaiveDate> {
    use chrono::{Local, TimeZone};
    Local.timestamp_opt(ts, 0).single().map(|t| t.date_naive())
}

pub fn factory(s: &GhSnapshot, cards: &[crate::store::Card], now: i64) -> Factory {
    let card_for = |n: i64| cards.iter().find(|c| c.gh_ref == Some(n));
    let pr_issue = |p: &Pr| -> Option<i64> {
        p.closes
            .first()
            .copied()
            .or_else(|| s.issues.iter().find(|i| branch_matches(&p.head_ref, i.number)).map(|i| i.number))
    };
    let who_for = |issue: i64, p: &Pr| {
        card_for(issue).and_then(|c| c.owner.clone()).unwrap_or_else(|| p.author.clone())
    };
    let pr_links = s.prs.iter().map(|p| pr_issue(p).map(|i| (i, who_for(i, p)))).collect();
    let mut issues: Vec<IssueRow> = s
        .issues
        .iter()
        .map(|i| {
            let pr = s.prs.iter().find(|p| p.closes.contains(&i.number))
                .or_else(|| s.prs.iter().find(|p| branch_matches(&p.head_ref, i.number)));
            let card = card_for(i.number);
            let (kind, state, pr_ci, who) = match (pr, card) {
                (Some(p), _) => {
                    let st = if p.ci == "-" { format!("PR #{}", p.number) } else { format!("PR #{} {}", p.number, p.ci) };
                    (StateKind::Pr, st, Some(p.ci.clone()), who_for(i.number, p))
                }
                (None, Some(c)) if c.column == "doing" || c.column == "review" => {
                    (StateKind::InProgress, "in progress".into(), None, c.owner.clone().unwrap_or_else(|| "-".into()))
                }
                (None, Some(c)) if c.column == "todo" => {
                    (StateKind::OnBoard, "on board".into(), None, c.owner.clone().unwrap_or_else(|| "-".into()))
                }
                _ => (StateKind::Unclaimed, "unclaimed".into(), None, "-".into()),
            };
            IssueRow {
                number: i.number,
                title: i.title.clone(),
                labels: i.labels.clone(),
                created_at: i.created_at.clone(),
                kind,
                state,
                pr_ci,
                who,
            }
        })
        .collect();
    issues.sort_by(|a, b| a.kind.cmp(&b.kind).then(b.created_at.cmp(&a.created_at)));
    let today = local_date(now);
    let new_today = s
        .issues
        .iter()
        .filter(|i| {
            chrono::DateTime::parse_from_rfc3339(&i.created_at)
                .ok()
                .and_then(|t| local_date(t.timestamp()))
                == today
        })
        .count();
    Factory {
        unclaimed: issues.iter().filter(|r| r.kind == StateKind::Unclaimed).count(),
        issues,
        pr_links,
        new_today,
        failing: s.prs.iter().filter(|p| p.ci == "FAIL").count(),
        running: s.prs.iter().filter(|p| p.ci == "run").count(),
        drafts: s.prs.iter().filter(|p| p.is_draft).count(),
    }
}

/// Segments of the one-line summary; `true` = red (only `FAIL`).
/// `ISSUES 11 (+2, 5 unclaimed) · PRS 1 (1 FAIL) · MERGED 6 · MAIN ok`
pub fn compact_summary(s: &GhSnapshot, f: &Factory) -> Vec<(String, bool)> {
    let mut v = vec![(format!("ISSUES {} (+{}, {} unclaimed) · PRS {}", s.issues_open, f.new_today, f.unclaimed, s.prs.len()), false)];
    if f.failing > 0 {
        v.push((format!(" ({} ", f.failing), false));
        v.push(("FAIL".into(), true));
        v.push((")".into(), false));
    }
    v.push((format!(" · MERGED {} · MAIN ", s.merged_today.len()), false));
    let main = s.main_ci.as_ref().map(|c| c.state.clone()).unwrap_or_else(|| "-".into());
    let red = main == "FAIL";
    v.push((main, red));
    v
}

/// The four tiles as (title, value, line 2). Value `FAIL` is the only red.
pub fn tiles(s: &GhSnapshot, f: &Factory, now: i64) -> [(String, String, String); 4] {
    let age = |ts: &str| age_of(ts, now).map(crate::store::fmt_age).unwrap_or_else(|| "?".into());
    let drafts = if f.drafts > 0 { format!(" ({} draft)", f.drafts) } else { String::new() };
    let pr2 = if f.failing > 0 {
        format!("{} failing CI", f.failing)
    } else if f.running > 0 {
        format!("{} running", f.running)
    } else if s.prs.is_empty() {
        "none open".into()
    } else {
        "all green".into()
    };
    let last = s.merged_today.iter().max_by(|a, b| a.merged_at.cmp(&b.merged_at));
    let merged2 = last.map_or("none yet".into(), |m| format!("last: #{} {} ago", m.number, age(&m.merged_at)));
    let (ci, ci2) = match &s.main_ci {
        Some(c) => (c.state.clone(), format!("{} · {}", c.workflow, age(&c.created_at))),
        None => ("-".into(), "no runs".into()),
    };
    let issues2 = if s.issues_open == 0 && s.prs.is_empty() {
        "no open issues or PRs".to_string()
    } else {
        format!("+{} today · {} unclaimed", f.new_today, f.unclaimed)
    };
    [
        ("ISSUES".into(), format!("{} open", s.issues_open), issues2),
        ("PULL REQUESTS".into(), format!("{} open{drafts}", s.prs.len()), pr2),
        ("MERGED".into(), format!("{} today", s.merged_today.len()), merged2),
        ("MAIN CI".into(), ci, ci2),
    ]
}

/// Plain-text snapshot for `ttyboard github` (up to `n` PRs/issues, full titles).
pub fn text(s: &GhSnapshot, cards: &[crate::store::Card], error: Option<&str>, n: usize, now: i64) -> String {
    let mut out = String::new();
    // every line through the sanitizer: GitHub titles, names and branches are remote text
    macro_rules! ln {
        ($($a:tt)*) => { crate::text::push_line(&mut out, &format!($($a)*)) };
    }
    let f = factory(s, cards, now);
    ln!("GITHUB · {} · synced {}", s.repo, crate::store::fmt_clock(s.fetched_at));
    if let Some(e) = error {
        ln!("github: {e} (showing the last good snapshot)");
    }
    let (head, ci) = summary(s);
    ln!("{head}{ci} · {} unclaimed · +{} today", f.unclaimed, f.new_today);
    let age = |ts: &str| age_of(ts, now).map(crate::store::fmt_age).unwrap_or_else(|| "?".into());
    ln!("\nOPEN PRS");
    for (p, link) in s.prs.iter().zip(&f.pr_links).take(n) {
        let draft = if p.is_draft { " (draft)" } else { "" };
        let link = link.as_ref().map(|(i, w)| format!(" -> #{i} ({w})")).unwrap_or_default();
        ln!(
            "  #{} {}{draft} · CI {} · review {} · {} · {} · {}{link}",
            p.number,
            p.title,
            p.ci,
            p.review,
            age(&p.created_at),
            p.author,
            p.head_ref
        );
    }
    ln!("\nISSUES (state · who)");
    for r in f.issues.iter().take(n) {
        let labels = if r.labels.is_empty() { String::new() } else { format!(" [{}]", r.labels.join(",")) };
        ln!("  #{} {}{labels} · {} · {} · {}", r.number, r.title, r.state, r.who, age(&r.created_at));
    }
    if f.issues.len() > n {
        ln!("  +{} more", f.issues.len() - n);
    }
    ln!("\nMERGED TODAY");
    for m in s.merged_today.iter().take(n) {
        ln!("  #{} {}", m.number, m.title);
    }
    if let Some(c) = &s.main_ci {
        ln!("\nMAIN CI {} · {} · {}", c.state, c.workflow, age(&c.created_at));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rollup_mixes() {
        let ok = json!({"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"});
        let skip = json!({"__typename":"CheckRun","status":"COMPLETED","conclusion":"SKIPPED"});
        let fail = json!({"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"});
        let pend = json!({"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""});
        let ctx_ok = json!({"__typename":"StatusContext","state":"SUCCESS"});
        let ctx_pend = json!({"__typename":"StatusContext","state":"PENDING"});
        let ctx_err = json!({"__typename":"StatusContext","state":"ERROR"});
        assert_eq!(rollup(&json!([ok, skip, ctx_ok])), "ok");
        assert_eq!(rollup(&json!([ok, fail, pend])), "FAIL", "any failure wins");
        assert_eq!(rollup(&json!([ok, pend])), "run");
        assert_eq!(rollup(&json!([ok, ctx_pend])), "run");
        assert_eq!(rollup(&json!([ctx_err])), "FAIL");
        assert_eq!(rollup(&json!([])), "-");
        assert_eq!(rollup(&Value::Null), "-");
    }

    #[test]
    fn reviews_and_repos() {
        assert_eq!(review(Some("APPROVED")), "ok");
        assert_eq!(review(Some("CHANGES_REQUESTED")), "chg");
        assert_eq!(review(Some("REVIEW_REQUIRED")), "-");
        assert_eq!(review(None), "-");
        assert!(valid_repo("acme/widgets"));
        assert!(!valid_repo("widgets") && !valid_repo("a/b/c") && !valid_repo("a/"));
    }

    #[test]
    fn main_ci_states() {
        let one = |s: &str, c: &str| {
            parse_main_ci(&json!([{"status":s,"conclusion":c,"workflowName":"ci","createdAt":"2026-09-18T10:00:00Z"}]).to_string())
                .unwrap()
                .unwrap()
                .state
        };
        assert_eq!(one("completed", "success"), "ok");
        assert_eq!(one("completed", "failure"), "FAIL");
        assert_eq!(one("in_progress", ""), "run");
        assert_eq!(parse_main_ci("[]").unwrap(), None);
        assert!(parse_main_ci("{").is_err());
    }

    #[test]
    fn sync_suffix_quiet_then_red() {
        let net = "Post \"https://api.github.com/graphql\": net/http: TLS handshake timeout";
        let other = "HTTP 401: Bad credentials (https://api.github.com)";
        assert_eq!(sync_suffix(None, 0), (String::new(), false));
        assert_eq!(sync_suffix(Some(net), 0), (" · offline, retrying".into(), false));
        assert_eq!(sync_suffix(Some(other), 0), (" · gh error".into(), false));
        assert_eq!(sync_suffix(Some(net), 2), (" · offline, retrying".into(), false), "not red yet");
        assert_eq!(sync_suffix(Some(other), RED_AFTER_FAILS), (" · gh error".into(), true), "red at 3");
        assert_eq!(sync_suffix(Some(net), 10), (" · offline, retrying".into(), true), "still red past the threshold");
        // gh's own offline message and Go's lower-case network errors, as gh prints them
        for e in [
            "error connecting to api.github.com",
            "Post \"https://api.github.com/graphql\": dial tcp: lookup api.github.com: no such host",
            "Post \"https://api.github.com/graphql\": dial tcp 127.0.0.1:443: connect: network is unreachable",
            "Post \"https://api.github.com/graphql\": read tcp 127.0.0.1:51234->127.0.0.1:443: read: connection reset by peer",
            "Post \"https://api.github.com/graphql\": write tcp 127.0.0.1:51234->127.0.0.1:443: write: broken pipe",
            "Post \"https://api.github.com/graphql\": dial tcp 127.0.0.1:443: connect: no route to host",
            "Post \"https://api.github.com/graphql\": dial tcp 127.0.0.1:443: i/o timeout",
            "Post \"https://api.github.com/graphql\": dial tcp: lookup api.github.com on 127.0.0.1:53: Temporary failure in name resolution",
            "Get \"https://api.github.com/zen\": dial tcp 127.0.0.1:443: connect: Connection Refused",
            "NETWORK IS UNREACHABLE",
        ] {
            assert_eq!(sync_suffix(Some(e), 0), (" · offline, retrying".into(), false), "{e}");
        }
        for e in ["HTTP 404: Not Found (https://api.github.com/repos/acme/nope)", "GraphQL: Could not resolve to a Repository"] {
            assert_eq!(sync_suffix(Some(e), 0), (" · gh error".into(), false), "{e}");
        }
    }

    #[test]
    fn save_github_counts_consecutive_failures() {
        use crate::store::Store;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("b.db");
        let s = Store::open(&db).unwrap();
        s.set_github(Some("acme/widgets")).unwrap();
        let (json, error, fails) = s.github_cache().unwrap();
        assert_eq!((json.is_none(), error.is_none(), fails), (true, true, 0));
        for want in 1..=5 {
            s.save_github(&Err(format!("boom {want}"))).unwrap();
            let (_, error, fails) = s.github_cache().unwrap();
            assert_eq!(fails, want);
            assert_eq!(error.as_deref(), Some(format!("boom {want}").as_str()));
        }
        s.save_github(&Ok(GhSnapshot { repo: "acme/widgets".into(), fetched_at: 100, ..Default::default() })).unwrap();
        let (json, error, fails) = s.github_cache().unwrap();
        assert_eq!(fails, 0, "success resets the counter");
        assert!(error.is_none());
        let snap: GhSnapshot = serde_json::from_str(&json.unwrap()).unwrap();
        assert_eq!(snap.fetched_at, 100);
        let view = s.github_view().unwrap();
        assert_eq!(view.fails, 0);
        assert_eq!(view.snap.as_ref().map(|s| s.fetched_at), Some(100));
    }
}
