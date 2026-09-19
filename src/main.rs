use clap::{Parser, Subcommand};
use serde_json::json;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use terminal_board::store::{BoardError, Store, COLUMNS};
use terminal_board::{boards, contract, github, plain, resolve_actor, setup, tui};

const HELP: &str = "\
tb {version} - Terminal Board: one shared task board for people and agents (todo > doing > review > done)
Usage: tb [BOARD] [COMMAND] [--json] [--as NAME] [-b BOARD]   no command: open the board (? = keys)

Cards   add \"tag: title\" [-d DESC] [--check ITEM]...   edit ID [--title T] [--desc D]   rm ID
        list · show ID · note ID \"text\" · block ID \"#7\" | --clear
        check ID N (toggle) | --add \"text\" | --rm N
Flow    next (take the top todo) · take ID · done ID [--force] · drop ID
        move ID todo|doing|review|done [--force] · prio ID top|bottom|up|down
Boards  boards · board (print; --json = full state) · watch --json (NDJSON on every change)
Config  config [wip N | theme dark|light | layout L | github OWNER/REPO|--off | github-panel|agents-panel shown|hidden]
GitHub  github [--refresh] · github repos · sync (move gh cards on PR/merge/close evidence)
Agents  agents (herdr panes + the card each holds)
Setup   setup [--yes] [--github R | --no-github] [--agents | --no-agents] [--agents-md PATH] [--dry-run]

