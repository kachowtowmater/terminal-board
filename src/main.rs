use clap::{Parser, Subcommand};
// all output through `terminal_board::write_stdout` (see lib.rs): a closed stdout is a clean exit
macro_rules! println {
    () => { terminal_board::write_stdout("\n") };
    ($($a:tt)*) => { terminal_board::write_stdout(&format!("{}\n", format_args!($($a)*))) };
}
macro_rules! print {
    ($($a:tt)*) => { terminal_board::write_stdout(&format!($($a)*)) };
}

use serde_json::json;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use terminal_board::store::due::{self, DueDate};
use terminal_board::store::{BoardError, Store, COLUMNS};
use terminal_board::{boards, contract, github, import, plain, resolve_actor, setup, textin, tui};

const HELP: &str = "\
tb {version} - Terminal Board: one shared task board for people and agents (todo > doing > review > done)
Usage: tb [BOARD] [COMMAND] [--json] [--as NAME] [-b BOARD]   no command: open the board (? = keys)

Cards   add \"tag: title\" [-d DESC] [--check ITEM]...   edit ID [--title T] [--desc D]   rm ID
        list · show ID · note ID \"text\" · block ID \"#7\" | --clear
        check ID N (toggle) | --add \"text\" | --rm N · long text from a file: --desc-file PATH · note ID --file PATH (- = stdin)
Due     add|edit … --due YYYY-MM-DD|none (a calendar date)   config tz ZONE|local · due-warn DAYS
Bulk    import FILE.json|- · edit --from FILE.json|-   [--dry-run]   many cards from one JSON file, all or nothing
Flow    next (take the top todo) · next --review (claim a card to review) · take ID · done ID [--force] · drop ID
        move ID todo|doing|review|done [--force] · move ID doing \"why\" (send back from review)
        prio ID top|bottom|up|down
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
        /// Due date, a calendar date: YYYY-MM-DD.
        #[arg(long, value_name = "DATE")]
        due: Option<String>,
        /// Read the description from a file, byte for byte (`-` = standard input).
        #[arg(long = "desc-file", value_name = "PATH", conflicts_with = "desc")]
        desc_file: Option<std::path::PathBuf>,
    },
    List,
    Show { id: i64 },
    Next {
        /// Claim the top REVIEW card you did not do yourself, instead of a TODO card.
        #[arg(long)]
        review: bool,
    },
    Take { id: i64 },
    Note {
        id: i64,
        #[arg(required_unless_present = "file")]
        text: Option<String>,
        /// Read the note from a file, byte for byte (`-` = standard input).
        #[arg(long, value_name = "PATH", conflicts_with = "text")]
        file: Option<std::path::PathBuf>,
    },
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
        /// Why a REVIEW card goes back to doing (required for that move only)
        reason: Option<String>,
        #[arg(long)]
        force: bool,
    },
    Done {
        id: i64,
        #[arg(long)]
        force: bool,
        /// Record your approval without moving the card (REVIEW stays in REVIEW; the gh#
        /// card still waits for its merge to reach done).
        #[arg(long, conflicts_with = "force")]
        approve: bool,
    },
    Block {
        id: i64,
        reason: Option<String>,
        #[arg(long, conflicts_with = "reason")]
        clear: bool,
    },
    Drop {
        id: i64,
        /// Take someone else's DOING card back to todo (logged as its own event).
        #[arg(long)]
        force: bool,
    },
    Rm { id: i64 },
    Prio { id: i64, how: String },
    Edit {
        // required — except with --from, which conflicts with it (a conflict with a present
        // argument lifts the requirement); this keeps every ID message exactly as it was
        #[arg(required = true)]
        id: Option<i64>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        desc: Option<String>,
        /// Due date, a calendar date: YYYY-MM-DD, or `none` to clear it.
        #[arg(long, value_name = "DATE|none")]
        due: Option<String>,
        /// Read the description from a file, byte for byte (`-` = standard input).
        #[arg(long = "desc-file", value_name = "PATH", conflicts_with = "desc")]
        desc_file: Option<std::path::PathBuf>,
        /// Change many cards from one JSON file (`-` = standard input): rows keyed by `id`,
        /// only the fields present change, all or nothing.
        #[arg(long, value_name = "FILE", conflicts_with_all = ["id", "title", "desc", "due", "desc_file"])]
        from: Option<std::path::PathBuf>,
        /// With --from: report what would change, write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    Config {
        key: Option<String>,
        value: Option<String>,
        #[arg(long)]
        off: bool,
    },
    Boards,
    Board,
    /// Opt-in event stream: one NDJSON line per event; `--since` resumes after a restart.
    Watch {
        /// Print one NDJSON line per event instead of the whole board on every change.
        #[arg(long, requires = "json")]
        events: bool,
        /// Start from events at/after this unix-second timestamp.
        #[arg(long, value_name = "TS", requires = "events")]
        since: Option<i64>,
    },
    Agents,
    Sync,
    Guide,
    /// Create many cards from one JSON file (`-` = standard input), all or nothing.
    Import {
        file: std::path::PathBuf,
        /// Report what would be created, write nothing.
        #[arg(long)]
        dry_run: bool,
    },
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

