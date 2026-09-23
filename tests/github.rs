//! GitHub panel + CLI, no network: fixtures and a fake `gh` script (TB_GH).
mod common;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::Terminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use terminal_board::github::{
    branch_matches, factory, parse_issues, parse_main_ci, parse_merged, parse_prs, short_title, tiles, GhSnapshot,
    GhView, StateKind, RED_AFTER_FAILS,
};
use terminal_board::herdr::AgentsState;
use terminal_board::store::Store;
use terminal_board::tui::{draw, App};

fn ago(secs: i64) -> String {
    chrono::DateTime::from_timestamp(terminal_board::store::now() - secs, 0).unwrap().to_rfc3339()
}

fn prs_json() -> String {
    format!(
        r#"[
 {{"number":332,"title":"misc: bump deps","headRefName":"chore/deps","isDraft":false,"reviewDecision":"APPROVED",
  "statusCheckRollup":[{{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"}},{{"__typename":"StatusContext","state":"SUCCESS"}}],
  "createdAt":"{d2}","author":{{"login":"rev"}},"closingIssuesReferences":[]}},
 {{"number":335,"title":"api: rate limit ignores burst setting","headRefName":"fix/315","isDraft":false,"reviewDecision":"CHANGES_REQUESTED",
  "statusCheckRollup":[{{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"}},{{"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"}}],
  "createdAt":"{m28}","author":{{"login":"bot"}},"closingIssuesReferences":[{{"number":315}}]}},
 {{"number":333,"title":"search: facets","headRefName":"feat-310-facets","isDraft":true,"reviewDecision":"",
  "statusCheckRollup":[{{"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""}}],
  "createdAt":"{h1}","author":{{"login":"bot-3"}}}},
 {{"number":331,"title":"no checks","headRefName":"x","isDraft":false,"reviewDecision":null,
  "statusCheckRollup":[],"createdAt":"{d3}","author":{{"login":"me"}}}}
]"#,
        d2 = ago(2 * 86400),
        m28 = ago(28 * 60),
        h1 = ago(3600 + 42 * 60),
        d3 = ago(3 * 86400)
    )
}

fn issues_json() -> String {
    format!(
        r#"[
 {{"number":334,"title":"ui: totals overflow on wide tables (#315)","labels":[{{"name":"bug"}}],"assignees":[],"createdAt":"{h1}"}},
 {{"number":327,"title":"login form rejects plus-addresses","labels":[{{"name":"bug"}}],"assignees":[],"createdAt":"{h10}"}},
 {{"number":315,"title":"rate limit resets too early — see ticket 4","labels":[{{"name":"bug"}},{{"name":"enhancement"}}],"assignees":[{{"login":"bot"}}],"createdAt":"{d1}"}},
 {{"number":310,"title":"search: facets missing","labels":[],"assignees":[],"createdAt":"{d2}"}},
 {{"number":309,"title":"retry backoff","labels":[],"assignees":[],"createdAt":"{d5}"}},
 {{"number":301,"title":"old unclaimed thing","labels":[],"assignees":[],"createdAt":"{d6}"}}
]"#,
        h1 = ago(3600),
        h10 = ago(10 * 3600),
        d1 = ago(86400 + 60),
        d2 = ago(2 * 86400),
        d5 = ago(5 * 86400),
        d6 = ago(6 * 86400)
    )
}

fn merged_json() -> String {
    format!(
        r#"[{{"number":330,"title":"dark theme","mergedAt":"{a}"}},{{"number":333,"title":"div","mergedAt":"{b}"}}]"#,
        a = ago(3 * 3600),
        b = ago(12 * 60)
    )
}

fn run_json(conclusion: &str) -> String {
    format!(r#"[{{"status":"completed","conclusion":"{conclusion}","workflowName":"ci-main","createdAt":"{}"}}]"#, ago(41 * 60))
}

fn snapshot(conclusion: &str) -> GhSnapshot {
    GhSnapshot {
        repo: "acme/widgets".into(),
        fetched_at: terminal_board::store::now(),
        issues_open: 11,
        prs: parse_prs(&prs_json()).unwrap(),
        issues: parse_issues(&issues_json()).unwrap(),
        merged_today: parse_merged(&merged_json()).unwrap(),
        main_ci: parse_main_ci(&run_json(conclusion)).unwrap(),
    }
}

/// Board with cards for #327 (doing, bot-2), #309 (todo), #315 (doing, bot-5).
fn board() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("b.db")).unwrap();
    let a = s.add("widgets: gh#327 login form rete args", "", &[], "me").unwrap();
    s.take(a, "bot-2").unwrap();
    s.add("widgets: gh#309 retry backoff", "", &[], "me").unwrap();
    let c = s.add("widgets: gh#315 flags", "", &[], "me").unwrap();
    s.take(c, "bot-5").unwrap();
    (dir, s)
}

#[test]
fn parses_every_gh_shape() {
    common::pin_clock();
    let prs = parse_prs(&prs_json()).unwrap();
    let nums: Vec<i64> = prs.iter().map(|p| p.number).collect();
    assert_eq!(nums, [335, 333, 332, 331], "newest first");
    let ci: Vec<&str> = prs.iter().map(|p| p.ci.as_str()).collect();
    assert_eq!(ci, ["FAIL", "run", "ok", "-"]);
    let rv: Vec<&str> = prs.iter().map(|p| p.review.as_str()).collect();
    assert_eq!(rv, ["chg", "-", "ok", "-"]);
    assert_eq!(prs[0].closes, [315]);
    assert!(prs[1].closes.is_empty() && prs[1].is_draft);
    let issues = parse_issues(&issues_json()).unwrap();
    assert_eq!(issues[2].labels, ["bug", "enhancement"]);
    assert_eq!(parse_merged(&merged_json()).unwrap().len(), 2);
    assert_eq!(parse_main_ci(&run_json("success")).unwrap().unwrap().state, "ok");
    assert_eq!(parse_main_ci(&run_json("failure")).unwrap().unwrap().state, "FAIL");
    assert!(parse_prs("{}").is_err() && parse_issues("nope").is_err());
}

#[test]
fn short_titles() {
    common::pin_clock();
    let cases = [
        ("api: rate limit ignores burst setting", "rate limit ignores burst setting"),
        ("ui: totals overflow on wide tables (#315)", "totals overflow on wide tables"),
        ("rate limit resets too early — see ticket 4", "rate limit resets too early"),
        ("search: facets", "facets"),
        ("login form rejects plus-addresses", "login form rejects plus-addresses"),
        ("a very long area name that is more than thirty chars: keep", "a very long area name that is more than thirty chars: keep"),
        ("fix (a) and (b)", "fix (a) and"),
        ("(only a parenthetical)", "(only a parenthetical)"),
    ];
    for (full, want) in cases {
        assert_eq!(short_title(full), want, "{full}");
    }
}

#[test]
fn branch_fallback() {
    common::pin_clock();
    for b in ["fix/315", "fix/315-flags", "fix-315", "315", "bot/315-x", "feat315", "feat/315/x"] {
        assert!(branch_matches(b, 315), "{b}");
    }
    for b in ["fix/41040", "fix/14104", "x315", "fix/315a", "misc"] {
        assert!(!branch_matches(b, 315), "{b}");
    }
}

#[test]
fn state_derivation_matrix() {
    common::pin_clock();
    let (_d, s) = board();
    let f = factory(&snapshot("success"), &s.list().unwrap(), terminal_board::store::now());
    let by = |n: i64| f.issues.iter().find(|r| r.number == n).unwrap().clone();
    // closingIssuesReferences; WHO = board card owner over PR author
    let r = by(315);
    assert_eq!((r.kind, r.state.as_str(), r.who.as_str()), (StateKind::Pr, "PR gh#335 FAIL", "bot-5"));
    // branch fallback (feat-310-facets), WHO = PR author (no card)
    let r = by(310);
    assert_eq!((r.kind, r.state.as_str(), r.who.as_str()), (StateKind::Pr, "PR gh#333 run", "bot-3"));
    // board card in doing
    let r = by(327);
    assert_eq!((r.kind, r.state.as_str(), r.who.as_str()), (StateKind::InProgress, "in progress", "bot-2"));
    // board card in todo
    let r = by(309);
    assert_eq!((r.kind, r.state.as_str(), r.who.as_str()), (StateKind::OnBoard, "on board", "-"));
    // nothing
    let r = by(334);
    assert_eq!((r.kind, r.state.as_str(), r.who.as_str()), (StateKind::Unclaimed, "unclaimed", "-"));
    assert_eq!(f.unclaimed, 2);
    // order: PRs, in progress, on board, unclaimed; newest first within
    let order: Vec<i64> = f.issues.iter().map(|r| r.number).collect();
    assert_eq!(order, [315, 310, 327, 309, 334, 301]);
    // PR -> issue links
    assert_eq!(f.pr_links[0], Some((315, "bot-5".into())));
    assert_eq!(f.pr_links[1], Some((310, "bot-3".into())));
    assert_eq!(f.pr_links[2], None);
    // no board cards: WHO falls back to the PR author
    let f = factory(&snapshot("success"), &[], terminal_board::store::now());
    assert_eq!(f.issues[0].who, "bot");
    assert_eq!(f.unclaimed, 4);
}

#[test]
fn tile_contents() {
    common::pin_clock();
    let (_d, s) = board();
    let snap = snapshot("failure");
    let now = terminal_board::store::now();
    let f = factory(&snap, &s.list().unwrap(), now);
    let t = tiles(&snap, &f, now);
    assert_eq!(t[0], ("ISSUES".into(), "11 open".into(), format!("+{} today · 2 unclaimed", f.new_today)));
    assert!(f.new_today >= 1, "the 1h-old issue is from today (unless run just after midnight)");
    assert_eq!(t[1], ("PULL REQUESTS".into(), "4 open (1 draft)".into(), "1 failing CI".into()));
    assert_eq!(t[2], ("MERGED".into(), "2 today".into(), "last: gh#333 12m ago".into()));
    assert_eq!(t[3], ("MAIN CI".into(), "FAIL".into(), "ci-main · 41m".into()));
}

fn screen_of(app: &App, w: u16, h: u16) -> (String, Buffer) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    let text = buf
        .content
        .chunks(w as usize)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    (text, buf)
}

fn red_runs(buf: &Buffer) -> Vec<String> {
    let mut runs = Vec::new();
    for y in 0..buf.area.height {
        let mut run = String::new();
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            if c.fg == Color::Red {
                run.push_str(c.symbol());
            } else if !run.is_empty() {
                runs.push(std::mem::take(&mut run));
            }
        }
        if !run.is_empty() {
            runs.push(run);
        }
    }
    runs
}

fn board_app(view: GhView) -> (tempfile::TempDir, App) {
    let (d, s) = board();
    let mut app = App::new(s.snapshot().unwrap(), "me");
    app.agents = AgentsState::Unavailable("herdr not available".into());
    app.gh = view;
    (d, app)
}

fn on(snap: Option<GhSnapshot>, error: Option<&str>) -> GhView {
    GhView { repo: Some("acme/widgets".into()), snap, error: error.map(str::to_string), fails: error.is_some() as i64 }
}

/// Column x of `needle` in the first line containing `anchor`.
fn col_in(screen: &str, anchor: &str, needle: &str) -> usize {
    let l = screen.lines().find(|l| l.contains(anchor)).unwrap_or_else(|| panic!("{anchor}:\n{screen}"));
    l[..l.find(needle).unwrap_or_else(|| panic!("{needle} in {l}"))].chars().count()
}

#[test]
fn panel_tiles_tables_and_only_fail_is_red() {
    common::pin_clock();
    for w in [120u16, 160] {
        let (_d, app) = board_app(on(Some(snapshot("success")), None));
        let (screen, buf) = screen_of(&app, w, 50);
        for want in [
            "GITHUB · acme/widgets · synced",
            "ISSUES  11 open",
            "unclaimed",
            "4 open (1 draft)",
            "1 failing CI",
            "MERGED  2 today",
            "last: gh#333 12m ago",
            "MAIN CI  ok",
            "ci-main · 41m",
            "fix/315 -> gh#315 (bot-5)",
            "PR gh#335 FAIL",
            "in progress",
            "bot-2",
            "rate limit ignores burst",
        ] {
            assert!(screen.contains(want), "{w}: missing {want:?}:\n{screen}");
        }
        assert!(!screen.contains("ir emit:") && !screen.contains("(#315)"), "titles shortened");
        // tiles are 4 boxes on one row
        let tile_row = screen.lines().find(|l| l.contains("ISSUES  11 open")).unwrap();
        assert!((tile_row.contains("PRS") || tile_row.contains("PULL REQUESTS")) && tile_row.contains("MERGED") && tile_row.contains("MAIN CI"), "{w}: {tile_row}");
        // tables aligned: header columns line up with the cells below
        let ci_h = col_in(&screen, "BRANCH / ISSUE", "CI ");
        let ci_c = col_in(&screen, "│ gh#335  ", "FAIL");
        assert_eq!(ci_h, ci_c, "{w}: CI column aligned");
        let st_h = col_in(&screen, "LABELS", "STATE");
        let st_c = col_in(&screen, "│ gh#327  ", "in progress");
        assert_eq!(st_h, st_c, "{w}: STATE column aligned");
        let who_h = col_in(&screen, "LABELS", "WHO");
        let who_c = col_in(&screen, "│ gh#327  ", "bot-2");
        assert_eq!(who_h, who_c, "{w}: WHO column aligned");
        // header rows bold
        let (x, y) = (ci_h as u16, screen.lines().position(|l| l.contains("BRANCH / ISSUE")).unwrap() as u16);
        assert!(buf[(x, y)].modifier.contains(ratatui::style::Modifier::BOLD));
        let mut runs = red_runs(&buf);
        runs.sort();
        runs.dedup();
        assert_eq!(runs, ["FAIL"], "{w}: only FAIL is red");
        // coarse ages in the AGE columns: 1h42m -> 1h, 10h -> 10h, 1d+ -> 1d
        let pr_row = screen.lines().find(|l| l.contains("│ gh#333  ")).unwrap();
        assert!(pr_row.contains(" 1h ") && !pr_row.contains("1h42"), "{pr_row}");
        let age_h = col_in(&screen, "BRANCH / ISSUE", "AGE");
        assert_eq!(col_in(&screen, "│ gh#333  ", " 1h ") + 1, age_h, "{w}: AGE aligned");
        let is_row = screen.lines().find(|l| l.contains("│ gh#327  ")).unwrap();
        assert!(is_row.contains(" 10h "), "{is_row}");
        // ordering + more line
        let r2104 = screen.lines().position(|l| l.contains("│ gh#315  ")).unwrap();
        let r2116 = screen.lines().position(|l| l.contains("│ gh#327  ")).unwrap();
        let r2123 = screen.lines().position(|l| l.contains("│ gh#334  ")).unwrap_or(usize::MAX);
        assert!(r2104 < r2116 && r2116 < r2123, "linked, then in progress, then unclaimed");
    }
    // main CI failing: tile value FAIL red too
    let (_d, app) = board_app(on(Some(snapshot("failure")), None));
    let (screen, buf) = screen_of(&app, 160, 50);
    assert!(screen.contains("MAIN CI  FAIL"));
    assert!(red_runs(&buf).iter().all(|r| r == "FAIL"));
    assert!(red_runs(&buf).len() >= 3);
}

#[test]
fn panel_more_line_and_compact_fallback() {
    common::pin_clock();
    let (_d, app) = board_app(on(Some(snapshot("success")), None));
    // full layout 160x44: panel 14 rows -> tiles + a few rows + "+N more"
    let (screen, _) = screen_of(&app, 160, 44);
    assert!(screen.contains("more issues · tb github for all"), "{screen}");
    // a third of the height (110x22): the rail's tidy GITHUB block with stat rows and rows
    let (screen, _) = screen_of(&app, 110, 22);
    assert!(screen.contains("GITHUB · widgets"), "{screen}");
    assert!(screen.contains("ISSUES  11 open") && screen.contains("MERGED    2 today") && screen.contains("MAIN CI  ok"), "{screen}");
    assert!(screen.contains("PR    gh#335") && screen.contains("CI FAIL"), "{screen}");
}

#[test]
fn panel_hidden_when_unconfigured_toggled_or_narrow() {
    common::pin_clock();
    let (_d, mut app) = board_app(GhView::default());
    let (screen, _) = screen_of(&app, 140, 45);
    assert!(screen.contains("GITHUB") && screen.contains("no repo — enter to pick one"), "discoverable:\n{screen}");
    assert!(!screen.contains("G github") && screen.contains("R github: pick repo"));
    app.show_github = false;
    let (screen, _) = screen_of(&app, 140, 45);
    assert!(!screen.contains("GITHUB"), "G hides it entirely");
    let (_d, mut app) = board_app(on(Some(snapshot("success")), None));
    app.show_github = false;
    let (screen, _) = screen_of(&app, 140, 45);
    assert!(!screen.contains("GITHUB ·") && screen.contains("? help"), "G hides it");
    app.show_github = true;
    // half-v (90x45, taller than wide): the panel shows full width below the 2x2 grid
    let (screen, _) = screen_of(&app, 90, 45);
    assert!(screen.contains("GITHUB ·"), "{screen}");
}

#[test]
fn panel_hiccup_keeps_layout_and_goes_red_only_after_three() {
    common::pin_clock();
    // one-off failure: no extra row, last good snapshot stays, quiet words in the header
    let (_d, app) = board_app(on(Some(snapshot("success")), Some("gh pr timed out")));
    let (screen, buf) = screen_of(&app, 160, 50);
    assert!(screen.contains("synced") && screen.contains("offline, retrying"), "{screen}");
    assert!(!screen.contains("github: gh pr timed out"), "no raw error row:\n{screen}");
    assert!(screen.contains("PULL REQUESTS"), "stale data stays:\n{screen}");
    assert!(red_runs(&buf).iter().all(|r| r == "FAIL"), "1 failure: nothing new is red:\n{screen}");
    // a non-network error gets the other quiet wording
    let (_d, app) = board_app(on(Some(snapshot("success")), Some("HTTP 401: Bad credentials")));
    let (screen, _) = screen_of(&app, 160, 50);
    assert!(screen.contains("gh error") && !screen.contains("offline, retrying"), "{screen}");
    // never-synced: still no extra row — the header carries the quiet wording; the raw
    // error lives in `tb github` / `--json`
    let (_d, app) = board_app(on(None, Some("HTTP 401: Bad credentials")));
    let (screen, _) = screen_of(&app, 140, 45);
    assert!(screen.contains("synced never · gh error") && !screen.contains("github: HTTP 401"), "{screen}");
}

#[test]
fn panel_third_consecutive_failure_turns_header_red() {
    common::pin_clock();
    let base = GhView {
        repo: Some("acme/widgets".into()),
        snap: Some(snapshot("success")),
        error: Some("Post \"https://api.github.com/graphql\": net/http: TLS handshake timeout".into()),
        fails: RED_AFTER_FAILS,
    };
    let (_d, app) = board_app(base.clone());
    let (screen, buf) = screen_of(&app, 160, 50);
    assert!(screen.contains("offline, retrying"), "{screen}");
    let reds = red_runs(&buf);
    assert!(reds.iter().any(|r| r.contains("offline, retrying")), "header is red now: {reds:?}\n{screen}");
    // and not at 2
    let (_d, app) = board_app(GhView { fails: RED_AFTER_FAILS - 1, ..base });
    let (screen, buf) = screen_of(&app, 160, 50);
    assert!(red_runs(&buf).iter().all(|r| r == "FAIL"), "2 failures: header not red yet:\n{screen}");
}

// ---------- CLI ----------

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        Env { dir: tempfile::tempdir().unwrap() }
    }
    fn db(&self) -> PathBuf {
        self.dir.path().join("b.db")
    }
    fn fake_gh(&self, run: &str, fail: bool) -> PathBuf {
        let d = self.dir.path();
        for (f, body) in [("prs.json", prs_json()), ("issues.json", issues_json()), ("merged.json", merged_json()), ("run.json", run.to_string())] {
            std::fs::write(d.join(f), body).unwrap();
        }
        let script = if fail {
            "#!/bin/sh\necho 'HTTP 401: Bad credentials (https://api.github.com)' >&2\nexit 1\n".to_string()
        } else {
            let p = d.display();
            format!(
                "#!/bin/sh\necho \"$*\" >> {p}/calls.log\ncase \"$1 $2\" in\n  \"repo view\") echo '{{\"nameWithOwner\":\"acme/widgets\"}}';;\n  \"pr list\") case \"$*\" in *merged*) cat {p}/merged.json;; *) cat {p}/prs.json;; esac;;\n  \"issue list\") cat {p}/issues.json;;\n  \"run list\") cat {p}/run.json;;\n  api*) echo 42;;\n  *) exit 2;;\nesac\n"
            )
        };
        let path = d.join(if fail { "gh-fail" } else { "gh-ok" });
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }
    fn run(&self, args: &[&str], gh: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tb"))
            .args(args)
            .env("TB_DB", self.db())
            .env("TB_GH", gh)
            .env("TB_AS", "tester")
            .env("TB_NO_HERDR", "1")
            .output()
            .unwrap()
    }
    fn calls(&self) -> usize {
        std::fs::read_to_string(self.dir.path().join("calls.log")).map(|s| s.lines().count()).unwrap_or(0)
    }
}

fn keys(v: &serde_json::Value) -> Vec<String> {
    use std::collections::BTreeSet;
    v.as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

fn sorted(list: &[&str]) -> Vec<String> {
    use std::collections::BTreeSet;
    list.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>().into_iter().collect()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}
fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn config_and_off_state() {
    common::pin_clock();
    let e = Env::new();
    let none = Path::new("/nonexistent/gh");
    assert!(e.run(&["add", "x"], none).status.success());
    let o = e.run(&["github"], none);
    assert!(!o.status.success() && err(&o).contains("config github owner/repo"), "{}", err(&o));
    let o = e.run(&["config", "github", "not-a-repo"], none);
    assert!(!o.status.success() && err(&o).contains("owner/repo"));
    // config now verifies the repo exists (the picker's check); a fake gh answers it
    let ok0 = e.fake_gh(&run_json("success"), false);
    let o = e.run(&["config", "github", "acme/widgets"], &ok0);
    assert!(o.status.success(), "{}", err(&o));
    let s = Store::open(&e.db()).unwrap();
    assert_eq!(s.github_repo().unwrap().as_deref(), Some("acme/widgets"));
    assert!(e.run(&["config", "github", "--off"], none).status.success());
    assert_eq!(s.github_repo().unwrap(), None);
}

#[test]
fn cli_serves_fresh_cache_without_calling_gh() {
    common::pin_clock();
    let e = Env::new();
    {
        let s = Store::open(&e.db()).unwrap();
        s.set_github(Some("acme/widgets")).unwrap();
        s.save_github(&Ok(snapshot("failure"))).unwrap();
        let mut s = s;
        let id = s.add("widgets: gh#327 call args", "", &[], "me").unwrap();
        s.take(id, "bot-2").unwrap();
    }
    let none = Path::new("/nonexistent/gh");
    let o = e.run(&["github"], none);
    assert!(o.status.success(), "{}", err(&o));
    let t = out(&o);
    for want in [
        "GITHUB · acme/widgets · synced",
        "issues 11 open · PRs 4 open (1 draft) · merged today 2 · main CI FAIL · 3 unclaimed",
        "gh#335 api: rate limit ignores burst setting · CI FAIL · review chg",
        "fix/315 -> gh#315 (bot)",
        "gh#327 login form rejects plus-addresses [bug] · in progress · bot-2",
        "gh#334 ui: totals overflow on wide tables (#315) [bug] · unclaimed · -",
        "gh#315 rate limit resets too early — see ticket 4 [bug,enhancement] · PR gh#335 FAIL · bot",
        "MERGED TODAY",
        "gh#330 dark theme",
        "MAIN CI FAIL · ci-main",
    ] {
        assert!(t.contains(want), "missing {want:?}:\n{t}");
    }
    let j: serde_json::Value = serde_json::from_str(&out(&e.run(&["github", "--json"], none))).unwrap();
    assert_eq!(j["repo"], "acme/widgets");
    assert_eq!(j["issues_open"], 11);
    assert_eq!(j["prs"][0]["ci"], "FAIL");
    let i2116 = j["issues"].as_array().unwrap().iter().find(|i| i["number"] == 327).unwrap();
    assert_eq!((i2116["state"].as_str(), i2116["who"].as_str()), (Some("in progress"), Some("bot-2")));
    let i2123 = j["issues"].as_array().unwrap().iter().find(|i| i["number"] == 334).unwrap();
    assert_eq!(i2123["state"], "unclaimed");
}

#[test]
fn cli_fetches_via_gh_when_stale_or_forced_and_reports_errors() {
    common::pin_clock();
    let e = Env::new();
    let ok = e.fake_gh(&run_json("success"), false);
    let bad = e.fake_gh(&run_json("success"), true);
    assert!(e.run(&["config", "github", "acme/widgets"], &ok).status.success());
    // no cache + failing gh: actionable error, non-zero
    let o = e.run(&["github"], &bad);
    assert!(!o.status.success() && err(&o).contains("HTTP 401") && err(&o).contains("gh auth status"), "{}", err(&o));
    // no cache: fetch (config's repo-view check made one call already)
    let o = e.run(&["github"], &ok);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("issues 42 open") && out(&o).contains("main CI ok"));
    assert_eq!(e.calls(), 6, "1 repo-view (config) + 5 per snapshot");
    // fresh: served from cache
    e.run(&["github", "--json"], &ok);
    assert_eq!(e.calls(), 6);
    // forced
    e.run(&["github", "--refresh"], &ok);
    assert_eq!(e.calls(), 11);
    // failing refresh keeps the last good snapshot and says so
    let o = e.run(&["github", "--refresh"], &bad);
    assert!(o.status.success());
    assert!(out(&o).contains("github: HTTP 401") && out(&o).contains("gh#334"), "{}", out(&o));
    // --json exposes the full error, the fail count and the snapshot's age
    let j: serde_json::Value = serde_json::from_str(&out(&e.run(&["github", "--json"], &bad))).unwrap();
    assert!(j["error"].as_str().unwrap().contains("HTTP 401"), "{}", j);
    assert_eq!(j["fails"], 1);
    assert!(j["fetched_at"].as_i64().unwrap() > 0);
    // two more failing refreshes -> 3 in a row (the UI threshold); a success resets it
    for want in [2, 3] {
        let j: serde_json::Value = serde_json::from_str(&out(&e.run(&["github", "--refresh", "--json"], &bad))).unwrap();
        assert_eq!(j["fails"], want, "{}", j);
    }
    let j: serde_json::Value = serde_json::from_str(&out(&e.run(&["github", "--refresh", "--json"], &ok))).unwrap();
    assert_eq!(j["fails"], 0, "success resets the counter: {}", j);
    assert!(j["error"].is_null());
}

/// `tb sync` reports a gh#N that GitHub answers 404 for, says "could not check" when the
/// lookup fails, looks each ref up once, and never looks up DONE cards.
#[test]
fn sync_reports_unknown_refs_once_per_ref_and_never_guesses() {
    let env = Env::new();
    let d = env.dir.path();
    let _ = env.fake_gh("[]", false); // writes the list fixtures
    let p = d.display();
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> {p}/calls.log\ncase \"$*\" in\n  *issues/991*) echo 'gh: Not Found (HTTP 404)' >&2; exit 1;;\n  *issues/992*) echo 'error connecting to api.github.com' >&2; exit 1;;\n  *issues/993*) echo '{{\"state\":\"closed\"}}'; exit 0;;\n  *repos/*/issues/*) echo '{{\"state\":\"open\"}}'; exit 0;;\nesac\ncase \"$1 $2\" in\n  \"pr list\") case \"$*\" in *merged*) cat {p}/merged.json;; *) cat {p}/prs.json;; esac;;\n  \"issue list\") cat {p}/issues.json;;\n  \"run list\") echo '[]';;\n  api*) echo 42;;\n  *) exit 2;;\nesac\n"
    );
    let gh = d.join("gh-refs");
    std::fs::write(&gh, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let s = Store::open(&env.db()).unwrap();
    s.set_github(Some("acme/widgets")).unwrap();
    s.add("web: gh#991 mistyped ref", "", &[], "lead").unwrap();
    s.add("web: gh#992 lookup fails", "", &[], "lead").unwrap();
    s.add("web: gh#993 closed long ago", "", &[], "lead").unwrap();
    let old = s.add("web: gh#994 finished work", "", &[], "lead").unwrap();
    s.add("web: gh#995 fine", "", &[], "lead").unwrap();
    drop(s);
    let mut s = Store::open(&env.db()).unwrap();
    s.move_to_forced(old, "done", "lead").unwrap(); // fixture only: nothing reaches done except from review (verifier rule), so a card seeded straight into done is a forced move
    drop(s);
    let log = || std::fs::read_to_string(d.join("calls.log")).unwrap_or_default();
    let o = env.run(&["sync", "--json"], &gh);
    assert!(o.status.success(), "{}", err(&o));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["unknown_refs"], serde_json::json!([991]), "{v}");
    assert_eq!(v["unchecked_refs"], serde_json::json!([992]), "{v}");
    // one lookup per not-done ref, none for the DONE card
    for n in [991, 992, 993] {
        assert_eq!(log().matches(&format!("issues/{n}")).count(), 1, "issues/{n}: {}", log());
    }
    assert!(!log().contains("issues/994"), "a DONE card is never looked up: {}", log());
    // plain text: the 404 is reported as missing, the failure as not checked
    let o = env.run(&["sync"], &gh);
    let text = out(&o);
    assert!(text.contains("gh#991: no such issue or PR in acme/widgets"), "{text}");
    assert!(text.contains("gh#992: could not check on GitHub") && !text.contains("gh#992: no such"), "{text}");
    assert!(!text.contains("gh#993") && !text.contains("gh#994"), "{text}");
}

#[test]
fn config_refuses_a_repo_that_does_not_exist() {
    let e = Env::new();
    // a fake gh whose `repo view` fails with gh's own not-found text
    let d = e.dir.path();
    std::fs::write(d.join("prs.json"), prs_json()).unwrap();
    std::fs::write(d.join("issues.json"), issues_json()).unwrap();
    std::fs::write(d.join("merged.json"), merged_json()).unwrap();
    std::fs::write(d.join("run.json"), run_json("success")).unwrap();
    let script = format!(
        r#"#!/bin/sh
case "$1 $2" in
  "repo view") echo "GraphQL: Could not resolve to a Repository with the name 'nobody-xyz/does-not-exist-123'." >&2; exit 1;;
  "repo list") echo '[]';;
  "pr list") case "$*" in *merged*) cat {p}/merged.json;; *) cat {p}/prs.json;; esac;;
  "issue list") cat {p}/issues.json;;
  "run list") cat {p}/run.json;;
  api*) echo 42;;
  *) exit 2;;
esac
"#,
        p = d.display()
    );
    let gh = d.join("gh-norepo");
    std::fs::write(&gh, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // config refuses with the repo-name hint, not the auth hint
    let o = e.run(&["config", "github", "nobody-xyz/does-not-exist-123"], &gh);
    assert!(!o.status.success());
    let err = err(&o);
    assert!(err.contains("no repo 'nobody-xyz/does-not-exist-123'"), "{err}");
    assert!(!err.contains("gh auth status"), "the name, not auth, is the problem: {err}");
    // nothing was saved
    let s = Store::open(&e.db()).unwrap();
    assert_eq!(s.github_repo().unwrap(), None);
    // --json carries the standard object
    let o = e.run(&["config", "github", "nobody-xyz/does-not-exist-123", "--json"], &gh);
    assert!(!o.status.success());
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(keys(&v), sorted(&["ok", "error", "hint", "code"]), "{}", v);
    // #81: a failed GitHub API/network call carries the stable `github_error` code
    assert_eq!(v["code"], "github_error");
    // an existing repo still saves (the ok script from Env covers it)
    let ok = e.fake_gh(&run_json("success"), false);
    assert!(e.run(&["config", "github", "acme/widgets"], &ok).status.success());
}