Options
  --json         machine-readable output; every write prints {\"ok\":…}   (docs/JSON.md)
  --as NAME      act as NAME (else $TB_AS, $HERDR_AGENT_NAME, $USER)
  -b NAME        board (else a first-arg name, $TB_BOARD, default); $TB_DB = file
  -h, -V         help, version
agents: run 'tb guide' for the full agent manual
";

/// The agent manual (`tb guide`).
const GUIDE: &str = include_str!("../docs/AGENTS.md");

#[derive(Parser)]
#[command(name = "Terminal Board", bin_name = "tb", version, help_template = HELP, disable_help_subcommand = true)]
struct Cli {
    /// Act as NAME
    #[arg(long = "as", global = true, value_name = "NAME")]
    actor: Option<String>,
    /// Machine-readable output
    #[arg(long, global = true)]
    json: bool,
    /// Board to use
    #[arg(short = 'b', long = "board", global = true, value_name = "NAME")]
    board: Option<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    Add {
        title: String,
        #[arg(short = 'd', long = "desc", default_value = "")]
        desc: String,
        #[arg(long = "check")]
        checks: Vec<String>,
    },
    List,
    Show { id: i64 },
    Next,
    Take { id: i64 },
    Note { id: i64, text: String },
    Check {
        id: i64,
        n: Option<i64>,
        #[arg(long, value_name = "TEXT", conflicts_with_all = ["n", "rm"])]
        add: Option<String>,
        #[arg(long, value_name = "N", conflicts_with = "n")]
        rm: Option<i64>,
    },
    Move {
        id: i64,
        column: String,
        #[arg(long)]
        force: bool,
    },
    Done {
        id: i64,
        #[arg(long)]
        force: bool,
    },
    Block {
        id: i64,
        reason: Option<String>,
        #[arg(long, conflicts_with = "reason")]
        clear: bool,
    },
    Drop { id: i64 },
    Rm { id: i64 },
    Prio { id: i64, how: String },
    Edit {
        id: i64,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        desc: Option<String>,
    },
    Config {
        key: Option<String>,
        value: Option<String>,
        #[arg(long)]
        off: bool,
    },
    Boards,
    Board,
    Watch,
    Agents,
    Sync,
    Guide,
    Github {
        what: Option<String>,
        #[arg(long)]
        refresh: bool,
    },
    Setup {
        #[arg(long, short = 'y')]
        yes: bool,
        #[arg(long, value_name = "OWNER/REPO", conflicts_with = "no_github")]
        github: Option<String>,
        #[arg(long)]
        no_github: bool,
        #[arg(long, conflicts_with = "no_agents")]
        agents: bool,
        #[arg(long)]
        no_agents: bool,
        #[arg(long, value_name = "PATH")]
        agents_md: Option<std::path::PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
}

impl Cmd {
    /// Commands that may create the board file.
    fn writes(&self) -> bool {
        !matches!(
            self,
            Cmd::List | Cmd::Show { .. } | Cmd::Boards | Cmd::Github { .. } | Cmd::Board | Cmd::Agents | Cmd::Guide
        )
    }
}

fn pretty<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".into())
}

/// `{"ok":true,"card":…}` for --json, else the human line.
fn done_card(store: &Store, jsonout: bool, id: i64, human: String) -> Result<(), BoardError> {
    if jsonout {
        println!("{}", pretty(&json!({"ok": true, "card": contract::card_by_id(store, id)?})));
    } else {
        println!("{human}");
    }
    Ok(())
}

/// Decision 14: a gh card only reaches done on evidence unless forced.
fn guard_done(store: &Store, id: i64, force: bool, cmd: &str) -> Result<(), BoardError> {
    if force {
        return Ok(());
    }
    let c = store.card(id)?;
    let (Some(n), Some(snap)) = (c.gh_ref, store.github_view()?.snap) else { return Ok(()) };
    if github::still_open(&snap, n) {
        return Err(BoardError(format!(
            "issue #{n} still open on GitHub — close it there, or 'tb {cmd} --force' to mark it done anyway"
        )));
    }
    Ok(())
}

fn agents_now(store: &Store) -> Result<Vec<contract::AgentJ>, BoardError> {
    let list = match terminal_board::herdr::probe() {
        terminal_board::herdr::AgentsState::Agents(a) => a,
        _ => Vec::new(),
    };
    Ok(contract::agents(&list, &store.list()?))
}

/// NDJSON (or plain) board on every change; exits quietly when stdout closes.
fn watch(store: &Store, jsonout: bool) -> Result<(), BoardError> {
    let mut out = std::io::stdout().lock();
    let mut last = None;
    loop {
        let v = store.data_version()?;
        if last != Some(v) {
            last = Some(v);
            let text = if jsonout {
                serde_json::to_string(&contract::board(store)?).unwrap_or_default()
            } else {
                format!("{}\n", plain::board(&store.snapshot()?))
            };
            if writeln!(out, "{text}").and_then(|_| out.flush()).is_err() {
                return Ok(()); // reader went away
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

fn open_board(name: &str, create: bool) -> Result<Store, BoardError> {
    let path = boards::path_for(name);
    if !path.exists() && !create {
        // read on a board that doesn't exist yet: empty, and nothing is created
        return Ok(Store::open(Path::new(":memory:"))?.named(name));
    }
    let existed = path.exists();
    let store = Store::open(&path)?.named(name);
    if !existed {
        eprintln!("created board '{name}'");
    }
    Ok(store)
}

fn list_boards(json_out: bool) -> Result<(), BoardError> {
    let def = boards::default_name();
    let names = if terminal_board::env("DB").is_some() {
        vec![def.clone()]
    } else {
        boards::list()
    };
    let mut rows = Vec::new();
    for n in &names {
        let snap = Store::open(&boards::path_for(n))?.named(n).snapshot()?;
        let counts: Vec<usize> = COLUMNS.iter().map(|c| snap.in_column(c).len()).collect();
        rows.push((n.clone(), *n == def, counts));
    }
    if json_out {
        let v: Vec<_> = rows
            .iter()
            .map(|(n, d, c)| {
                serde_json::json!({"name": n, "default": d, "todo": c[0], "doing": c[1], "review": c[2], "done": c[3]})
            })
            .collect();
        println!("{}", pretty(&v));
    } else if rows.is_empty() {
        println!("no boards yet — 'tb add \"title\"' creates '{def}', 'tb home add \"title\"' creates 'home'");
    } else {
        for (n, d, c) in &rows {
            println!(
                "{} {n:<16} todo {:<3} doing {:<3} review {:<3} done {}",
                if *d { "*" } else { " " },
                c[0], c[1], c[2], c[3]
            );
        }
        println!("* = default (plain 'tb'); open another with 'tb NAME'");
    }
    Ok(())
}


fn run(cli: Cli, positional: Option<String>) -> Result<(), BoardError> {
    let actor = resolve_actor(cli.actor.as_deref());
    if !terminal_board::env("DB").is_some() {
        match boards::migrate(&boards::old_state_dir(), &boards::state_dir()) {
            Ok(notes) => {
                for n in notes {
                    eprintln!("tb: {n}");
                }
            }
            Err(e) => eprintln!("tb: could not migrate the legacy board: {e}"),
        }
    }
    if matches!(cli.cmd, Some(Cmd::Boards)) {
        return list_boards(cli.json);
    }
    let env = terminal_board::env("BOARD");
    let name = boards::select(positional.as_deref(), cli.board.as_deref(), env.as_deref())?;
    if let Some(Cmd::Setup { yes, github, no_github, agents, no_agents, agents_md, dry_run }) = cli.cmd {
        let agents = if agents { Some(true) } else if no_agents { Some(false) } else { None };
        let o = setup::Options { yes, github, no_github, agents, agents_md, dry_run, first_run: false };
        return setup::run(&name, o);
    }
    let tty = std::io::stdout().is_terminal();
    // first run: bare `tb` in a terminal on a machine where nothing is set up yet
    if cli.cmd.is_none() && tty && std::io::stdin().is_terminal() && setup::first_run() {
        setup::run(&name, setup::Options { first_run: true, ..Default::default() })?;
    }
    let create = cli.cmd.as_ref().map_or(tty, Cmd::writes);
    let mut store = open_board(&name, create)?;
    let cmd = cli.cmd;
    let j = cli.json;
    let Some(cmd) = cmd else {
        if tty {
            return tui::run(store, &actor)
                .map_err(|e| BoardError(format!("terminal error: {e} — try 'tb list'")));
        }
        if j {
            println!("{}", pretty(&contract::board(&store)?));
        } else {
            print!("{}", plain::board(&store.snapshot()?));
        }
        return Ok(());
    };
    let now = terminal_board::store::now();
    match cmd {
        Cmd::Add { title, desc, checks } => {
            let id = store.add(&title, &desc, &checks, &actor)?;
            done_card(&store, j, id, format!("added #{id} — take it with 'tb take {id}'"))?;
        }
        Cmd::List => {
            let snap = store.snapshot()?;
            if j {
                println!("{}", pretty(&snap.cards));
            } else {
                print!("{}", plain::list(&snap));
            }
        }
        Cmd::Show { id } => {
            let d = store.show(id)?;
            if j {
                println!("{}", pretty(&d));
            } else {
                print!("{}", plain::detail(&d, now));
            }
        }
        Cmd::Board => {
            if j {
                println!("{}", pretty(&contract::board(&store)?));
            } else {
                print!("{}", plain::board(&store.snapshot()?));
            }
        }
        Cmd::Watch => watch(&store, j)?,
        Cmd::Agents => {
            let list = agents_now(&store)?;
            if j {
                println!("{}", pretty(&list));
            } else if list.is_empty() {
                println!("no agents (herdr not available or no agent panes)");
            } else {
                for a in list {
                    let card = a.card_id.map(|c| format!("#{c}")).unwrap_or_else(|| "-".into());
                    println!("{:<16} {:<8} {:<8} {:<8} {card}  {}", a.name, a.harness, a.status, a.pane_id, a.job.unwrap_or_default());
                }
            }
        }
        Cmd::Next | Cmd::Take { .. } => {
            let card = match cmd {
                Cmd::Take { id } => store.take(id, &actor)?,
                _ => store.next(&actor)?,
            };
            let human = format!(
                "{}\ntaken by {actor} — log progress with 'tb note {id} \"...\"', finish with 'tb done {id}'",
                plain::detail(&store.show(card.id)?, now).trim_end(),
                id = card.id
            );
            done_card(&store, j, card.id, human)?;
        }
        Cmd::Note { id, text } => {
            store.note(id, &text, &actor)?;
            done_card(&store, j, id, format!("noted #{id}"))?;
        }
        Cmd::Check { id, n, add, rm } => {
            let human = match (n, add, rm) {
                (_, _, Some(r)) => {
                    store.remove_check(id, r, &actor)?;
                    format!("#{id} item {r} deleted, the rest renumbered — see 'tb show {id}'")
                }
                (_, Some(text), None) => {
                    let n = store.add_check(id, &text, &actor)?;
                    format!("#{id} item {n} added — toggle it with 'tb check {id} {n}'")
                }
                (Some(n), None, None) => {
                    let on = store.check(id, n, &actor)?;
                    format!("#{id} item {n} {}", if on { "checked" } else { "unchecked" })
                }
                (None, None, None) => {
                    return Err(BoardError(format!(
                        "give an item number, --add or --rm — 'tb check {id} 1', 'tb check {id} --add \"text\"'"
                    )))
                }
            };
            done_card(&store, j, id, human)?;
        }
        Cmd::Move { id, column, force } => {
            if column.eq_ignore_ascii_case("done") {
                guard_done(&store, id, force, &format!("move {id} done"))?;
            }
            let c = if force { store.move_to_forced(id, &column, &actor)? } else { store.move_to(id, &column, &actor)? };
            done_card(&store, j, id, format!("#{id} is now in {}", c.column))?;
        }
        Cmd::Done { id, force } => {
            if store.card(id)?.column != "doing" {
                guard_done(&store, id, force, &format!("done {id}"))?;
            }
            let c = if force { store.done_forced(id, &actor)? } else { store.done(id, &actor)? };
            let human = if c.column == "review" {
                format!("#{id} is now in review — close it with 'tb done {id}' once verified")
            } else {
                format!("#{id} is done")
            };
            done_card(&store, j, id, human)?;
        }
        Cmd::Block { id, reason, clear } => {
            let human = match (reason, clear) {
                (_, true) => {
                    store.block(id, None, &actor)?;
                    format!("#{id} unblocked")
                }
                (Some(r), false) => {
                    store.block(id, Some(&r), &actor)?;
                    format!("#{id} blocked — clear it with 'tb block {id} --clear'")
                }
                (None, false) => {
                    return Err(BoardError(format!(
                        "say what blocks it — 'tb block {id} \"#7\"' or 'tb block {id} --clear'"
                    )))
                }
            };
            done_card(&store, j, id, human)?;
        }
        Cmd::Drop { id } => {
            store.drop_card(id, &actor)?;
            done_card(&store, j, id, format!("#{id} is back in todo, unowned"))?;
        }
        Cmd::Rm { id } => {
            let before = contract::card_by_id(&store, id)?;
            let c = store.delete_card(id, &actor)?;
            if j {
                println!("{}", pretty(&json!({"ok": true, "card": before})));
            } else {
                println!("deleted #{id} \"{}\"", c.title);
            }
        }
        Cmd::Prio { id, how } => {
            let c = store.reorder(id, &how.to_ascii_lowercase(), &actor)?;
            done_card(&store, j, id, format!("#{id} is now at position {} in {}", c.position + 1, c.column))?;
        }
        Cmd::Edit { id, title, desc } => {
            store.edit(id, title.as_deref(), desc.as_deref(), &actor)?;
            done_card(&store, j, id, format!("#{id} saved"))?;
        }
        Cmd::Sync => {
            let repo = store.github_repo()?.ok_or_else(|| {
                BoardError(format!(
                    "github is off for board '{}' — turn it on with 'tb config github owner/repo'",
                    store.name
                ))
            })?;
            let r = github::fetch(&repo, now);
            store.save_github(&r)?;
            let snap = r.map_err(|e| BoardError(format!("github: {e} — check 'gh auth status', then 'tb sync'")))?;
            let cards = store.list()?;
            let states = github::fetch_states(&repo, &github::needs_state(&snap, &cards));
            let moves = github::plan_moves(&snap, &cards, &states);
            github::apply_moves(&mut store, &moves)?;
            if j {
                println!("{}", pretty(&json!({"ok": true, "moves": moves})));
            } else if moves.is_empty() {
                println!("synced {repo}: nothing to move");
            } else {
                for m in &moves {
                    println!("#{} {} -> {}  ({})", m.card_id, m.from, m.to, m.text);
                }
            }
        }
        Cmd::Guide => print!("{GUIDE}"),
        Cmd::Config { key: None, .. } => {
            let all = store.settings()?;
            if j {
                let m: serde_json::Map<String, serde_json::Value> = all.into_iter().map(|(k, v)| (k, json!(v))).collect();
                println!("{}", pretty(&json!({"ok": true, "config": m})));
            } else {
                for (k, v) in all {
                    println!("{k:<13} {v}");
                }
            }
        }
        Cmd::Config { key: Some(key), value, off } => {
            let (k, v): (String, serde_json::Value) = match (key.as_str(), value) {
                ("github", _) if off => {
                    store.set_github(None)?;
                    ("github".into(), serde_json::Value::Null)
                }
                ("github", None) => {
                    let r = store.github_repo()?;
                    if !j {
                        match &r {
                            Some(r) => println!("{r}"),
                            None => println!(
                                "github is off for board '{}' — 'tb config github owner/repo', or press R on the board",
                                store.name
                            ),
                        }
                        return Ok(());
                    }
                    ("github".into(), json!(r))
                }
                ("github", Some(repo)) => {
                    // the picker checks the repo exists; the CLI must not save a name that
                    // will fail every later sync with an auth-flavoured error
                    github::check_repo(&repo).map_err(BoardError)?;
                    store.set_github(Some(&repo))?;
                    ("github".into(), json!(repo))
                }
                ("wip", Some(value)) => {
                    let n: i64 = value.parse().map_err(|_| {
                        BoardError(format!("wip must be a number, got '{value}' — try 'tb config wip 3'"))
                    })?;
                    store.change_wip(n, &actor)?;
                    ("wip".into(), json!(n))
                }
                ("layout", Some(value)) => {
                    store.set_layout(&value)?;
                    ("layout".into(), json!(store.layout()?))
                }
                ("theme", Some(value)) => {
                    store.set_theme(&value)?;
                    ("theme".into(), json!(store.theme()?))
                }
                (panel @ ("github-panel" | "agents-panel"), Some(value)) => {
                    store.set_panel(panel, &value)?;
                    (panel.into(), json!(if store.panel(panel)? { "shown" } else { "hidden" }))
                }
                _ => {
                    return Err(BoardError(format!(
                        "unknown or incomplete setting '{key}' — use 'tb config wip 3', 'config github owner/repo', 'config theme dark|light'"
                    )))
                }
            };
            if j {
                println!("{}", pretty(&json!({"ok": true, "config": {"key": k, "value": v}})));
            } else {
                match (k.as_str(), &v) {
                    ("github", serde_json::Value::Null) => println!("github panel off for board '{}'", store.name),
                    ("github", r) => println!("github panel on: {} — see it with 'tb github' or 'G' on the board", r.as_str().unwrap_or("")),
                    ("wip", n) => println!("wip limit is now {n}"),
                    (k, v) => println!("{k} is now {}", v.as_str().unwrap_or("")),
                }
            }
        }
        Cmd::Github { what: Some(w), .. } if w == "repos" => {
            let repos = github::list_repos().map_err(|e| BoardError(format!("github: {e}")))?;
            if j {
                println!("{}", pretty(&repos));
            } else {
                let cur = store.github_repo()?;
                for r in &repos {
                    let mark = if cur.as_deref() == Some(r.name_with_owner.as_str()) { "*" } else { " " };
                    println!("{mark} {}", github::repo_row(r, now));
                }
                println!("pick one with 'tb config github owner/repo' or R on the board");
            }
        }
        Cmd::Github { what: Some(w), .. } => {
            return Err(BoardError(format!("unknown 'github {w}' — try 'tb github' or 'tb github repos'")));
        }
        Cmd::Github { refresh, .. } => {
            let repo = store.github_repo()?.ok_or_else(|| {
                BoardError(format!(
                    "github is off for board '{}' — turn it on with 'tb config github owner/repo'",
                    store.name
                ))
            })?;
            let view = store.github_view()?;
            let fresh = view.snap.as_ref().is_some_and(|s| now - s.fetched_at < github::MAX_AGE_SECS);
            if refresh || !fresh {
                let r = github::fetch(&repo, now);
                store.save_github(&r)?;
                if let (Err(e), None) = (&r, &view.snap) {
                    return Err(BoardError(format!("github: {e} — check 'gh auth status', then 'tb github --refresh'")));
                }
            }
            let view = store.github_view()?;
            let cards = store.list()?;
            if j {
                // raw cached snapshot, plus the factory view per issue (state/who)
                let raw = store.github_cache()?.0.unwrap_or_else(|| "null".into());
                let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
                if let (Some(s), Some(list)) = (&view.snap, v.get_mut("issues").and_then(|i| i.as_array_mut())) {
                    let f = github::factory(s, &cards, now);
                    for item in list.iter_mut() {
                        let n = item.get("number").and_then(|x| x.as_i64());
                        if let Some(r) = f.issues.iter().find(|r| Some(r.number) == n) {
                            item["state"] = json!(r.state);
                            item["who"] = json!(r.who);
                        }
                    }
                }
                println!("{}", pretty(&v));
            } else if let Some(s) = &view.snap {
                print!("{}", github::text(s, &cards, view.error.as_deref(), 10, now));
            }
        }
        Cmd::Boards | Cmd::Setup { .. } => unreachable!("handled above"),
    }
    Ok(())
}

/// `ttyboard home add "x"`: a first argument that isn't a flag or a command is a board name.
fn split_board(mut args: Vec<std::ffi::OsString>) -> Result<(Option<String>, Vec<std::ffi::OsString>), BoardError> {
    let first = args.get(1).and_then(|a| a.to_str()).map(str::to_string);
    match first {
        Some(a) if !a.starts_with('-') && !boards::COMMANDS.contains(&a.as_str()) => {
            boards::validate(&a)?;
            args.remove(1);
            Ok((Some(a), args))
        }
        _ => Ok((None, args)),
    }
}

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let jsonout = args.iter().any(|a| a == "--json");
    let parsed = split_board(args).and_then(|(board, args)| run(Cli::parse_from(args), board));
    match parsed {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if jsonout {
                println!("{}", pretty(&contract::error(&e.to_string())));
            } else {
                eprintln!("tb: {e}");
            }
            ExitCode::FAILURE
        }
    }
}