/// Human output goes through the sanitizer: stored, typed and remote text is data, never
/// terminal control. `say!` is one line; the `_lines` forms keep line breaks. JSON is untouched.
macro_rules! say {
    ($($a:tt)*) => { println!("{}", terminal_board::text::sanitize(&format!($($a)*))) };
}
macro_rules! say_lines {
    ($($a:tt)*) => { println!("{}", terminal_board::text::sanitize_lines(&format!($($a)*))) };
}
macro_rules! print_lines {
    ($($a:tt)*) => { print!("{}", terminal_board::text::sanitize_lines(&format!($($a)*))) };
}
macro_rules! warn {
    ($($a:tt)*) => { eprintln!("{}", terminal_board::text::sanitize_lines(&format!($($a)*))) };
}

fn pretty<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".into())
}

/// The command a hint names: `tb take 1` on the default board, `tb work take 1` on an
/// explicitly named non-default board (a copied hint must not act on the default board).
/// A board picked by `TB_BOARD` travels in the env, so the bare form is right there too.
fn cmd_hint(explicit: Option<&str>, rest: &str) -> String {
    match explicit {
        Some(name) if name != boards::DEFAULT_BOARD => format!("'tb {name} {rest}'"),
        _ => format!("'tb {rest}'"),
    }
}

/// The board named on the command line (a first-argument name or `-b`), unless it is
/// `default`. A board picked by `TB_BOARD` travels in the environment, so it is not
/// "explicit": its hints stay bare on every path.
fn explicit_board<'a>(positional: Option<&'a str>, flag: Option<&'a str>) -> Option<&'a str> {
    positional.or(flag).filter(|n| *n != boards::DEFAULT_BOARD)
}

/// Put an explicitly named board into every command a text hints at
/// (`'tb list'` -> `'tb work list'`); unchanged without one.
fn with_board(text: &str, explicit: Option<&str>) -> String {
    match explicit {
        Some(name) => {
            // hints built with cmd_hint already carry the board: leave those alone
            let done = [format!("'tb {name} "), format!("`tb {name} ")];
            let marks = ["\u{0}q", "\u{0}b"];
            let mut t = text.replace(&done[0], marks[0]).replace(&done[1], marks[1]);
            t = t.replace("'tb ", &done[0]).replace("`tb ", &done[1]);
            t.replace(marks[0], &done[0]).replace(marks[1], &done[1])
        }
        None => text.to_string(),
    }
}

/// Plain board/list text: an empty board's `'tb add …'` hint names an explicit board too.
fn plain_hinted(text: String, empty: bool, explicit: Option<&str>) -> String {
    if empty {
        with_board(&text, explicit)
    } else {
        text
    }
}

/// `{"ok":true,"card":…}` for --json, else the human line.
fn done_card(store: &Store, jsonout: bool, id: i64, human: String) -> Result<(), BoardError> {
    if jsonout {
        println!("{}", pretty(&json!({"ok": true, "card": contract::card_by_id(store, id)?})));
    } else {
        say_lines!("{human}");
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
            "issue gh#{n} still open on GitHub — close it there, or 'tb {cmd} --force' to mark it done anyway"
        )));
    }
    Ok(())
}

fn agents_now(store: &Store) -> Result<Vec<contract::AgentJ>, BoardError> {
    let list = match terminal_board::herdr::probe() {
        terminal_board::herdr::AgentsState::Agents(a) => a,
        _ => Vec::new(),
    };
    Ok(contract::agents(&list, &store.snapshot()?))
}

/// One `tb watch --events --json` line: the event plus the column transition of every event
/// that changes a card's column (`created` → todo, `taken` todo → doing, `moved`/`dropped`
/// from their `a -> b` text); `from`/`to` stay null for events that move nothing.
#[derive(serde::Serialize)]
struct EventLine<'a> {
    v: u32,
    ts: i64,
    card_id: i64,
    actor: &'a str,
    kind: &'a str,
    from: Option<&'a str>,
    to: Option<&'a str>,
    text: &'a str,
}

