//! `tb setup`: the first-run wizard (board, GitHub, AGENTS panel, Claude Code skill, the
//! agent snippet). Every step is `[y]es / [s]kip`; anything that touches the outside world
//! defaults to skip. Prompts read from the terminal (`/dev/tty`, or `$TB_TTY`), so they work
//! even when stdin is a pipe (`curl … | bash`).

use crate::store::{BoardError, Result, Store};
use crate::{boards, github};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The Claude Code skill, written to `~/.claude/skills/terminal-board/SKILL.md`.
pub const SKILL: &str = include_str!("../integrations/claude-code/SKILL.md");
/// The block appended to an AGENTS.md / CLAUDE.md, between the markers.
pub const SNIPPET: &str = include_str!("../integrations/AGENTS-snippet.md");
pub const START_MARK: &str = "<!-- terminal-board:start -->";
pub const END_MARK: &str = "<!-- terminal-board:end -->";
/// Config key marking a board as set up (bare `tb` then skips the wizard).
pub const SETUP_DONE: &str = "setup_done";

#[derive(Debug, Default, Clone)]
pub struct Options {
    /// Accept defaults, never prompt (external steps skipped unless flagged).
    pub yes: bool,
    pub github: Option<String>,
    pub no_github: bool,
    /// Some(true) = --agents, Some(false) = --no-agents.
    pub agents: Option<bool>,
    pub agents_md: Option<PathBuf>,
    pub dry_run: bool,
    /// Started by bare `tb` on first run: the first prompt may skip the whole wizard.
    pub first_run: bool,
}

/// Line-based prompts from the terminal; EOF or no terminal = the default answer.
struct Prompt {
    input: Option<Box<dyn BufRead>>,
    yes: bool,
}

impl Prompt {
    fn new(yes: bool) -> Prompt {
        let path = crate::env("TTY").unwrap_or_else(|| "/dev/tty".into());
        let input = if yes {
            None
        } else {
            std::fs::File::open(&path).ok().map(|f| Box::new(BufReader::new(f)) as Box<dyn BufRead>)
        };
        Prompt { input, yes }
    }

    fn line(&mut self) -> Option<String> {
        let input = self.input.as_mut()?;
        let mut s = String::new();
        match input.read_line(&mut s) {
            Ok(0) | Err(_) => {
                self.input = None;
                println!();
                None
            }
            Ok(_) => Some(s.trim().to_string()),
        }
    }

    /// `[Y]es / [s]kip` (default yes) or `[y]es / [S]kip` (default skip).
    fn ask(&mut self, q: &str, def_yes: bool) -> bool {
        let choices = if def_yes { "[Y]es / [s]kip" } else { "[y]es / [S]kip" };
        if self.yes {
            println!("    {q} {choices}: {} (--yes)", if def_yes { "y" } else { "s" });
            return def_yes;
        }
        print!("    {q} {choices}: ");
        let _ = std::io::stdout().flush();
        match self.line().as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("y" | "yes") => true,
            Some("s" | "skip" | "n" | "no") => false,
            _ => def_yes,
        }
    }

    /// A free-text answer ("" with --yes, EOF or enter).
    fn text(&mut self, q: &str) -> String {
        if self.yes {
            return String::new();
        }
        print!("    {q} ");
        let _ = std::io::stdout().flush();
        self.line().unwrap_or_default()
    }
}

struct Wizard {
    o: Options,
    p: Prompt,
    done: Vec<String>,
    skipped: Vec<String>,
}

fn note(s: &str) {
    println!("    {s}");
}

fn step(n: usize, title: &str) {
    println!("\n[{n}/5] {title}");
}

/// Is `name` an executable on PATH?
pub fn on_path(name: &str) -> bool {
    let p = Path::new(name);
    if p.components().count() > 1 {
        return p.is_file();
    }
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(name).is_file()))
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

fn gh_bin() -> String {
    crate::env("GH").unwrap_or_else(|| "gh".into())
}

fn gh_ok(args: &[&str]) -> bool {
    Command::new(gh_bin())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The terminal as a child's stdin (for `gh auth login`), else inherit.
fn tty_stdin() -> Stdio {
    let path = crate::env("TTY").unwrap_or_else(|| "/dev/tty".into());
    std::fs::File::open(path).map(Stdio::from).unwrap_or_else(|_| Stdio::inherit())
}

/// The command that installs the GitHub CLI here, if we know one.
fn gh_install_cmd() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        on_path("brew").then_some("brew install gh")
    } else if on_path("pacman") {
        Some("sudo pacman -S github-cli")
    } else if on_path("apt-get") {
        Some("sudo apt install gh")
    } else if on_path("dnf") {
        Some("sudo dnf install gh")
    } else {
        None
    }
}

/// Replace (or add) the marked snippet block in `file`; the rest of the file is kept.
pub fn write_block(file: &Path) -> std::io::Result<()> {
    let old = std::fs::read_to_string(file).unwrap_or_default();
    let mut kept = strip_block_text(&old);
    while kept.ends_with("\n\n") {
        kept.pop();
    }
    let sep = if kept.is_empty() { "" } else if kept.ends_with('\n') { "\n" } else { "\n\n" };
    let body = format!("{kept}{sep}{START_MARK}\n{}{}{END_MARK}\n", SNIPPET, if SNIPPET.ends_with('\n') { "" } else { "\n" });
    if let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, body)
}