impl<'a> EventLine<'a> {
    fn of(e: &'a terminal_board::store::Event) -> Self {
        let column = |c: &'a str| COLUMNS.iter().copied().find(|k| *k == c);
        let (from, to) = match e.kind.as_str() {
            "created" => (None, Some("todo")),
            "taken" => (Some("todo"), Some("doing")),
            "moved" | "dropped" => e
                .text
                .split_once(" -> ")
                .and_then(|(f, t)| Some((column(f.trim())?, column(t.trim())?)))
                .map(|(f, t)| (Some(f), Some(t)))
                .unwrap_or((None, None)),
            _ => (None, None),
        };
        EventLine { v: contract::SCHEMA_VERSION, ts: e.ts, card_id: e.card_id, actor: &e.actor, kind: &e.kind, from, to, text: &e.text }
    }
}

/// NDJSON (or plain) board on every change; exits quietly when stdout closes.
/// With `events` (JSON only): one `{v, ts, card_id, actor, kind, from, to, text}` line per
/// event, resuming from `since` (unix seconds) after a restart.
fn watch(
    store: &Store,
    jsonout: bool,
    events: bool,
    since: Option<i64>,
    explicit: Option<&str>,
) -> Result<(), BoardError> {
    let mut out = std::io::stdout().lock();
    if events {
        // Resume: the id of the last event at/after `since` (0 = stream from the start).
        let mut last = match since {
            None => 0,
            Some(ts) => store.events_cursor_at(ts)?,
        };
        loop {
            for e in store.events_since(last)? {
                last = e.id;
                let line = EventLine::of(&e.event);
                let text = serde_json::to_string(&line).unwrap_or_default();
                if writeln!(out, "{text}").and_then(|_| out.flush()).is_err() {
                    return Ok(()); // reader went away
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
    let mut last = None;
    loop {
        let v = store.data_version()?;
        if last != Some(v) {
            last = Some(v);
            let text = if jsonout {
                serde_json::to_string(&contract::board(store)?).unwrap_or_default()
            } else {
                let snap = store.snapshot()?;
                format!("{}\n", plain_hinted(plain::board(&snap), snap.cards.is_empty(), explicit))
            };
            if writeln!(out, "{text}").and_then(|_| out.flush()).is_err() {
                return Ok(()); // reader went away
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

/// The `--due` of `add` / `edit`, checked: `None` = no flag, `Some(None)` = `--due none`
/// (clear it), `Some(Some(date))` = set it. The hint names the command that was being run.
fn due_flag(cmd: Option<&Cmd>) -> Result<Option<Option<DueDate>>, BoardError> {
    let (raw, example) = match cmd {
        Some(Cmd::Add { due: Some(d), .. }) => (d, "tb add \"tag: title\" --due 2026-10-09".to_string()),
        Some(Cmd::Edit { id: Some(id), due: Some(d), .. }) => (d, format!("tb edit {id} --due 2026-10-09")),
        _ => return Ok(None),
    };
    DueDate::parse(raw, &example).map(Some)
}

fn open_board(name: &str, create: bool) -> Result<Store, BoardError> {
    let path = boards::path_for(name);
    if !path.exists() {
        if create {
            let store = Store::open(&path)?.named(name);
            warn!("created board '{name}'");
            return Ok(store);
        }
        // A missing non-default board is a typo until shown otherwise: fail with the
        // existing boards and the create hint instead of acting on an empty phantom
        // (reads showed "no cards", `next`/`take` silently created it on disk).
        // (TB_DB pins one file per board name — no boards dir, no list, no gate.)
        if name != boards::DEFAULT_BOARD && terminal_board::env("DB").is_none() {
            let names = boards::list();
            let all = if names.is_empty() { "none yet".to_string() } else { names.join(", ") };
            return Err(BoardError(format!(
                "no board '{name}' — boards: {all} · create it with 'tb {name} add \"…\"'"
            )));
        }
        // the default board keeps today's behaviour: reads show it empty, writes create it
        return Ok(Store::open(Path::new(":memory:"))?.named(name));
    }
    Ok(Store::open(&path)?.named(name))
}

fn list_boards(json_out: bool) -> Result<(), BoardError> {
    let def = boards::default_name();
    // the same rows the board picker (B) shows inside the TUI
    let rows = boards::rows()?;
    if json_out {
        let v: Vec<_> = rows
            .iter()
            .map(|b| {
                let c = b.counts;
                serde_json::json!({"name": b.name, "default": b.is_default, "todo": c[0], "doing": c[1], "review": c[2], "done": c[3]})
            })
            .collect();
        println!("{}", pretty(&v));
    } else if rows.is_empty() {
        say!("no boards yet — 'tb add \"title\"' creates '{def}', 'tb home add \"title\"' creates 'home'");
    } else {
        for b in &rows {
            let (n, c) = (&b.name, b.counts);
            say!(
                "{} {n:<16} todo {:<3} doing {:<3} review {:<3} done {}",
                if b.is_default { "*" } else { " " },
                c[0], c[1], c[2], c[3]
            );
        }
        say!("* = default (plain 'tb'); open another with 'tb NAME'");
    }
    Ok(())
}

/// Text given as a file (`--desc-file PATH|-`, `note --file PATH|-`) becomes the plain text
/// the command would have carried: read here, BEFORE the board is opened or created, so a bad
/// path never leaves a new empty board behind, and the write paths below stay the ones
/// `--desc` and note text already use. Every write path trims the blank space around a
/// description or a note, so one file always leaves the same text whichever command
/// carried it.
fn text_from_files(cmd: &mut Cmd) -> Result<(), BoardError> {
    match cmd {
        Cmd::Add { desc, desc_file: Some(path), .. } => {
            *desc = textin::read(path, "add \"tag: title\" --desc-file")?.trim().to_string();
        }
        Cmd::Edit { id: Some(id), desc, desc_file: Some(path), .. } => {
            *desc = Some(textin::read(path, &format!("edit {id} --desc-file"))?);
        }
        Cmd::Note { id, text, file: Some(path) } => {
            *text = Some(textin::read(path, &format!("note {id} --file"))?);
        }
        _ => {}
    }
    Ok(())
}

fn run(mut cli: Cli, positional: Option<String>) -> Result<(), BoardError> {
    // an explicit but blank `--as` (e.g. `--as "$NAME"` with NAME unset) must never
    // silently lose to the fallback chain — refuse before anything is written
    if cli.actor.as_deref().is_some_and(|a| a.trim().is_empty()) {
        return Err(BoardError(
            "--as is empty — pass your agent name, e.g. --as bot-1 (or drop the flag to use TB_AS/the pane's agent)".to_string(),
        ));
    }
    let actor = resolve_actor(cli.actor.as_deref());
    if !terminal_board::env("DB").is_some() {
        match boards::migrate(&boards::old_state_dir(), &boards::state_dir()) {
            Ok(notes) => {
                for n in notes {
                    warn!("tb: {n}");
                }
            }
            Err(e) => warn!("tb: could not migrate the legacy board: {e}"),
        }
    }
    if matches!(cli.cmd, Some(Cmd::Boards)) {
        return list_boards(cli.json);
    }
    let env = terminal_board::env("BOARD");
    let name = boards::select(positional.as_deref(), cli.board.as_deref(), env.as_deref())?;
    // TB_DB pins ONE file: a board NAME would silently alias it (every name opens the same
    // file while JSON/header claim the typed name). Refuse the mix; bare/default still works.
    if terminal_board::env("DB").is_some() && name != boards::DEFAULT_BOARD {
        return Err(BoardError(
            "TB_DB is set — board names are ignored; unset TB_DB to use boards".to_string(),
        ));
    }
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
    if let Some(cmd) = cli.cmd.as_mut() {
        text_from_files(cmd)?;
    }
    let cmd_ref = cli.cmd.as_ref();
    // a `--due` that is not a date is refused before the board file is opened (or created)
    let due_arg = due_flag(cmd_ref)?;
    // on a named board only `add` and `config` (and bare `tb` in a terminal) may create it;
    // every other command on a missing board must fail with the boards list + create hint
    let creates = cmd_ref.map_or(tty, |c| {
        matches!(c, Cmd::Add { .. } | Cmd::Config { .. })
            || (c.writes() && name == boards::DEFAULT_BOARD)
    });
    // many cards from one file: read and checked BEFORE the board is opened, and a dry run or a
    // file with problems never creates a board
    let bulk = match cmd_ref {
        Some(Cmd::Import { file, dry_run }) => Some(import::Request::read(import::Mode::Import, file, *dry_run)?),
        Some(Cmd::Edit { from: Some(file), dry_run, .. }) => Some(import::Request::read(import::Mode::Edit, file, *dry_run)?),
        // (the parser cannot say this: --from conflicts with ID, which switches its own rule off)
        Some(Cmd::Edit { from: None, dry_run: true, .. }) => {
            return Err(BoardError(
                "--dry-run goes with --from; a single edit has no dry run — drop it, or 'tb edit --from FILE.json --dry-run'".into(),
            ))
        }
        _ => None,
    };
    let creates = creates && bulk.as_ref().is_none_or(import::Request::will_write);
    let mut store = open_board(&name, creates)?;
    // hints carry the board name only when it was chosen explicitly in this shell
    let explicit = explicit_board(positional.as_deref(), cli.board.as_deref());
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
            let snap = store.snapshot()?;
            print_lines!("{}", plain_hinted(plain::board(&snap), snap.cards.is_empty(), explicit));
        }
        return Ok(());
    };
    if let Some(request) = bulk {
        return import::run(&mut store, request, &actor, j, &|text| with_board(text, explicit));
    }
    let now = terminal_board::store::now();
    match cmd {
        Cmd::Add { title, desc, checks, .. } => {
            let id = store.add(&title, &desc, &checks, &actor)?;
            if let Some(Some(date)) = &due_arg {
                store.set_due(id, Some(date), &actor)?;
            }
            done_card(&store, j, id, format!("added #{id} — take it with {}", cmd_hint(explicit, &format!("take {id}"))))?;
        }
        Cmd::List => {
            let snap = store.snapshot()?;
            if j {
                let ctx = store.due_ctx()?;
                println!("{}", pretty(&snap.cards.iter().map(|c| ctx.with(c, c)).collect::<Vec<_>>()));
            } else {
                print_lines!("{}", plain_hinted(plain::list(&snap), snap.cards.is_empty(), explicit));
            }
        }
        Cmd::Show { id } => {
            let d = store.show(id)?;
            if j {
                println!("{}", pretty(&store.due_ctx()?.with(&d, &d.card)));
            } else {
                print_lines!("{}", plain::detail(&d, now));
            }
        }
        Cmd::Board => {
            if j {
                println!("{}", pretty(&contract::board(&store)?));
            } else {
                let snap = store.snapshot()?;
                print_lines!("{}", plain_hinted(plain::board(&snap), snap.cards.is_empty(), explicit));
            }
        }
        Cmd::Watch { events, since } => watch(&store, j, events, since, explicit)?,
        Cmd::Agents => {
            let list = agents_now(&store)?;
            if j {
                println!("{}", pretty(&list));
            } else if list.is_empty() {
                say!("no agents (herdr not available or no agent panes)");
            } else {
                for a in list {
                    let card = a.card_id.map(|c| format!("#{c}")).unwrap_or_else(|| "-".into());
                    say!("{:<16} {:<8} {:<8} {:<8} {card}  {}", a.name, a.harness, a.status, a.pane_id, a.job.unwrap_or_default());
                }
            }
        }
        Cmd::Next { review: true } => {
            let card = store.next_review(&actor)?;
            let human = format!(
                "{}\nreviewing by {actor} — check it against its Done criteria, then 'tb done {id}' with a note of what you checked, or 'tb move {id} doing \"what is missing\"' to send it back",
                plain::detail(&store.show(card.id)?, now).trim_end(),
                id = card.id
            );
            done_card(&store, j, card.id, human)?;
        }
        Cmd::Next { .. } | Cmd::Take { .. } => {
            let card = match cmd {
                Cmd::Take { id } => store.take(id, &actor)?,
                _ => store.next(&actor)?,
            };
            let human = format!(
                "{}\ntaken by {actor} — log progress with {}, finish with {}",
                plain::detail(&store.show(card.id)?, now).trim_end(),
                cmd_hint(explicit, &format!("note {} \"...\"", card.id)),
                cmd_hint(explicit, &format!("done {}", card.id))
            );
            done_card(&store, j, card.id, human)?;
        }
        Cmd::Note { id, text, .. } => {
            store.note(id, &text.unwrap_or_default(), &actor)?;
            done_card(&store, j, id, format!("noted #{id}"))?;
        }
        Cmd::Check { id, n, add, rm } => {
            let human = match (n, add, rm) {
                (_, _, Some(r)) => {
                    store.remove_check(id, r, &actor)?;
                    format!("#{id} item {r} deleted, the rest renumbered — see {}", cmd_hint(explicit, &format!("show {id}")))
                }
                (_, Some(text), None) => {
                    let n = store.add_check(id, &text, &actor)?;
                    format!("#{id} item {n} added — toggle it with {}", cmd_hint(explicit, &format!("check {id} {n}")))
                }
                (Some(n), None, None) => {
                    let on = store.check(id, n, &actor)?;
                    format!("#{id} item {n} {}", if on { "checked" } else { "unchecked" })
                }
                (None, None, None) => {
                    return Err(BoardError(format!(
                        "give an item number, --add or --rm — {}, {}",
                        cmd_hint(explicit, &format!("check {id} 1")),
                        cmd_hint(explicit, &format!("check {id} --add \"text\""))
                    )))
                }
            };
            done_card(&store, j, id, human)?;
        }
        Cmd::Move { id, column, reason, force } => {
            if column.eq_ignore_ascii_case("done") {
                guard_done(&store, id, force, &format!("move {id} done"))?;
            }
            let c = store.move_opts(id, &column, &actor, force, reason.as_deref())?;
            let human = if reason.is_some() {
                let round = store.show(id)?.round;
                format!(
                    "#{id} is back in doing with {} (round r{round}) — they fix it and 'tb done {id}' again",
                    c.owner.as_deref().unwrap_or("its owner")
                )
            } else {
                format!("#{id} is now in {}", c.column)
            };
            done_card(&store, j, id, human)?;
        }
        Cmd::Done { id, force, approve } => {
            if approve {
                if store.card(id)?.column != "review" {
                    return Err(BoardError(format!(
                        "#{id} is not in review — approval records a review pass; move it to review first"
                    )));
                }
                // the self-approval rule (#11) applies to --approve too: the card's author
                // cannot record their own approval
                if store.author(id)?.is_some_and(|a| a.eq_ignore_ascii_case(&actor)) {
                    return Err(BoardError(format!(
                        "you did this work — ask another person or agent to approve #{id}"
                    )));
                }
                store.note_kind(id, &actor, "approved (the card stays in review; done waits for the merge)", "approved")?;
                let human = format!("#{id} approved by {actor} — it stays in review until the gh# PR merges");
                done_card(&store, j, id, human)?;
                return Ok(());
            }
            if store.card(id)?.column != "doing" {
                guard_done(&store, id, force, &format!("done {id}"))?;
            }
            let c = if force { store.done_forced(id, &actor)? } else { store.done(id, &actor)? };
            let human = if c.column == "review" {
                format!("#{id} is now in review — close it with {} once verified", cmd_hint(explicit, &format!("done {id}")))
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
                    format!("#{id} blocked — clear it with {}", cmd_hint(explicit, &format!("block {id} --clear")))
                }
                (None, false) => {
                    return Err(BoardError(format!(
                        "say what blocks it — {} or {}",
                        cmd_hint(explicit, &format!("block {id} \"#7\"")),
                        cmd_hint(explicit, &format!("block {id} --clear"))
                    )))
                }
            };
            done_card(&store, j, id, human)?;
        }
        Cmd::Drop { id, force } => {
            if force {
                store.drop_card_forced(id, &actor)?;
            } else {
                store.drop_card(id, &actor)?;
            }
            done_card(&store, j, id, format!("#{id} is back in todo, unowned"))?;
        }
        Cmd::Rm { id } => {
            let before = contract::card_by_id(&store, id)?;
            let c = store.delete_card(id, &actor)?;
            if j {
                println!("{}", pretty(&json!({"ok": true, "card": before})));
            } else {
                say!("deleted #{id} \"{}\"", c.title);
            }
        }
        Cmd::Prio { id, how } => {
            let c = store.reorder(id, &how.to_ascii_lowercase(), &actor)?;
            done_card(&store, j, id, format!("#{id} is now at position {} in {}", c.position + 1, c.column))?;
        }
        Cmd::Edit { id, title, desc, .. } => {
            let id = id.expect("the parser requires ID unless --from is given, and --from is handled above");
            // `--due` alone is a whole edit; with --title/--desc the date (already checked)
            // is written after them
            if due_arg.is_none() || title.is_some() || desc.is_some() {
                store.edit(id, title.as_deref(), desc.as_deref(), &actor, None)?;
            }
            if let Some(date) = &due_arg {
                store.set_due(id, date.as_ref(), &actor)?;
            }
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
            let snap = r.map_err(|e| BoardError(format!("github: {e} — {}", github::fetch_hint(&e, "tb sync"))))?;
            let cards = store.list()?;
            // one lookup per not-done ref the open lists can't vouch for: its state, or a 404
            // (no such issue or PR), or a failed call (nothing known — never reported as missing)
            let lookups = github::lookup_refs(&repo, &github::needs_state(&snap, &cards));
            let states = github::found_states(&lookups);
            // the page holds only the newest 20 open PRs: look up PRs linking the board's
            // other refs (sync only, capped) so an older issue's PR still moves its card
            let mut plan = snap.clone();
            plan.prs.extend(github::linked_prs_beyond_page(&repo, &snap, &github::refs_beyond_page(&snap, &cards)));
            let moves = github::plan_moves(&plan, &cards, &states, &store.returned_at()?);
            github::apply_moves(&mut store, &moves)?;
            let unknown: Vec<i64> =
                lookups.iter().filter(|(_, l)| *l == github::RefLookup::Missing).map(|(n, _)| *n).collect();
            let unchecked: Vec<(i64, &str)> = lookups
                .iter()
                .filter_map(|(n, l)| match l {
                    github::RefLookup::Failed(e) => Some((*n, e.as_str())),
                    _ => None,
                })
                .collect();
            if j {
                let unchecked_n: Vec<i64> = unchecked.iter().map(|(n, _)| *n).collect();
                println!(
                    "{}",
                    pretty(&json!({
                        "ok": true,
                        "moves": moves,
                        "unknown_refs": unknown,
                        "unchecked_refs": unchecked_n,
                    }))
                );
            } else {
                for n in &unknown {
                    say!("gh#{n}: no such issue or PR in {repo} — fix the title with 'tb edit'");
                }
                for (n, e) in &unchecked {
                    say!("gh#{n}: could not check on GitHub ({e}) — try 'tb sync' again");
                }
                if moves.is_empty() {
                    say!("synced {repo}: nothing to move");
                } else {
                    for m in &moves {
                        say!("#{} {} -> {}  ({})", m.card_id, m.from, m.to, m.text);
                    }
                }
            }
        }
        Cmd::Guide => print!("{GUIDE}"),
        Cmd::Import { .. } => unreachable!("handled above"),
        Cmd::Config { key: None, .. } => {
            let all = store.settings()?;
            if j {
                let m: serde_json::Map<String, serde_json::Value> = all.into_iter().map(|(k, v)| (k, json!(v))).collect();
                println!("{}", pretty(&json!({"ok": true, "config": m})));
            } else {
                for (k, v) in all {
                    say!("{k:<13} {v}");
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
                            Some(r) => say!("{r}"),
                            None => say!(
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
                // due dates: `tz` decides what today is, `due-warn` how early a date is `soon`.
                // Without a value each one is READ (its default when the board sets none).
                ("tz" | "due-warn", _) if off => {
                    return Err(BoardError(format!(
                        "--off does not go with {key} — 'tb config tz local' clears the zone, 'tb config due-warn 3' is the default"
                    )))
                }
                ("tz", value) => {
                    let zone = match &value {
                        Some(v) => store.set_tz(v)?,
                        None => store.tz()?.map_or_else(|| due::LOCAL.to_string(), |z| z.name().to_string()),
                    };
                    if value.is_none() && !j {
                        say!("{zone}");
                        return Ok(());
                    }
                    ("tz".into(), json!(zone))
                }
                ("due-warn", Some(value)) => {
                    let n: i64 = value.parse().map_err(|_| {
                        BoardError(format!("due-warn must be a number of days, got '{value}' — try 'tb config due-warn 3'"))
                    })?;
                    store.set_due_warn(n)?;
                    ("due-warn".into(), json!(n))
                }
                ("due-warn", None) => {
                    let n = store.due_warn()?;
                    if !j {
                        say!("{n}");
                        return Ok(());
                    }
                    ("due-warn".into(), json!(n))
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
                    ("github", serde_json::Value::Null) => say!("github panel off for board '{}'", store.name),
                    ("github", r) => say!("github panel on: {} — see it with 'tb github' or 'G' on the board", r.as_str().unwrap_or("")),
                    ("wip", n) => say!("wip limit is now {n}"),
                    ("tz", z) => say!("tz is now {} — it decides what 'today' is for due dates", z.as_str().unwrap_or("")),
                    ("due-warn", n) => say!("due-warn is now {n} — a card is 'soon' from {n} day(s) before its due date"),
                    (k, v) => say!("{k} is now {}", v.as_str().unwrap_or("")),
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
                    say!("{mark} {}", github::repo_row(r, now));
                }
                say!("pick one with 'tb config github owner/repo' or R on the board");
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
                    return Err(BoardError(format!("github: {e} — {}", github::fetch_hint(e, "tb github --refresh"))));
                }
            }
            let view = store.github_view()?;
            let cards = store.list()?;
            if j {
                // raw cached snapshot, plus the factory view per issue (state/who) and the
                // sync state (full error text, consecutive fails, when the snapshot was fetched)
                let (raw, error, fails) = store.github_cache()?;
                let mut v: serde_json::Value = serde_json::from_str(raw.as_deref().unwrap_or("null").trim()).unwrap_or(serde_json::Value::Null);
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
                v["error"] = match &error {
                    Some(e) => json!(e),
                    None => serde_json::Value::Null,
                };
                v["fails"] = json!(fails);
                println!("{}", pretty(&v));
            } else if let Some(s) = &view.snap {
                print_lines!("{}", github::text(s, &cards, view.error.as_deref(), 10, now));
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
            // a bare first word is a board name (`tb work`); a first word followed by a
            // non-command word is a typo'd command — say so instead of silently opening a
            // board that will not exist (`tb frobnicate x`)
            if args.len() > 2
                && args
                    .get(2)
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| !x.starts_with('-') && !boards::COMMANDS.contains(&x))
            {
                return Err(BoardError(format!(
                    "unknown command '{a}' — run 'tb --help' for every command or 'tb guide' for the manual"
                )));
            }
            args.remove(1);
            Ok((Some(a), args))
        }
        _ => Ok((None, args)),
    }
}

/// A `-b NAME` / `--board NAME` read off the raw arguments, for when the parse failed.
fn raw_board_flag(rest: &[std::ffi::OsString]) -> Option<&str> {
    rest.windows(2).find(|w| w[0] == "-b" || w[0] == "--board").and_then(|w| w[1].to_str())
}

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let jsonout = args.iter().any(|a| a == "--json");
    // the board named on the command line (positional or -b), for hints on the error paths too
    let mut explicit: Option<String> = None;
    let parsed = match split_board(args.clone()) {
        Ok((board, rest)) => match Cli::try_parse_from(&rest) {
            Ok(cli) => {
                explicit = explicit_board(board.as_deref(), cli.board.as_deref()).map(str::to_string);
                run(cli, board)
            }
            // --help/--version print their text and succeed, with or without --json
            Err(e) if matches!(e.kind(), clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion) => {
                let _ = e.print();
                return ExitCode::SUCCESS;
            }
            // a parse failure is still a documented `--json` failure: the error object on
            // stdout (non-zero exit), not plain text on stderr with empty stdout
            Err(e) if jsonout => {
                let explicit = explicit_board(board.as_deref(), raw_board_flag(&rest));
                let (what, usage) = parse_error_parts(&e.to_string());
                let more = "see 'tb --help' for every command or 'tb guide' for the manual";
                let hint = match (e.kind(), usage) {
                    (clap::error::ErrorKind::InvalidSubcommand, _) => e
                        .get(clap::error::ContextKind::InvalidSubcommand)
                        .map(|c| format!("unknown command '{c}' — {more}"))
                        .unwrap_or_else(|| more.into()),
                    (_, Some(u)) => format!("usage: {u} — {more}"),
                    (_, None) => more.into(),
                };
                let v = contract::error(&with_board(&format!("argument error: {what} — {hint}"), explicit));
                println!("{}", pretty(&v));
                return ExitCode::from(2); // usage error, as without --json
            }
            // without --json: the parser's own message and exit code, then one line saying
            // what to run next (an unknown command is named)
            Err(e) => {
                let _ = e.print();
                let explicit = explicit_board(board.as_deref(), raw_board_flag(&rest));
                let more = with_board("run 'tb --help' for every command or 'tb guide' for the manual", explicit);
                match e.get(clap::error::ContextKind::InvalidSubcommand) {
                    Some(c) if e.kind() == clap::error::ErrorKind::InvalidSubcommand => {
                        eprintln!("tb: unknown command '{c}' — {more}")
                    }
                    _ => eprintln!("tb: {more}"),
                }
                return ExitCode::from(e.exit_code().clamp(1, 255) as u8);
            }
        },
        Err(e) => Err(e),
    };
    match parsed {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.0 == import::REPORTED => ExitCode::FAILURE,
        Err(e) => {
            let e = BoardError(with_board(&e.0, explicit.as_deref()));
            if jsonout {
                println!("{}", pretty(&contract::error(&e.to_string())));
            } else {
                warn!("tb: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

/// The parser's message as (what went wrong, usage line): the first line without its
/// `error: ` prefix plus any indented detail lines under it (e.g. the missing `<TEXT>`), and
/// the `Usage:` line when there is one.
fn parse_error_parts(msg: &str) -> (String, Option<String>) {
    let mut lines = msg.lines();
    let first = lines.next().unwrap_or("").trim();
    let first = first.strip_prefix("error:").unwrap_or(first).trim();
    let detail: Vec<&str> = lines.by_ref().take_while(|l| !l.trim().is_empty()).map(str::trim).collect();
    let what = if detail.is_empty() { first.to_string() } else { format!("{first} {}", detail.join(", ")) };
    // the parser echoes the global flags it saw; the usage hint is about the command itself
    let usage = msg
        .lines()
        .find_map(|l| l.trim().strip_prefix("Usage:"))
        .map(|u| u.split_whitespace().filter(|w| *w != "--json").collect::<Vec<_>>().join(" "));
    (what, usage)
}