/// `text` without the marked terminal-board block.
pub fn strip_block_text(text: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for l in text.lines() {
        if l == START_MARK {
            skip = true;
        } else if l == END_MARK {
            skip = false;
        } else if !skip {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

/// Remember an installed snippet for `install.sh --uninstall`.
fn remember(line: &str) {
    let dir = boards::state_dir();
    let file = dir.join("install.conf");
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    if old.lines().any(|l| l == line) {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&file, format!("{old}{line}\n"));
}

impl Wizard {
    fn did(&mut self, done: String, would: String) {
        self.done.push(if self.o.dry_run { would } else { done });
    }

    fn run(&mut self, board: &str) -> Result<()> {
        let dry = self.o.dry_run;
        println!("Terminal Board — setup");
        if dry {
            println!("(dry run: nothing will be changed)");
        }
        if self.o.first_run {
            println!("First run: a few questions (every step can be skipped; change anything later).");
            if !self.p.ask("Set up Terminal Board now?", true) {
                if !dry {
                    open(board)?.set_setup_done()?;
                }
                println!("Skipped. Run 'tb setup' any time.");
                return Ok(());
            }
        }

        step(1, "Board");
        let path = boards::path_for(board);
        let store = if dry {
            if path.exists() {
                Some(Store::open(&path)?.named(board))
            } else {
                None
            }
        } else {
            Some(open(board)?)
        };
        note(&format!("board '{board}' ({})", path.display()));
        self.did(format!("board '{board}' ready"), format!("Would create the board '{board}'"));

        step(2, "GitHub");
        self.github(store.as_ref())?;

        step(3, "AGENTS panel");
        let herdr = crate::herdr::herdr_enabled();
        note(if herdr { "herdr found" } else { "herdr not found (optional: it powers the AGENTS panel)" });
        let show = match self.o.agents {
            Some(v) => v,
            None => self.p.ask("Show the AGENTS panel (a live view of your herdr agents)?", herdr && !self.o.yes),
        };
        if let Some(s) = &store {
            if !dry {
                s.set_panel("agents-panel", if show { "shown" } else { "hidden" })?;
            }
        }
        if show {
            self.did("AGENTS panel shown".into(), "Would show the AGENTS panel".into());
        } else {
            self.skipped.push("AGENTS panel (press A on the board to show it)".into());
        }

        step(4, "Claude Code skill");
        let skill = home().join(".claude/skills/terminal-board/SKILL.md");
        let want = match self.o.agents {
            Some(false) => false,
            Some(true) => true,
            None => self.p.ask(&format!("Install the Claude Code skill ({})?", tilde(&skill)), false),
        };
        if want {
            if !dry {
                std::fs::create_dir_all(skill.parent().unwrap_or(Path::new(".")))
                    .and_then(|_| std::fs::write(&skill, SKILL))
                    .map_err(|e| BoardError(format!("cannot write {}: {e}", skill.display())))?;
            }
            self.did(format!("Claude Code skill in {}", tilde(&skill)), "Would install the Claude Code skill".into());
        } else {
            self.skipped.push("Claude Code skill".into());
        }

        step(5, "Agent instructions (AGENTS.md / CLAUDE.md)");
        let target = match (&self.o.agents_md, self.o.agents) {
            (Some(p), _) => Some(p.clone()),
            (None, Some(false)) => None,
            (None, _) => {
                let t = self.p.text("Add the agent snippet to which AGENTS.md / CLAUDE.md? Path (enter = skip):");
                (!t.is_empty()).then(|| expand(&t))
            }
        };
        match target {
            Some(t) => {
                if !dry {
                    write_block(&t).map_err(|e| BoardError(format!("cannot write {}: {e}", t.display())))?;
                    remember(&format!("snippet={}", t.display()));
                }
                self.did(format!("agent snippet in {}", t.display()), format!("Would add the agent snippet to {}", t.display()));
            }
            None => self.skipped.push("agent snippet ('tb setup --agents-md PATH' later)".into()),
        }

        if let Some(s) = &store {
            if !dry {
                s.set_setup_done()?;
            }
        }
        self.summary();
        Ok(())
    }

    fn github(&mut self, store: Option<&Store>) -> Result<()> {
        let dry = self.o.dry_run;
        let current = match store {
            Some(s) => s.github_repo()?,
            None => None,
        };
        // 0 = skip, 1 = connect, 2 = keep
        let want = if self.o.no_github {
            0
        } else if self.o.github.is_some() {
            1
        } else if let Some(c) = &current {
            note(&format!("connected to {c}"));
            if self.p.ask("Change the GitHub repo?", false) {
                1
            } else {
                2
            }
        } else if self.p.ask("Connect GitHub (issues and PRs next to your cards; needs the gh CLI)?", false) {
            1
        } else {
            0
        };
        if want == 2 {
            let c = current.unwrap_or_default();
            self.did(format!("GitHub kept: {c}"), format!("Would keep GitHub: {c}"));
            return Ok(());
        }
        let repo = if want == 1 { self.github_repo()? } else { None };
        match (repo, store) {
            (Some(r), Some(s)) if !dry => {
                s.set_github(Some(&r))?;
                s.set_panel("github-panel", "shown")?;
                sync(s, &r);
                note(&format!("connected to {r}"));
                self.done.push(format!("GitHub: {r}"));
            }
            (Some(r), _) => self.done.push(format!("Would connect GitHub: {r}")),
            (None, s) => {
                if let (Some(s), false) = (s, dry) {
                    s.set_github(None)?;
                    s.set_panel("github-panel", "hidden")?;
                }
                self.skipped.push("GitHub (panel hidden; press R on the board or 'tb config github OWNER/REPO' later)".into());
            }
        }
        Ok(())
    }

    /// gh present (install it after asking), logged in (log in after asking), then a repo
    /// picked from a numbered list or typed, validated with `gh repo view`.
    /// A repo named with `--github` that gh cannot find is refused (an error, exit 1); one typed
    /// at the prompt is reported and skipped.
    fn github_repo(&mut self) -> Result<Option<String>> {
        let dry = self.o.dry_run;
        if !on_path(&gh_bin()) {
            match gh_install_cmd() {
                Some(cmd) => {
                    note(&format!("The GitHub CLI (gh) is needed. Install it with: {cmd}"));
                    if !self.p.ask("Run that now?", false) {
                        note("Skipping GitHub: install gh (https://cli.github.com), then run 'tb setup' again.");
                        return Ok(None);
                    }
                    if dry {
                        note(&format!("would run: {cmd}"));
                        return Ok(self.o.github.clone());
                    }
                    let ok = Command::new("sh").args(["-c", cmd]).stdin(tty_stdin()).status().is_ok_and(|s| s.success());
                    if !ok || !on_path(&gh_bin()) {
                        note("gh did not install; skipping GitHub for now.");
                        return Ok(None);
                    }
                }
                None => {
                    note("The GitHub CLI (gh) is needed: https://cli.github.com — then run 'tb setup' again.");
                    return Ok(None);
                }
            }
        }
        if dry {
            return Ok(self.o.github.clone());
        }
        if !gh_ok(&["auth", "status"]) {
            note("gh is not logged in.");
            if !self.o.yes && self.p.ask("Log in now with 'gh auth login'? (gh handles your credentials)", false) {
                let _ = Command::new(gh_bin()).args(["auth", "login"]).stdin(tty_stdin()).status();
            }
            if !gh_ok(&["auth", "status"]) {
                note("Skipping GitHub: run 'gh auth login', then 'tb setup --github OWNER/REPO'.");
                return Ok(None);
            }
        }
        let mut repo = self.o.github.clone();
        if repo.is_none() {
            let list: Vec<String> = Command::new(gh_bin())
                .args(["repo", "list", "--limit", "15", "--json", "nameWithOwner", "--jq", ".[].nameWithOwner"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
                .unwrap_or_default();
            if !list.is_empty() {
                note("Your repos (most recently pushed first):");
                for (i, r) in list.iter().enumerate() {
                    note(&format!("  {}) {r}", i + 1));
                }
            }
            let pick = self.p.text("Pick a number, or type owner/repo (enter = skip):");
            repo = match pick.parse::<usize>() {
                Ok(n) if (1..=list.len()).contains(&n) => Some(list[n - 1].clone()),
                Ok(_) => None,
                Err(_) => (!pick.is_empty()).then_some(pick),
            };
        }
        let Some(r) = repo else { return Ok(None) };
        match github::check_repo(&r) {
            Ok(name) => Ok(Some(name)),
            Err(e) if self.o.github.is_some() => Err(BoardError(e)),
            Err(e) => {
                note(&e);
                Ok(None)
            }
        }
    }

    fn summary(&self) {
        println!("\nSummary");
        if !self.done.is_empty() {
            println!("{}", if self.o.dry_run { "Dry run — nothing was changed. A real run would:" } else { "Done:" });
            for d in &self.done {
                note(&format!("- {d}"));
            }
        }
        if !self.skipped.is_empty() {
            println!("Skipped:");
            for s in &self.skipped {
                note(&format!("- {s}"));
            }
        }
        println!();
        println!("Start: tb        Agents: tb guide        Settings: tb config        Again: tb setup");
    }
}

/// First sync after connecting (like `tb sync`); failures only print a hint.
fn sync(store: &Store, repo: &str) {
    let now = crate::store::now();
    let r = github::fetch(repo, now);
    let _ = store.save_github(&r);
    match r {
        Ok(_) => note("first sync done"),
        Err(_) => note("(first sync failed; it retries on the board)"),
    }
}

fn open(board: &str) -> Result<Store> {
    Ok(Store::open(&boards::path_for(board))?.named(board))
}

fn tilde(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(p),
    }
}

/// Run the wizard for `board`.
pub fn run(board: &str, o: Options) -> Result<()> {
    let p = Prompt::new(o.yes);
    let mut w = Wizard { o, p, done: Vec::new(), skipped: Vec::new() };
    w.run(board)
}

/// Bare `tb` on a fresh machine: no board has been set up or configured yet.
pub fn first_run() -> bool {
    if crate::env("DB").is_some() || crate::env("NO_SETUP").is_some() {
        return false;
    }
    boards::list().iter().all(|n| {
        Store::open(&boards::path_for(n)).is_ok_and(|s| !s.is_set_up().unwrap_or(true))
    })
}
