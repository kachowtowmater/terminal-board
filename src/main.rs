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
use terminal_board::store::{BoardError, Code, Store, COLUMNS};
use terminal_board::{boards, contract, export, filter, github, import, plain, resolve_actor, setup, textin, tui};

const HELP: &str = "\
tb {version} - Terminal Board: one shared task board for people and agents (todo > doing > review > done)
Usage: tb [BOARD] [COMMAND] [--json] [--as NAME] [-b BOARD]   no command: open the board (? = keys)

Cards   add \"tag: title\" [-d DESC] [--check ITEM]... | edit ID | rm ID | restore ID | list [filters] | show ID | note ID \"text\" | note ID --file PATH (- = stdin) | --desc-file PATH | check ID N|--add|--rm | block ID \"#7\"|--clear
Due     add|edit --due YYYY-MM-DD|none   config tz|due-warn|sort
Look    config card-line|label|waiting-lane|wip-counts-blocked|done-by|rules
In/out  import FILE|- | edit --from FILE|- [--dry-run] | export --json|--csv [--history] | log [--since DATE]
Flow    next [--review] | take ID | assign ID NAME | done ID [--force] | drop ID | move ID todo|doing|review|done | move ID doing \"why\" | prio ID top|bottom|up|down
Boards  boards [--default [NAME|--clear]] | boards [--archived] | boards archive|restore NAME | new NAME [--kind K|--from BOARD] | mv ID --to BOARD | board | watch [--json|--events]
Config  config [wip N|theme T|layout L|github OWNER/REPO|--off|file-mode M|github-panel|agents-panel shown|hidden|rm delete|archive]
Hooks   config hook|hook-after NAME|--off | trust [NAME [-- CMD ARG...] [--sha256 HEX|--off]] | move|done|take|next|drop ... --break-glass \"why\"
GitHub  github [--refresh] | github repos | sync
Agents  agents
Setup   setup [--yes] [--github R|--no-github] [--agents-md PATH] [--dry-run]

Options --json (docs/JSON.md) | --as NAME ($TB_AS) | -b NAME ($TB_BOARD; $TB_DB = file) | -h, -V | full reference: README.md
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
    /// Refuse every write: for watching a board you must not change.
    #[arg(long = "read-only", global = true)]
    read_only: bool,
    /// Board to use
    #[arg(short = 'b', long = "board", global = true, value_name = "NAME")]
    board: Option<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

/// The filters `tb list` and `tb board` share. Nothing set = today's output, untouched.
#[derive(clap::Args, Clone, Default)]
struct Filters {
    /// Only this tag (`none` = the cards without one).
    #[arg(long, value_name = "TAG")]
    tag: Option<String>,
    /// Only cards this person or agent holds (`none` = nobody).
    #[arg(long, value_name = "NAME")]
    owner: Option<String>,
    /// Only blocked cards.
    #[arg(long)]
    blocked: bool,
    /// Only cards waiting on this card or name (`#7`, or `alice`).
    #[arg(long = "blocked-on", value_name = "NAME|#ID")]
    blocked_on: Option<String>,
    /// Only cards due before this calendar date.
    #[arg(long = "due-before", value_name = "DATE")]
    due_before: Option<String>,
    /// Only this column — the internal name (todo, doing, review, done), never a label.
    #[arg(long, value_name = "COLUMN")]
    column: Option<String>,
    /// Print the cards grouped: `tag`.
    #[arg(long, value_name = "WHAT")]
    group: Option<String>,
}

impl Filters {
    fn build(&self, look: &terminal_board::store::display::Display) -> Result<filter::Filter, BoardError> {
        filter::Filter::parse(
            filter::Asked {
                tag: self.tag.clone(),
                owner: self.owner.clone(),
                blocked: self.blocked,
                blocked_on: self.blocked_on.clone(),
                due_before: self.due_before.clone(),
                column: self.column.clone(),
                group: self.group.clone(),
            },
            look,
        )
    }
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
        /// The card's tag, instead of the one guessed from a `tag:` prefix in the title.
        #[arg(long, value_name = "KEY")]
        tag: Option<String>,
    },
    List {
        /// The cards `tb rm` archived on an archive board (`tb config rm archive`).
        #[arg(long)]
        archived: bool,
        /// Finished cards only — today's, or every one since `--since`.
        #[arg(long, conflicts_with = "archived")]
        done: bool,
        /// From this local calendar date (`YYYY-MM-DD`), or a unix second.
        #[arg(long, value_name = "DATE", requires = "done")]
        since: Option<String>,
        /// Every board this machine has, not just this one (with `--owner`).
        /// Not with `--done` or `--archived`: "finished today" and "archived" are each one
        /// board's own question (its own zone, its own archive), and answering them across
        /// boards needs a decision about whose day it is — refused rather than guessed.
        #[arg(long = "all-boards", conflicts_with_all = ["archived", "done"])]
        all_boards: bool,
        #[command(flatten)]
        filters: Filters,
    },
    Show { id: i64 },
    Next {
        /// Claim the top REVIEW card you did not do yourself, instead of a TODO card.
        #[arg(long)]
        review: bool,
        /// Skip this board's pre-change hook, and say why (recorded on the card and the board).
        #[arg(long = "break-glass", value_name = "WHY")]
        break_glass: Option<String>,
    },
    Take {
        id: i64,
        /// Skip this board's pre-change hook, and say why (recorded on the card and the board).
        #[arg(long = "break-glass", value_name = "WHY")]
        break_glass: Option<String>,
    },
    /// Hand a specific TODO card straight to NAME, without taking it yourself: `take` done on
    /// someone else's behalf. Only a TODO card is a valid target — same restriction `take`
    /// has, and like `take` there is no `--force` to pull a card away from its current holder.
    Assign { id: i64, name: String },
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
        /// Change a DOING card's checklist when someone else holds it (logged as its own event).
        #[arg(long)]
        force: bool,
    },
    Move {
        id: i64,
        column: String,
        /// Why a REVIEW card goes back to doing (required for that move only)
        reason: Option<String>,
        #[arg(long)]
        force: bool,
        /// Skip this board's pre-change hook, and say why (recorded on the card and the board).
        #[arg(long = "break-glass", value_name = "WHY")]
        break_glass: Option<String>,
    },
    Done {
        id: i64,
        #[arg(long)]
        force: bool,
        /// Record your approval without moving the card (REVIEW stays in REVIEW; the gh#
        /// card still waits for its merge to reach done).
        #[arg(long, conflicts_with = "force")]
        approve: bool,
        /// Skip this board's pre-change hook, and say why (recorded on the card and the board).
        #[arg(long = "break-glass", value_name = "WHY", conflicts_with = "approve")]
        break_glass: Option<String>,
    },
    Block {
        id: i64,
        reason: Option<String>,
        #[arg(long, conflicts_with = "reason")]
        clear: bool,
        /// Block or unblock a DOING card someone else holds (logged as its own event).
        #[arg(long)]
        force: bool,
        /// Who or what the card waits for: a name, or `#ID` (that card unblocks it when done).
        #[arg(long, value_name = "NAME|#ID", conflicts_with = "clear")]
        on: Option<String>,
        /// When to look again: a calendar date, YYYY-MM-DD.
        #[arg(long, value_name = "DATE", conflicts_with = "clear")]
        until: Option<String>,
    },
    Drop {
        id: i64,
        /// Take someone else's DOING card back to todo (logged as its own event).
        #[arg(long)]
        force: bool,
        /// Skip this board's pre-change hook, and say why (recorded on the card and the board).
        #[arg(long = "break-glass", value_name = "WHY")]
        break_glass: Option<String>,
    },
    Rm {
        id: i64,
        /// Remove a DOING card someone else holds (logged as its own event).
        #[arg(long)]
        force: bool,
    },
    /// Bring an archived card back (`tb list --archived` shows them).
    Restore { id: i64 },
    Prio {
        id: i64,
        how: String,
        /// Reorder a DOING card someone else holds (logged as its own event).
        #[arg(long)]
        force: bool,
    },
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
        /// Edit a DOING card someone else holds (logged as its own event).
        #[arg(long)]
        force: bool,
        /// Change many cards from one JSON file (`-` = standard input): rows keyed by `id`,
        /// only the fields present change, all or nothing.
        #[arg(long, value_name = "FILE", conflicts_with_all = ["id", "title", "desc", "due", "desc_file"])]
        from: Option<std::path::PathBuf>,
        /// The card's tag, or `none` to clear it. Digits, spaces and hyphens are allowed.
        #[arg(long, value_name = "KEY|none", conflicts_with = "from")]
        tag: Option<String>,
        /// With --from: report what would change, write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    Config {
        key: Option<String>,
        value: Option<String>,
        #[arg(long)]
        off: bool,
        /// The display text of `config label COLUMN "TEXT"`.
        text: Option<String>,
        /// Read `rules` from a file, byte for byte (`-` = standard input):
        /// `tb config rules --file PATH`.
        #[arg(long = "file", value_name = "PATH", conflicts_with_all = ["value", "text"])]
        file: Option<std::path::PathBuf>,
    },
    /// This machine's trust store for hooks (`tb config hook`, `crate::hooks`). No name: list
    /// every hook this machine knows. A name alone: its state. `NAME -- COMMAND ARG…`: record
    /// what it runs (left UNTRUSTED — the file and digest are printed to check). `NAME --sha256
    /// HEX`: confirm the digest just shown, so it may run. `NAME --off`: forget it.
    Trust {
        name: Option<String>,
        /// The command and its arguments to record, after `--`.
        #[arg(last = true)]
        cmd: Vec<String>,
        /// Confirm the digest `tb trust NAME` just showed, so this hook may run.
        #[arg(long, value_name = "HEX", conflicts_with_all = ["cmd", "off"])]
        sha256: Option<String>,
        /// How long the hook may run before it is stopped and the change refused (default 10).
        #[arg(long, value_name = "SECS")]
        timeout: Option<u64>,
        /// Forget this hook — this machine no longer knows it.
        #[arg(long, conflicts_with_all = ["cmd", "sha256", "timeout"])]
        off: bool,
    },
    /// Attach evidence to a card: a path, a sha or a URL, under a label (`tb show ID` lists
    /// them). tb only stores the text — it never reads, follows or fetches a link.
    Link {
        id: i64,
        value: Option<String>,
        /// What kind of evidence it is — free text, e.g. brief, verdict, commit.
        #[arg(long, value_name = "LABEL")]
        label: Option<String>,
        /// Remove link N instead of adding one.
        #[arg(long, value_name = "N", conflicts_with_all = ["value", "label"])]
        rm: Option<i64>,
    },
    /// List boards. `--default` alone shows the board plain `tb` opens; `--default NAME`
    /// saves it; `--default --clear` goes back to the built-in `default`. `archive NAME` /
    /// `restore NAME` retire or bring back one; `--archived` lists what is archived.
    Boards {
        #[arg(long = "default", value_name = "NAME", num_args = 0..=1, conflicts_with = "what")]
        default: Option<Option<String>>,
        #[arg(long, requires = "default")]
        clear: bool,
        /// `archive` or `restore` (omit to list boards)
        #[arg(value_name = "VERB")]
        what: Option<String>,
        /// The board to archive or restore
        #[arg(value_name = "NAME")]
        name: Option<String>,
        /// List archived boards instead of live ones
        #[arg(long, conflicts_with_all = ["what", "name", "default"])]
        archived: bool,
    },
    /// Make a board with a kind's settings, or with another board's.
    New {
        /// Left out on purpose: `tb new` alone is answered by hand, so a board that is
        /// actually called `new` can be pointed at instead of a bare usage error.
        name: Option<String>,
        /// The kind of board: `default` (as always) or `deadline`.
        #[arg(long, value_name = "KIND", conflicts_with = "from")]
        kind: Option<String>,
        /// Copy the settings — not the cards — of another board.
        #[arg(long, value_name = "BOARD")]
        from: Option<String>,
    },
    Board {
        #[command(flatten)]
        filters: Filters,
    },
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
    /// The whole board, for a person who does not use tb: `--json` re-imports, `--csv` opens
    /// in a spreadsheet.
    Export {
        /// One JSON document: every card with its whole history (the default).
        #[arg(long, conflicts_with = "csv")]
        json_out: bool,
        /// Comma-separated, for a spreadsheet.
        #[arg(long)]
        csv: bool,
        /// With --csv: one row per EVENT instead of one per card.
        #[arg(long)]
        history: bool,
    },
    /// The board's history, oldest first.
    Log {
        /// From this local calendar date (`YYYY-MM-DD`), or a unix second.
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
    },
    /// Move a card to another board, with its checklist and its history.
    Mv {
        id: i64,
        /// The board it goes to. It has to exist already.
        #[arg(long = "to", value_name = "BOARD")]
        to: String,
        /// Move a DOING card someone else holds (logged on the card and on both boards).
        #[arg(long)]
        force: bool,
    },
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
            Cmd::List { .. } | Cmd::Show { .. } | Cmd::Boards { .. } | Cmd::Github { .. } | Cmd::Board { .. } | Cmd::Agents | Cmd::Guide | Cmd::New { .. }
                | Cmd::Export { .. } | Cmd::Log { .. } | Cmd::Trust { .. }
        )
    }
}

/// Does this command change the board? An exhaustive match whose catch-all is `true`, so a
/// command added later counts as a write until somebody says otherwise — the safe way round.
/// (`writes()` above answers a different question: may this command CREATE the board file.)
fn changes_the_board(cmd: &Cmd) -> bool {
    !matches!(
        cmd,
        Cmd::List { .. }
            | Cmd::Show { .. }
            | Cmd::Board { .. }
            | Cmd::Boards { .. }
            | Cmd::Agents
            | Cmd::Guide
            | Cmd::Export { .. }
            | Cmd::Log { .. }
            | Cmd::Watch { .. }
            // reading the settings is a read: `tb config` lists them and `tb config KEY`
            // prints one. Only a VALUE (or `--off`) changes anything.
            | Cmd::Config { key: None, .. }
            | Cmd::Config { value: None, off: false, .. }
            // `tb github repos` asks GitHub, not the board. Plain `tb github` may refresh a
            // stale cache, which IS a write — it is refused by the read-only connection if it
            // gets that far, with the same wording.
            | Cmd::Github { refresh: false, .. }
            // `tb trust` never touches the board file — it reads and writes THIS MACHINE's own
            // settings (`~/.config/terminal-board/config.json`), the same file `--read-only`
            // says nothing about. Gating it here would refuse recording a hook on a pane opened
            // only to watch a board.
            | Cmd::Trust { .. }
    )
}

/// The name a refusal shows for a command.
fn command_name(cmd: &Cmd) -> &'static str {
    match cmd {
        Cmd::Add { .. } => "tb add",
        Cmd::Note { .. } => "tb note",
        Cmd::Check { .. } => "tb check",
        Cmd::Move { .. } => "tb move",
        Cmd::Done { .. } => "tb done",
        Cmd::Block { .. } => "tb block",
        Cmd::Drop { .. } => "tb drop",
        Cmd::Rm { .. } => "tb rm",
        Cmd::Restore { .. } => "tb restore",
        Cmd::Prio { .. } => "tb prio",
        Cmd::Edit { .. } => "tb edit",
        Cmd::Config { .. } => "tb config",
        Cmd::Next { .. } => "tb next",
        Cmd::Take { .. } => "tb take",
        Cmd::Sync => "tb sync",
        Cmd::Import { .. } => "tb import",
        Cmd::Mv { .. } => "tb mv",
        Cmd::New { .. } => "tb new",
        Cmd::Setup { .. } => "tb setup",
        Cmd::Trust { .. } => "tb trust",
        _ => "that command",
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

/// Pretty JSON for stdout. Stored text is cleaned through the display sanitiser before it is
/// serialized (see `text::sanitize_json`), so DEL, C1 and terminal escape sequences never
/// reach a terminal, a log or another tool — whatever wrote them to the board; line breaks
/// are kept. Warnings raised so far ride along as `"warnings": […]` on object-shaped output
/// (additive; absent when there are none — see `notice`).
fn pretty<T: serde::Serialize>(v: &T) -> String {
    let text = serde_json::to_string_pretty(&terminal_board::clean_json(v))
        .unwrap_or_else(|_| "null".into());
    // a warning quotes what it is about — a `TB_BOARD` from the environment, a path, the
    // board's own `tz` setting — so it goes through the same cleaner the body does
    let warnings: Vec<String> =
        terminal_board::notice::all().iter().map(|w| terminal_board::text::sanitize_json(w)).collect();
    terminal_board::notice::splice(&text, &warnings)
}

/// Print each warning nobody has printed yet, once, on stderr: `tb: …`.
fn print_warnings() {
    for w in terminal_board::notice::take_unprinted() {
        warn!("tb: {w}");
    }
}

/// The command a hint names: `tb take 1` on the default board, `tb work take 1` on an
/// explicitly named non-default board (a copied hint must not act on the default board).
/// A board picked by `TB_BOARD` travels in the env, so the bare form is right there too.
fn cmd_hint(explicit: Option<&str>, rest: &str) -> String {
    match explicit {
        Some(name) => format!("'tb {name} {rest}'"),
        None => format!("'tb {rest}'"),
    }
}

/// The board named on the command line (a first-argument name or `-b`), unless it is
/// `default`. A board picked by `TB_BOARD` travels in the environment, so it is not
/// "explicit": its hints stay bare on every path.
fn explicit_board<'a>(positional: Option<&'a str>, flag: Option<&'a str>) -> Option<&'a str> {
    let typed = positional.or(flag)?;
    // A hint may drop the typed name only when the bare command is CERTAIN to reach the same
    // board: it is the board plain `tb` opens (the saved default board, else `default`) and
    // no `TB_BOARD` is steering this shell somewhere else. `tb default take 1` on a machine
    // whose saved default is `work` must hint 'tb default …', never a bare 'tb …'.
    let bare_reaches_it = terminal_board::env("BOARD").is_none() && typed == boards::default_name();
    (!bare_reaches_it).then_some(typed)
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
    done_card_extra(store, jsonout, id, human, None)
}

/// `done_card`, with one more top-level JSON field when `extra` is given — additive, never a
/// rename (docs/JSON.md). Used for `tb next`'s once-per-agent rules banner: `--json` must
/// carry it too, or an agent that only reads JSON would never actually receive it, yet the
/// "seen" mark (set by the caller before this runs) would already say it had.
fn done_card_extra(
    store: &Store,
    jsonout: bool,
    id: i64,
    human: String,
    extra: Option<(&str, serde_json::Value)>,
) -> Result<(), BoardError> {
    if jsonout {
        let mut v = json!({"ok": true, "card": contract::card_by_id(store, id)?});
        if let Some((k, val)) = extra {
            v[k] = val;
        }
        println!("{}", pretty(&v));
    } else {
        say_lines!("{human}");
    }
    Ok(())
}

/// This board's rules text, the first time `actor` is shown it — else `None`. Marks it seen
/// (store::rules) as a side effect, so the SAME text is not shown to them again; changing the
/// text with `tb config rules` makes it new again for everyone. Scoped to `tb next` only (both
/// forms): an orchestrator's `tb assign` does not call this, because the agent it names never
/// ran the command that would deliver the banner to them.
fn first_time_rules(store: &Store, actor: &str) -> Result<Option<String>, BoardError> {
    let Some(text) = store.rules()? else {
        return Ok(None);
    };
    if store.rules_seen(actor, &text)? {
        return Ok(None);
    }
    store.mark_rules_seen(actor, &text)?;
    Ok(Some(text))
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
        ), Code::GhIssueOpen));
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
    /// The identity behind `actor`, inlined: a stream has no `actors[]` to look an id up in.
    actor_id: Option<i64>,
    identity: Option<terminal_board::store::actors::Actor>,
}

impl<'a> EventLine<'a> {
    fn of(e: &'a terminal_board::store::Event, identity: Option<terminal_board::store::actors::Actor>) -> Self {
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
        EventLine {
            v: contract::SCHEMA_VERSION,
            ts: e.ts,
            card_id: e.card_id,
            actor: &e.actor,
            kind: &e.kind,
            from,
            to,
            text: &e.text,
            actor_id: e.actor_id,
            identity,
        }
    }
}

/// NDJSON (or plain) board on every change; exits quietly when stdout closes.
/// With `events` (JSON only): one `{v, ts, card_id, actor, kind, from, to, text, actor_id, identity}` line per
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
                let identity = match e.event.actor_id {
                    Some(id) => store.actor_by_id(id)?,
                    None => None,
                };
                let line = EventLine::of(&e.event, identity);
                let text =
                    serde_json::to_string(&terminal_board::clean_json(&line)).unwrap_or_default();
                if writeln!(out, "{text}").and_then(|_| out.flush()).is_err() {
                    return Ok(()); // reader went away
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
    let mut last = None;
    // `days_left` / `due_state` change when the board's day turns, not only on a write: a
    // board with open due dates is sent again at local midnight in the board's zone
    let mut day = None;
    loop {
        let v = store.data_version()?;
        let today = store.due_ctx()?.today;
        let turned = due::day_turned(day, today) && store.has_open_due_dates()?;
        day = Some(today);
        if last != Some(v) || turned {
            last = Some(v);
            let text = if jsonout {
                serde_json::to_string(&terminal_board::clean_json(&contract::board(store)?))
                    .unwrap_or_default()
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

/// The `--tag` of `add` / `edit`, checked: `None` = no flag, `Some(None)` = `--tag none`
/// (clear it), `Some(Some(key))` = set it.
fn tag_flag(cmd: Option<&Cmd>) -> Result<Option<Option<String>>, BoardError> {
    let (raw, example) = match cmd {
        Some(Cmd::Add { tag: Some(t), .. }) => (t, "tb add \"title\" --tag filing".to_string()),
        Some(Cmd::Edit { id: Some(id), tag: Some(t), .. }) => (t, format!("tb edit {id} --tag filing")),
        _ => return Ok(None),
    };
    terminal_board::store::closing::clean_tag(raw, &example).map(Some)
}

/// `tb new NAME [--kind KIND | --from BOARD]`: make a board and give it a kind's settings,
/// or another board's. A board of the default kind is exactly the board tb always made.
fn new_board(name: &str, kind: Option<&str>, from: Option<&str>, actor: &str, jsonout: bool) -> Result<(), BoardError> {
    use terminal_board::store::kinds;
    // TB_DB pins ONE file, so there is no board to make and no board to copy: every name
    // would open the pinned file and rewrite the settings of a board that holds real work.
    // Refused exactly like every other command that names a board.
    if terminal_board::env("DB").is_some() {
        return Err(BoardError("TB_DB is set — board names are ignored; unset TB_DB to use boards".to_string(), Code::DbPinned));
    }
    boards::validate(name)?;
    // the kind and the source board are checked before anything is created
    let kind = match kind {
        Some(k) => kinds::known(k)?,
        None => "default",
    };
    let source = match from {
        Some(other) => {
            if other == name {
                return Err(BoardError(format!("'{name}' cannot copy itself — name another board: 'tb boards'"), Code::InvalidValue));
            }
            boards::validate(other)?;
            let path = boards::path_for(other);
            // atomic against a concurrent archive/restore (#80/#112) — see open_board
            let Some(store) = Store::open_if_exists(&path)? else {
                let all = boards::list();
                let all = if all.is_empty() { "none yet".to_string() } else { all.join(", ") };
                return Err(BoardError(format!("no board '{other}' to copy — boards: {all}"), Code::NoBoard));
            };
            Some(store.named(other))
        }
        None => None,
    };
    let path = boards::path_for(name);
    if path.exists() {
        return Err(BoardError(format!(
            "board '{name}' already exists — open it with 'tb {name}', or give its settings to a new one with 'tb new other-name --from {name}'"
        ), Code::InvalidValue));
    }
    let store = Store::open(&path)?.named(name);
    let what = match &source {
        Some(other) => {
            let (_, skipped) = store.copy_settings_from(other, actor)?;
            let left = if skipped.is_empty() {
                String::new()
            } else {
                format!(" (not {}: each belongs to one board)", skipped.join(", "))
            };
            format!("with the settings of '{}'{left}", other.name)
        }
        None => {
            store.apply_kind(kind, actor)?;
            format!("of kind {}", store.kind()?)
        }
    };
    if jsonout {
        let config: serde_json::Map<String, serde_json::Value> =
            store.settings()?.into_iter().map(|(k, v)| (k, json!(v))).collect();
        println!(
            "{}",
            pretty(&json!({
                "ok": true,
                "board": {"name": name, "kind": store.kind()?, "from": source.as_ref().map(|s| s.name.clone()), "config": config}
            }))
        );
    } else {
        say!("created board '{name}' {what} — open it with 'tb {name}', add work with 'tb {name} add \"tag: title\"'");
    }
    Ok(())
}

fn open_board(name: &str, create: bool) -> Result<Store, BoardError> {
    let path = boards::path_for(name);
    if create {
        if !path.exists() {
            let store = Store::open(&path)?.named(name);
            warn!("created board '{name}'");
            return Ok(store);
        }
        return Ok(Store::open(&path)?.named(name));
    }
    // A command not allowed to create a board (every one but `add`/`config`, on a named,
    // non-default board): `Store::open_if_exists` checks under the SAME lock `archive`/
    // `restore` hold across their whole move (#80/#112), so a concurrent one cannot land
    // between "the board is there" and this actually opening it and resurrect an empty file
    // that then blocks `restore` — a plain `path.exists()` here could, and once did (up to
    // 19 of 100 rounds racing a `note`, before `open_if_exists` existed).
    match Store::open_if_exists(&path)? {
        Some(store) => Ok(store.named(name)),
        // A missing non-default board is a typo until shown otherwise: fail with the
        // existing boards and the create hint instead of acting on an empty phantom
        // (reads showed "no cards", `next`/`take` silently created it on disk).
        // (TB_DB pins one file per board name — no boards dir, no list, no gate.)
        None if name != boards::DEFAULT_BOARD && terminal_board::env("DB").is_none() => {
            let names = boards::list();
            let all = if names.is_empty() { "none yet".to_string() } else { names.join(", ") };
            Err(BoardError(format!(
                "no board '{name}' — boards: {all} · create it with 'tb {name} add \"…\"'"
            ), Code::NoBoard))
        }
        // the default board keeps today's behaviour: reads show it empty, writes create it
        None => Ok(Store::open(Path::new(":memory:"))?.named(name)),
    }
}

fn list_boards(json_out: bool) -> Result<(), BoardError> {
    let def = boards::default_name();
    // the `*` below cannot follow a saved default board that is unusable or gone: say why
    match boards::saved_default_for_read() {
        Err(e) => warn!("tb: {e}"),
        Ok(Some(n)) if !boards::db_pinned() && !boards::path_for(&n).exists() => warn!(
            "tb: the saved default board is '{n}', but there is no board '{n}' — no row is marked; choose another with 'tb boards --default NAME' or go back with 'tb boards --default --clear'"
        ),
        Ok(_) => {}
    }
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

/// `tb boards`'s two verbs and `--archived`. Retiring a board is a MOVE the user can undo:
/// `archive` puts the file in `archive/` and prints the one line that restores it; nothing
/// deletes a board (`tb rm ID` already deletes a CARD, so `tb boards rm` would be a dangerous
/// near-miss — deliberately not offered).
fn boards_cmd(what: Option<&str>, name: Option<&str>, archived: bool, json_out: bool) -> Result<(), BoardError> {
    match (what, name) {
        (None, _) if archived => list_archived(json_out),
        (None, None) => list_boards(json_out),
        (None, Some(n)) => Err(BoardError(format!(
            "'tb boards {n}' is not a command — 'tb boards archive {n}' or 'tb boards restore {n}'? 'tb boards' lists them"
        ), Code::UnknownCommand)),
        (Some(v @ ("archive" | "restore")), None) => Err(BoardError(format!(
            "'tb boards {v}' needs a board name — e.g. 'tb boards {v} scratch' · 'tb boards' lists them"
        ), Code::ArgRequired)),
        (Some("archive"), Some(n)) => {
            let to = boards::archive(n)?;
            if json_out {
                println!(
                    "{}",
                    pretty(&json!({"ok": true, "board": n, "archived": to.display().to_string(), "restore": format!("tb boards restore {n}")}))
                );
            } else {
                say!("archived '{n}' -> {}", to.display());
                say!("restore it with 'tb boards restore {n}'");
            }
            Ok(())
        }
        (Some("restore"), Some(n)) => {
            let (from, to) = boards::restore(n)?;
            if json_out {
                println!(
                    "{}",
                    pretty(&json!({"ok": true, "board": n, "path": to.display().to_string(), "from": from.display().to_string()}))
                );
            } else {
                say!("restored '{n}' from {} -> {}", from.display(), to.display());
                say!("open it with 'tb {n}'");
            }
            Ok(())
        }
        (Some(v), _) => Err(BoardError(format!(
            "unknown 'tb boards' command '{v}' — use 'tb boards', 'tb boards --archived', 'tb boards archive NAME' or 'tb boards restore NAME'"
        ), Code::UnknownCommand)),
    }
}

/// `tb boards --archived`: what `tb boards archive` retired, with card counts read without
/// touching the files, so a restore returns the board exactly as it was.
fn list_archived(json_out: bool) -> Result<(), BoardError> {
    let rows = boards::archived();
    if json_out {
        let v: Vec<_> = rows
            .iter()
            .map(|a| {
                let n = |i: usize| a.counts.map_or(serde_json::Value::Null, |c| json!(c[i]));
                json!({"name": a.name, "archived_at": a.archived_at(), "path": a.path.display().to_string(),
                       "todo": n(0), "doing": n(1), "review": n(2), "done": n(3)})
            })
            .collect();
        println!("{}", pretty(&v));
    } else if rows.is_empty() {
        say!("no archived boards — 'tb boards archive NAME' retires one (it is moved, never deleted)");
    } else {
        for a in &rows {
            let counts = match a.counts {
                Some(c) => format!("todo {:<3} doing {:<3} review {:<3} done {}", c[0], c[1], c[2], c[3]),
                None => "counts unavailable".to_string(),
            };
            say!("  {:<16} archived {}  {counts}", a.name, a.archived_at());
        }
        say!("in {} — bring one back with 'tb boards restore NAME'", boards::archive_dir().display());
    }
    Ok(())
}

/// `tb boards --default` (show) · `--default NAME` (save) · `--default --clear` (back to the
/// built-in `default`): which board plain `tb` opens. A per-user choice kept in the
/// machine-local settings (`machine`), never in a board file. `TB_BOARD` still beats it for
/// one shell, a name on the command line beats both, and `TB_DB` pins one file — there the
/// saved default is ignored, and the answer says so.
fn default_board_cmd(name: Option<&str>, clear: bool, json_out: bool) -> Result<(), BoardError> {
    let changed = match (name, clear) {
        (Some(_), true) => {
            return Err(BoardError(
                "give a board name or --clear, not both — 'tb boards --default NAME' or 'tb boards --default --clear'".into(), Code::InvalidValue,
            ))
        }
        (Some(n), false) => {
            boards::set_default(Some(n))?;
            true
        }
        (None, true) => {
            boards::set_default(None)?;
            true
        }
        (None, false) => false,
    };
    let (opens, source) = boards::plain_board()?;
    // what is saved, whatever beats it in this shell (never read under TB_DB)
    let saved = if source == boards::DefaultSource::Pinned { None } else { boards::saved_default()? };
    // a saved board whose file is gone: plain `tb` refuses, so saying it opens it is a lie
    let gone = saved.as_deref().filter(|n| !boards::db_pinned() && !boards::path_for(n).exists());
    if let Some(n) = gone {
        let fix = format!("there is no board '{n}' — plain 'tb' refuses until you choose another with 'tb boards --default NAME' or go back with 'tb boards --default --clear'");
        if json_out {
            println!("{}", pretty(&json!({"ok": true, "default": opens, "source": source.as_str(), "setting": saved, "missing": true})));
        } else if changed {
            say!("default board is now '{n}', but {fix}");
        } else {
            say!("{n} is the saved default board, but {fix}");
        }
        return Ok(());
    }
    if json_out {
        println!(
            "{}",
            pretty(&json!({"ok": true, "default": opens, "source": source.as_str(), "setting": saved, "missing": false}))
        );
        return Ok(());
    }
    let saved_text = match &saved {
        Some(s) => format!("the saved default board is '{s}'"),
        None => "no default board is saved".to_string(),
    };
    match source {
        boards::DefaultSource::Pinned => {
            say!("{opens} — TB_DB pins one board file, so a saved default board is not used; unset TB_DB to use boards")
        }
        boards::DefaultSource::Env if changed => {
            say!("saved ({saved_text}) — but TB_BOARD={opens} is set in this shell and still wins here; unset TB_BOARD to use the saved one")
        }
        boards::DefaultSource::Env => {
            say!("{opens} — from TB_BOARD in this shell, which beats the saved default ({saved_text}); unset TB_BOARD to use it")
        }
        boards::DefaultSource::Setting if changed => {
            say!("default board is now '{opens}' — plain 'tb' opens it; go back with 'tb boards --default --clear'")
        }
        boards::DefaultSource::Setting => {
            say!("{opens} — the saved default board: plain 'tb' opens it; go back with 'tb boards --default --clear'")
        }
        boards::DefaultSource::Builtin if changed => {
            say!("default board is back to '{opens}' — choose another with 'tb boards --default NAME'")
        }
        boards::DefaultSource::Builtin => {
            say!("{opens} — the built-in default; choose another with 'tb boards --default NAME'")
        }
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
        Cmd::Config { key, value, file: Some(path), .. } if key.as_deref() == Some("rules") => {
            *value = Some(textin::read(path, "config rules --file")?);
        }
        _ => {}
    }
    Ok(())
}

/// `tb mv ID --to BOARD`. Everything that can be refused is refused BEFORE either board is
/// written: the destination has to exist (tb never creates a board here — see the note in
/// `store::transfer` about issue #112), it has to be a different board, and `TB_DB` pins one
/// file so there is no second board to move to.
fn move_card(
    store: &mut Store,
    id: i64,
    to: &str,
    actor: &str,
    force: bool,
) -> Result<terminal_board::store::transfer::Moved, BoardError> {
    if terminal_board::env("DB").is_some() {
        return Err(BoardError(
            "TB_DB pins one board file, so there is no other board to move a card to — unset TB_DB to use boards".into(), Code::DbPinned,
        ));
    }
    boards::validate(to)?;
    if to == store.name {
        return Err(BoardError(format!(
            "#{id} is already on '{to}' — name the board it should go to, e.g. 'tb mv {id} --to home'"
        ), Code::InvalidValue));
    }
    // the holder rule, exactly as `tb rm` and `tb edit` apply it: a card somebody else holds
    // in DOING is not taken off their board by someone walking past. `--force` is offered
    // because the refusal offers it, and because a move is no more final than `tb rm --force`
    // — it is logged on the card, which survives at the far end, and on both boards' logs.
    let forced = store.holder_check(id, actor, force, &format!("move it to {to}"))?;
    // atomic against a concurrent archive/restore of `to` (#80/#112): a plain exists() check
    // here could land in the gap and, on a missing board, `Store::open` would resurrect an
    // empty one to fail into — see open_board's doc comment for the full story.
    let names_err = || {
        let names = boards::list();
        let all = if names.is_empty() { "none yet".to_string() } else { names.join(", ") };
        // the command names the DESTINATION board, so it is written unquoted: `with_board`
        // rewrites a quoted `'tb …'` to carry the board the CURRENT command is on, which
        // would turn this into a command for the wrong board
        BoardError(format!(
            "no board '{to}' — boards: {all} · a card only moves to a board that exists: make it first with  tb {to} add \"…\""
        ), Code::NoBoard)
    };
    let Some(mut dest) = Store::open_if_exists(&boards::path_for(to))?.map(|s| s.named(to)) else {
        return Err(names_err());
    };
    store.move_to_board(id, &mut dest, actor, forced.as_deref())
}

/// `tb list --all-boards --owner NAME`: one person's work wherever it is. Reads every board
/// this machine has; a board that cannot be read is named and skipped, never fatal, because
/// the point is to find the work that IS there.
fn across_boards(f: &filter::Filter, json_out: bool, explicit: Option<&str>) -> Result<(), BoardError> {
    if terminal_board::env("DB").is_some() {
        return Err(BoardError(
            "TB_DB pins one board file, so there is only one board to look at — unset TB_DB to use boards".into(), Code::DbPinned,
        ));
    }
    let names = boards::list();
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for name in &names {
        // `names` is a snapshot of `boards_dir()` taken above: a board archived in the window
        // between that listing and this read must be skipped, not recreated empty and read as
        // if it had always been that (#80/#112) — `open_if_exists` is the same atomic check
        // `open_board` uses.
        let store = match Store::open_if_exists(&boards::path_for(name)) {
            Ok(Some(s)) => s.named(name),
            Ok(None) => continue,
            Err(e) => {
                warn!("tb: board '{name}' could not be read ({e}) — skipped");
                continue;
            }
        };
        let snap = store.snapshot()?;
        let ctx = store.due_ctx()?;
        for c in f.apply(snap.listed(), &snap.blocks) {
            if json_out {
                let card = snap.blocks.with(snap.display.with(ctx.with(c, c), c), c);
                let mut v = serde_json::to_value(&card).unwrap_or(serde_json::Value::Null);
                if let Some(o) = v.as_object_mut() {
                    // which board it is on: the one thing a cross-board row needs that a
                    // single-board row does not
                    o.insert("board".into(), json!(name));
                }
                rows.push(v);
            } else {
                lines.push(format!("{:<12} {:<7} {}", name, c.column, plain::card_head(c)));
            }
        }
    }
    if json_out {
        println!("{}", pretty(&rows));
        return Ok(());
    }
    if lines.is_empty() {
        let what = if f.any() { format!(" matching {}", f.describe()) } else { String::new() };
        let where_ = if names.is_empty() { "no boards yet".to_string() } else { format!("{} boards", names.len()) };
        say!("no cards{what} on any board ({where_}) — {}", cmd_hint(explicit, "boards"));
        return Ok(());
    }
    for l in &lines {
        say!("{l}");
    }
    Ok(())
}

fn run(mut cli: Cli, positional: Option<String>) -> Result<(), BoardError> {
    // `TB_NOW` changes what tb WRITES to the durable store, so it is validated HERE: the
    // first statement of `run()`, before any command is dispatched and before anything can
    // open a board. Every other placement leaks — the full-screen board, `tb setup` and the
    // bulk `import` / `edit --from` path all return before the command match below, and each
    // of them writes. One gate, at the top; `store::now()` stays a plain library read.
    let pinned = terminal_board::store::pinned_now()?;
    // an explicit but blank `--as` (e.g. `--as "$NAME"` with NAME unset) must never
    // silently lose to the fallback chain — refuse before anything is written
    if cli.actor.as_deref().is_some_and(|a| a.trim().is_empty()) {
        return Err(BoardError(
            "--as is empty — pass your agent name, e.g. --as bot-1 (or drop the flag to use TB_AS/the pane's agent)".to_string(), Code::EmptyActor,
        ));
    }
    // `--read-only` is the same switch as TB_READONLY, set for this process before any board
    // is opened — the store reads it when it decides how to open the connection.
    if cli.read_only {
        std::env::set_var("TB_READONLY", "1");
    }
    let readonly = terminal_board::store::access::readonly_env();
    if readonly {
        if let Some(cmd) = cli.cmd.as_ref() {
            if changes_the_board(cmd) {
                return Err(terminal_board::store::access::refusal(command_name(cmd)));
            }
        }
    }
    let actor = resolve_actor(cli.actor.as_deref());
    // `github` is the name tb's own GitHub sync acts under, and the store lets that name move
    // a card someone holds (its moves are evidence-driven). Nobody else may carry it: a write
    // — or the full-screen board — under that name is refused before anything opens.
    let acts = match &cli.cmd {
        None => std::io::stdout().is_terminal(),
        Some(c) => c.writes() && !matches!(c, Cmd::Watch { .. }),
    };
    if acts && actor.trim().eq_ignore_ascii_case("github") {
        return Err(BoardError(
            "'github' is the name tb's own GitHub sync acts under — pass your own name, e.g. --as bot-1".to_string(), Code::InvalidValue,
        ));
    }
    // every event this process writes also records who `actor` is (store::actors)
    terminal_board::store::actors::use_environment();
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
    // TB_DB and TB_BOARD both set: the pinned file wins and the name is dropped, with one
    // warning — an environment that names a board must not break a harness that pins a file.
    let (env, ignored) = boards::env_board();
    if let Some(n) = ignored {
        terminal_board::notice::push(format!(
            "TB_DB is set, so TB_BOARD={n} is ignored and the pinned file is used — unset TB_BOARD (or TB_DB) to stop this warning"
        ));
    }
    if let Some(Cmd::New { name, kind, from }) = &cli.cmd {
        let Some(name) = name else {
            // a board really called `new` is reachable, and this is where to say so
            let hint = if boards::path_for("new").exists() && terminal_board::env("DB").is_none() {
                " — the board called 'new' opens with 'tb -b new' (or TB_BOARD=new)"
            } else {
                ""
            };
            return Err(BoardError(format!(
                "say what to call the board — 'tb new filings --kind deadline'{hint}"
            ), Code::ArgRequired));
        };
        return new_board(name, kind.as_deref(), from.as_deref(), &actor, cli.json);
    }
    if let Some(Cmd::Boards { default, clear, what, name, archived }) = &cli.cmd {
        if what.is_some() || name.is_some() || *archived {
            return boards_cmd(what.as_deref(), name.as_deref(), *archived, cli.json);
        }
        return match default {
            Some(name) => default_board_cmd(name.as_deref(), *clear, cli.json),
            None => list_boards(cli.json),
        };
    }
    // TB_DB > a name on the command line > TB_BOARD > the saved default board > `default`
    let name = boards::resolve(positional.as_deref(), cli.board.as_deref(), env.as_deref())?;
    // TB_DB pins ONE file: a board NAME would silently alias it (every name opens the same
    // file while JSON/header claim the typed name). Refuse the mix; bare/default still works.
    if terminal_board::env("DB").is_some() && name != boards::DEFAULT_BOARD {
        return Err(BoardError(
            "TB_DB is set — board names are ignored; unset TB_DB to use boards".to_string(), Code::DbPinned,
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
    let tag_arg = tag_flag(cmd_ref)?;
    // on a named board only `add` and `config` (and bare `tb` in a terminal) may create it;
    // every other command on a missing board must fail with the boards list + create hint
    let creates = cmd_ref.map_or(tty, |c| {
        matches!(c, Cmd::Add { .. } | Cmd::Config { .. })
            || (c.writes() && name == boards::DEFAULT_BOARD)
    });
    // many cards from one file: read and checked BEFORE the board is opened, and a dry run or a
    // file with problems never creates a board
    let bulk = match cmd_ref {
        Some(Cmd::Import { file, dry_run }) => Some(import::Request::read(import::Mode::Import, file, *dry_run, false)?),
        Some(Cmd::Edit { from: Some(file), dry_run, force, .. }) => {
            Some(import::Request::read(import::Mode::Edit, file, *dry_run, *force)?)
        }
        // (the parser cannot say this: --from conflicts with ID, which switches its own rule off)
        Some(Cmd::Edit { from: None, dry_run: true, .. }) => {
            return Err(BoardError(
                "--dry-run goes with --from; a single edit has no dry run — drop it, or 'tb edit --from FILE.json --dry-run'".into(), Code::InvalidValue,
            ))
        }
        _ => None,
    };
    let creates = creates && bulk.as_ref().is_none_or(import::Request::will_write);
    let mut store = open_board(&name, creates)?;
    // a stored `tz` this build does not know must not be ignored silently: today then comes
    // from this machine's zone, and every command says so until the setting is fixed. Scoped
    // to `store.notice_key()` (the one function that computes this — see its doc comment on
    // `store::conn_notice_key`), so the full-screen board only ever drains warnings that are
    // actually about itself, whatever shape the board's path was given in.
    //
    // `config actors` is NOT checked here, and nothing else about access is either. It is
    // enforced in `Store::log`, which every card change goes through — the full-screen board
    // calls the store directly and would walk past a check at this layer. `tb config` stays
    // ungated because it writes no card event, which is what lets a list nobody satisfies be
    // corrected.
    if let Some(bad) = store.unknown_tz()? {
        let msg = format!(
            "this board's tz '{bad}' is not a time zone this version knows — 'today' is taken from this machine's zone until you set it again: 'tb config tz America/Los_Angeles' (or 'tb config tz local')"
        );
        match store.notice_key() {
            Some(k) => terminal_board::notice::push_for(&k, msg),
            None => terminal_board::notice::push(msg),
        }
    }
    // hints carry the board name only when it was chosen explicitly in this shell
    let explicit = explicit_board(positional.as_deref(), cli.board.as_deref());
    let cmd = cli.cmd;
    let j = cli.json;
    // before `watch` or plain/json output takes over the terminal. The one exception is the
    // full-screen board: printing here would flash on the real screen for an instant and then
    // be hidden behind the alternate screen it switches to, so it shows its own warnings in
    // the status line as they arrive instead (see `tui::run`'s use of `App::reload`).
    if !(cmd.is_none() && tty) {
        print_warnings();
    }
    let Some(cmd) = cmd else {
        if tty && readonly {
            return Err(BoardError(
                "read-only mode: the full-screen board changes cards as you use it — watch with 'tb board', 'tb list' or 'tb watch --json' instead".to_string(),
                terminal_board::store::Code::ReadOnly,
            ));
        }
        if tty {
            return tui::run(store, &actor)
                .map_err(|e| BoardError(format!("terminal error: {e} — try 'tb list'"), Code::TerminalError));
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
    // the pin was validated at the top of `run()`, before any of the early returns above
    let now = pinned.unwrap_or_else(terminal_board::store::now);
    match cmd {
        Cmd::Add { title, desc, checks, .. } => {
            let id = store.add_tagged(&title, &desc, &checks, &actor, tag_arg.as_ref().map(|t| t.as_deref()))?;
            if let Some(Some(date)) = &due_arg {
                store.set_due(id, Some(date), &actor)?;
            }
            done_card(&store, j, id, format!("added #{id} — take it with {}", cmd_hint(explicit, &format!("take {id}"))))?;
        }
        Cmd::List { archived: true, filters, .. } => {
            let f = filters.build(&store.display()?)?;
            if let Some(flag) = f.not_for_archived() {
                return Err(BoardError(format!(
                    "{flag} does not apply to an archived card — an archived card keeps only its title, tag, column and owner; drop it, or ask the live board with {}",
                    cmd_hint(explicit, "list")
                ), Code::InvalidValue));
            }
            let all = store.archived()?;
            let cards: Vec<_> = all
                .into_iter()
                .filter(|c| f.keeps_archived(c.tag.as_deref(), c.owner.as_deref(), &c.column))
                .collect();
            if j {
                println!("{}", pretty(&cards));
            } else if cards.is_empty() && f.any() {
                say!("no archived cards match {} — loosen it, or {}", f.describe(), cmd_hint(explicit, "list --archived"));
            } else if cards.is_empty() {
                say!("no archived cards — 'tb rm ID' archives instead of deleting once the board says 'tb config rm archive'");
            } else {
                for c in &cards {
                    let tag = c.tag.as_deref().map(|t| format!("{t} - ")).unwrap_or_default();
                    let owner = c.owner.as_deref().map(|o| format!(" - {o}")).unwrap_or_default();
                    say!(
                        "#{} {}  [{tag}was {}{owner} - archived {} ago by {}]",
                        c.id,
                        c.title,
                        c.column,
                        terminal_board::store::fmt_age(now - c.archived_at),
                        c.archived_by
                    );
                }
                say!("bring one back with {}", cmd_hint(explicit, "restore ID"));
            }
        }
        Cmd::List { done: true, since, filters, .. } => {
            // D10: the board's DONE column shows the last 24 hours, so finished work older
            // than that is invisible. `--since` moves the boundary to a local calendar day.
            let tz = store.tz()?;
            let from = match &since {
                Some(raw) => export::since_value(raw, tz, "tb list --done --since")?,
                None => now - terminal_board::store::DONE_WINDOW_SECS,
            };
            let f = filters.build(&store.display()?)?;
            let found = export::done_since(&store, from)?;
            let snap = store.snapshot()?;
            let cards: Vec<terminal_board::store::Card> =
                f.apply(found.iter().collect(), &snap.blocks).into_iter().cloned().collect();
            if j {
                let ctx = store.due_ctx()?;
                let rows: Vec<_> =
                    cards.iter().map(|c| snap.blocks.with(snap.display.with(ctx.with(c, c), c), c)).collect();
                println!("{}", pretty(&rows));
            } else if cards.is_empty() {
                let narrowed = if f.any() { format!(" matching {}", f.describe()) } else { String::new() };
                let what = match &since {
                    Some(raw) => format!("no cards finished{narrowed} since {raw}"),
                    None => format!("no cards finished{narrowed} today"),
                };
                say!("{what} — look further back with {}", cmd_hint(explicit, "list --done --since 2026-10-09"));
            } else {
                for c in &cards {
                    say!("{:<16} {}", export::local_time(c.column_since, tz), plain::card_head(c));
                }
            }
        }
        Cmd::List { all_boards: true, filters, .. } => {
            let f = filters.build(&store.display()?)?;
            return across_boards(&f, j, explicit);
        }
        Cmd::List { filters, .. } => {
            let snap = store.snapshot()?;
            let f = filters.build(&snap.display)?;
            // A filter removes rows and nothing else, so each output form is filtered in the
            // order that form already uses: `--json` follows `listed()`, and the plain list
            // follows `plain::list`, which walks the columns in turn.
            if j {
                let ctx = store.due_ctx()?;
                let rows: Vec<_> = f
                    .apply(snap.listed(), &snap.blocks)
                    .into_iter()
                    .map(|c| snap.blocks.with(snap.display.with(ctx.with(c, c), c), c))
                    .collect();
                println!("{}", pretty(&rows));
                return Ok(());
            }
            let in_print_order: Vec<&terminal_board::store::Card> =
                terminal_board::store::COLUMNS.iter().flat_map(|col| snap.in_column(col)).collect();
            let kept = f.apply(in_print_order, &snap.blocks);
            if !f.any() && f.group.is_none() {
                print_lines!("{}", plain_hinted(plain::list(&snap), snap.cards.is_empty(), explicit));
            } else if kept.is_empty() {
                say!("no cards match {} — loosen it, or {}", f.describe(), cmd_hint(explicit, "list"));
            } else if f.group.is_some() {
                for (tag, cards) in filter::Filter::groups(&kept) {
                    say!("{tag}");
                    print_lines!("{}", plain::list_of(&snap, &cards, "  "));
                }
            } else {
                print_lines!("{}", plain::list_of(&snap, &kept, ""));
            }
        }
        Cmd::Show { id } => {
            let d = store.show(id)?;
            if j {
                let (snap, ctx) = (store.snapshot()?, store.due_ctx()?);
                println!("{}", pretty(&snap.blocks.with(snap.display.with(ctx.with(&d, &d.card), &d.card), &d.card)));
            } else {
                let snap = store.snapshot()?;
                print_lines!("{}", plain::detail_all(&d, now, &snap.display, Some(&plain::waiting_for(&d.card, &snap))));
            }
        }
        Cmd::Board { filters } => {
            let f = filters.build(&store.display()?)?;
            if j {
                println!("{}", pretty(&contract::board_where(&store, &f)?));
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
                    let card = match (a.card_id, a.card_role) {
                        (Some(c), Some("reviewer")) => format!("review #{c}"),
                        (Some(c), _) => format!("#{c}"),
                        _ => "-".into(),
                    };
                    let pane = if a.pane_id.is_empty() { "-" } else { a.pane_id.as_str() };
                    // tb does not read other boards: an agent that is not here is only named
                    let job = a.job.unwrap_or_default();
                    let rest = if a.on_board { job } else { format!("(not on this board) {job}").trim_end().to_string() };
                    say!("{:<16} {:<8} {:<8} {:<8} {card}  {rest}", a.name, a.harness, a.status, pane);
                }
            }
        }
        Cmd::Next { review: true, .. } => {
            let card = store.next_review(&actor)?;
            let rules = first_time_rules(&store, &actor)?;
            let banner = rules.as_deref().map(|r| format!("this board's rules:\n{r}\n\n")).unwrap_or_default();
            let human = format!(
                "{banner}{}\nreviewing by {actor} — check it against its Done criteria, then 'tb done {id}' with a note of what you checked, or 'tb move {id} doing \"what is missing\"' to send it back",
                plain::detail_on(&store.show(card.id)?, now, &store.display()?).trim_end(),
                id = card.id
            );
            done_card_extra(&store, j, card.id, human, rules.map(|r| ("rules", json!(r))))?;
        }
        Cmd::Next { .. } | Cmd::Take { .. } => {
            let is_next = matches!(cmd, Cmd::Next { .. });
            let card = match cmd {
                Cmd::Take { id, break_glass } => store.take_bg(id, &actor, break_glass.as_deref())?,
                Cmd::Next { break_glass, .. } => store.next_bg(&actor, break_glass.as_deref())?,
                _ => unreachable!(),
            };
            // `tb take ID` is a deliberate, targeted pick — not the "first tb next" moment a
            // board's rules are meant to greet, so only `tb next` (either form) shows them.
            let rules = if is_next { first_time_rules(&store, &actor)? } else { None };
            let banner = rules.as_deref().map(|r| format!("this board's rules:\n{r}\n\n")).unwrap_or_default();
            let human = format!(
                "{banner}{}\ntaken by {actor} — log progress with {}, finish with {}",
                plain::detail_on(&store.show(card.id)?, now, &store.display()?).trim_end(),
                cmd_hint(explicit, &format!("note {} \"...\"", card.id)),
                cmd_hint(explicit, &format!("done {}", card.id))
            );
            done_card_extra(&store, j, card.id, human, rules.map(|r| ("rules", json!(r))))?;
        }
        Cmd::Assign { id, name } => {
            let card = store.assign(id, &name, &actor)?;
            let name = card.owner.clone().unwrap_or(name);
            let human = format!(
                "{}\nassigned to {name} by {actor} — {name} logs progress with {}, finishes with {}",
                plain::detail_on(&store.show(card.id)?, now, &store.display()?).trim_end(),
                cmd_hint(explicit, &format!("note {} \"...\"", card.id)),
                cmd_hint(explicit, &format!("done {}", card.id))
            );
            done_card(&store, j, card.id, human)?;
        }
        Cmd::Note { id, text, .. } => {
            store.note(id, &text.unwrap_or_default(), &actor)?;
            done_card(&store, j, id, format!("noted #{id}"))?;
        }
        Cmd::Check { id, n, add, rm, force } => {
            // the holder rule (store/archive.rs): the checklist on someone else's DOING card
            // is not theirs to rewrite, unless forced — a note stays open to everyone
            let (what, did) = match (n, &add, rm) {
                (_, _, Some(_)) => ("remove it", "removed a check from"),
                (_, Some(_), _) => ("add to it", "added a check to"),
                (Some(_), _, _) => ("tick it", "checked"),
                (None, None, None) => ("", ""),
            };
            let forced = if n.is_some() || add.is_some() || rm.is_some() {
                store.holder_check(id, &actor, force, what)?
            } else {
                None
            };
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
                    ), Code::ArgRequired))
                }
            };
            if let Some(owner) = forced {
                store.log_forced(id, &actor, did, &owner)?;
            }
            done_card(&store, j, id, human)?;
        }
        Cmd::Link { id, value, label, rm } => {
            let human = match (value, label, rm) {
                (_, _, Some(n)) => {
                    store.remove_link(id, n, &actor)?;
                    format!("#{id} link {n} deleted, the rest renumbered — see {}", cmd_hint(explicit, &format!("show {id}")))
                }
                (Some(v), Some(label), None) => {
                    let item = store.add_link(id, &v, &label, &actor)?;
                    format!("#{id} link {} added ({label}) — see {}", item.idx, cmd_hint(explicit, &format!("show {id}")))
                }
                (None, _, None) => {
                    return Err(BoardError(format!(
                        "give a value and --label, or --rm N — {}, {}",
                        cmd_hint(explicit, &format!("link {id} PATH|SHA|URL --label brief")),
                        cmd_hint(explicit, &format!("link {id} --rm 1"))
                    ), Code::ArgRequired))
                }
                (Some(_), None, None) => {
                    return Err(BoardError(format!(
                        "say what it is — {}",
                        cmd_hint(explicit, &format!("link {id} PATH|SHA|URL --label brief"))
                    ), Code::ArgRequired))
                }
            };
            done_card(&store, j, id, human)?;
        }
        Cmd::Move { id, column, reason, force, break_glass } => {
            // An internal column name is resolved FIRST and always wins: a board that labels
            // one column with another's name (an older tb allowed it; `config label` now
            // refuses it) must never make the real column unreachable. Only a word that is no
            // column at all is looked up as a label, to name the column the command takes.
            if terminal_board::store::display::column_named(&column).is_err() {
                if let Some(real) = store.display()?.column_of_label(&column) {
                    return Err(BoardError(format!(
                        "'{}' is a display label, not a column — the column is {real}: {}",
                        column.trim(),
                        cmd_hint(explicit, &format!("move {id} {real}"))
                    ), Code::InvalidValue));
                }
            }
            if column.eq_ignore_ascii_case("done") {
                guard_done(&store, id, force, &format!("move {id} done"))?;
            }
            let c = store.move_opts_bg(id, &column, &actor, force, reason.as_deref(), break_glass.as_deref())?;
            let human = if reason.is_some() {
                let round = store.show(id)?.round;
                format!(
                    "#{id} is back in doing with {} (round r{round}) — they fix it and 'tb done {id}' again",
                    c.owner.as_deref().unwrap_or("its owner")
                )
            } else {
                format!("#{id} is now in {}", store.display()?.typed(&c.column))
            };
            done_card(&store, j, id, human)?;
        }
        Cmd::Done { id, force, approve, break_glass } => {
            if approve {
                // every card, not only a gh# one: it records that somebody checked this and
                // leaves the card in review (store/closing.rs)
                let c = store.approve(id, &actor)?;
                let waits = match c.gh_ref {
                    Some(n) => format!("it stays in review until the gh#{n} PR merges"),
                    None => format!("it stays in review — close it with {}", cmd_hint(explicit, &format!("done {id}"))),
                };
                done_card(&store, j, id, format!("#{id} checked by {actor} — {waits}"))?;
                return Ok(());
            }
            if store.card(id)?.column != "doing" {
                guard_done(&store, id, force, &format!("done {id}"))?;
            }
            let c = store.done_bg(id, &actor, force, break_glass.as_deref())?;
            let human = if c.column == "review" {
                format!(
                    "#{id} is now in {} — close it with {} once verified",
                    store.display()?.typed("review"),
                    cmd_hint(explicit, &format!("done {id}"))
                )
            } else {
                format!("#{id} is done")
            };
            done_card(&store, j, id, human)?;
        }
        Cmd::Block { id, reason, clear, force, on, until } => {
            // the holder rule (store/archive.rs): not someone else's DOING card, unless forced
            let (what, did) = if clear { ("unblock it", "unblocked") } else { ("block it", "blocked") };
            let forced = if reason.is_some() || clear { store.holder_check(id, &actor, force, what)? } else { None };
            // `--on` / `--until` are checked before anything is written
            let on = match &on {
                Some(o) => Some(terminal_board::store::blocks::clean_on(id, o)?),
                None => None,
            };
            let until = match &until {
                Some(u) => DueDate::parse(u, &format!("tb block {id} \"…\" --until 2026-10-09"))?
                    .ok_or_else(|| BoardError(format!("'--until none' says nothing — give a date, or clear the block: 'tb block {id} --clear'"), Code::InvalidValue))?
                    .as_str()
                    .to_string()
                    .into(),
                None => None,
            };
            let human = match (reason, clear) {
                (_, true) => {
                    store.block_opts(id, None, None, None, &actor)?;
                    format!("#{id} unblocked")
                }
                (Some(r), false) => {
                    store.block_opts(id, Some(&r), on.as_deref(), until.as_deref(), &actor)?;
                    let mut line = format!("#{id} blocked");
                    if let Some(o) = &on {
                        line.push_str(&format!(" on {o}"));
                        if terminal_board::store::blocks::on_card(o).is_some() {
                            line.push_str(" — it unblocks itself when that card is done");
                        }
                    }
                    if let Some(u) = &until {
                        line.push_str(&format!("; look again on {u}"));
                    }
                    format!("{line} — clear it with {}", cmd_hint(explicit, &format!("block {id} --clear")))
                }
                (None, false) => {
                    return Err(BoardError(format!(
                        "say what blocks it — {} or {}",
                        cmd_hint(explicit, &format!("block {id} \"#7\"")),
                        cmd_hint(explicit, &format!("block {id} --clear"))
                    ), Code::ArgRequired))
                }
            };
            if let Some(owner) = forced {
                store.log_forced(id, &actor, did, &owner)?;
            }
            done_card(&store, j, id, human)?;
        }
        Cmd::Drop { id, force, break_glass } => {
            store.drop_card_bg(id, &actor, force, break_glass.as_deref())?;
            done_card(&store, j, id, format!("#{id} is back in todo, unowned"))?;
        }
        Cmd::Rm { id, force } => {
            let before = contract::card_by_id(&store, id)?;
            let r = store.remove_card(id, &actor, force)?;
            match (j, r.archived) {
                (true, false) => println!("{}", pretty(&json!({"ok": true, "card": before}))),
                (true, true) => println!("{}", pretty(&json!({"ok": true, "card": before, "archived": true}))),
                (false, false) => say!("deleted #{id} \"{}\"", r.card.title),
                (false, true) => say!(
                    "archived #{id} \"{}\" — bring it back with {}",
                    r.card.title,
                    cmd_hint(explicit, &format!("restore {id}"))
                ),
            }
        }
        Cmd::Restore { id } => {
            let c = store.restore(id, &actor)?;
            done_card(&store, j, id, format!("#{id} restored to {} with its history", c.column))?;
        }
        Cmd::Prio { id, how, force } => {
            // the holder rule (store/archive.rs): queue order is the holder's to set, unless
            // forced — like `edit`, not like `note`
            let forced = store.holder_check(id, &actor, force, "reorder it")?;
            let before = store.place(id).ok();
            let c = store.reorder(id, &how.to_ascii_lowercase(), &actor)?;
            let human = format!("#{id} is now at position {} in {}", c.position + 1, c.column);
            if store.sort()?.by_date(&c.column) {
                // on a due-sorted column `position` is only the tie-break: say where the card
                // really is, and what would move it, instead of seeming to do nothing
                let (at, of) = store.place(id)?;
                let was = match before {
                    Some((b, _)) if b != at => format!(" (was {b})"),
                    _ => " (unchanged)".to_string(),
                };
                let note = format!(
                    "this board sorts by due date, so position only orders cards with the same date (or none): #{id} is {at} of {of} in {}{was}; its date decides the rest — {}",
                    c.column,
                    cmd_hint(explicit, &format!("edit {id} --due DATE"))
                );
                if j {
                    println!("{}", pretty(&json!({"ok": true, "card": contract::card_by_id(&store, id)?, "note": note})));
                } else {
                    say_lines!("{human} — {note}");
                }
            } else {
                done_card(&store, j, id, human)?;
            }
            if let Some(owner) = forced {
                store.log_forced(id, &actor, "reordered", &owner)?;
            }
        }
        Cmd::Edit { id, title, desc, force, .. } => {
            let id = id.expect("the parser requires ID unless --from is given, and --from is handled above");
            // the holder rule (store/archive.rs): not someone else's DOING card, unless forced
            let forced = store.holder_check(id, &actor, force, "edit it")?;
            // `--due` alone is a whole edit; with --title/--desc the date (already checked)
            // is written after them
            if due_arg.is_none() || title.is_some() || desc.is_some() || tag_arg.is_some() {
                store.edit_tagged(id, title.as_deref(), desc.as_deref(), &actor, None, tag_arg.as_ref().map(|t| t.as_deref()))?;
            }
            if let Some(date) = &due_arg {
                store.set_due(id, date.as_ref(), &actor)?;
            }
            if let Some(owner) = forced {
                store.log_forced(id, &actor, "edited", &owner)?;
            }
            done_card(&store, j, id, format!("#{id} saved"))?;
        }
        Cmd::Sync => {
            let repo = store.github_repo()?.ok_or_else(|| {
                BoardError(format!(
                    "github is off for board '{}' — turn it on with 'tb config github owner/repo'",
                    store.name
                ), Code::GithubOff)
            })?;
            let r = github::fetch(&repo, now);
            store.save_github(&r)?;
            let snap = r.map_err(|e| BoardError(format!("github: {e} — {}", github::fetch_hint(&e, "tb sync")), Code::GithubError))?;
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
        Cmd::Guide => {
            print!("{GUIDE}");
            // a board that sets nothing prints exactly the manual above, byte for byte — the
            // same rule every other setting in this file follows
            if let Some(rules) = store.rules()? {
                print_lines!("\n## This board's rules\n\n{rules}\n");
            }
        }
        Cmd::Export { csv, history, .. } => {
            let format = if csv { export::Format::Csv } else { export::Format::Json };
            let mut out = std::io::stdout().lock();
            export::export(&store, &mut out, format, history)?;
        }
        Cmd::Log { since } => {
            let from = match &since {
                Some(raw) => export::since_value(raw, store.tz()?, "tb log --since")?,
                None => 0,
            };
            let mut out = std::io::stdout().lock();
            export::log(&store, &mut out, from, j)?;
        }
        Cmd::Mv { id, to, force } => {
            let moved = move_card(&mut store, id, &to, &actor, force)?;
            if j {
                println!(
                    "{}",
                    pretty(&json!({"ok": true, "from_board": moved.from_board, "to_board": moved.to_board,
                                   "old_id": moved.old_id, "id": moved.new_id, "title": moved.title,
                                   "checklist": moved.checklist, "events": moved.events, "links": moved.links}))
                );
            } else {
                say!(
                    "#{} \"{}\" moved to '{}' as #{} (its checklist, {} link(s) and {} events went with it) — it is in TODO, unowned: 'tb {} take {}'",
                    moved.old_id, moved.title, moved.to_board, moved.new_id, moved.links, moved.events, moved.to_board, moved.new_id
                );
            }
        }
        Cmd::Import { .. } => unreachable!("handled above"),
        Cmd::Config { key: None, .. } => {
            let all = store.settings()?;
            if j {
                // `kind` is two facts, so JSON gets two fields instead of one string a
                // reader has to parse; the plain listing keeps its `deadline (changed)`
                let mut m: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
                for (k, v) in all {
                    if k == "kind" {
                        m.insert("kind_changed".into(), json!(v.ends_with(" (changed)")));
                        m.insert("kind".into(), json!(v.trim_end_matches(" (changed)")));
                    } else {
                        m.insert(k, json!(v));
                    }
                }
                println!("{}", pretty(&json!({"ok": true, "config": m})));
            } else {
                for (k, v) in all {
                    say!("{k:<13} {v}");
                }
            }
        }
        Cmd::Config { key: Some(key), value, off, text, file } => {
            if text.is_some() && key != "label" {
                return Err(BoardError(format!(
                    "'{key}' takes one value — only a label has two: 'tb config label review \"WITH REVIEWER\"'"
                ), Code::InvalidValue));
            }
            if file.is_some() && key != "rules" {
                return Err(BoardError(
                    "--file only goes with rules — 'tb config rules --file PATH'".to_string(), Code::InvalidValue,
                ));
            }
            let (k, v): (String, serde_json::Value) = match (key.as_str(), value) {
                // the board's look (store/display.rs): display only, never what a command accepts
                ("label", None) => {
                    return Err(BoardError(
                        "say which column — 'tb config label review \"WITH REVIEWER\"', 'tb config label review' reads it, '--off' clears it".to_string(), Code::ArgRequired,
                    ))
                }
                ("label", Some(column)) => {
                    let column = terminal_board::store::display::column_named(&column)?;
                    let label = match (&text, off) {
                        (Some(_), true) => {
                            return Err(BoardError(format!(
                                "a label and --off together — set it with 'tb config label {column} \"TEXT\"' or clear it with 'tb config label {column} --off'"
                            ), Code::InvalidValue))
                        }
                        (Some(t), false) => store.set_label(column, Some(t))?,
                        (None, true) => store.set_label(column, None)?,
                        (None, false) => store.label(column)?,
                    };
                    let shown = label.clone().unwrap_or_else(|| column.to_ascii_uppercase());
                    if !j {
                        match (&text, off) {
                            (Some(_), _) => say!("{column} is now shown as {shown} — display only: commands and JSON still say {column}"),
                            (None, true) => say!("{column} is shown as {shown} again"),
                            (None, false) => say!("{shown}"),
                        }
                        return Ok(());
                    }
                    (format!("label.{column}"), json!(shown))
                }
                (k @ ("wip-counts-blocked" | "waiting-lane" | "done-needs-note"), _) if off => {
                    let instead = match k {
                        "waiting-lane" => "'tb config waiting-lane hidden' is the default",
                        "done-needs-note" => "'tb config done-needs-note off' is the default",
                        _ => "'tb config wip-counts-blocked yes' is the default",
                    };
                    return Err(BoardError(format!("--off does not go with {key} — {instead}"), Code::InvalidValue));
                }
                // blocks (store/blocks.rs): a blocked card's work slot, and the waiting lane
                ("wip-counts-blocked", value) => {
                    let yes = match &value {
                        Some(v) => store.set_wip_counts_blocked(v)?,
                        None => store.wip_counts_blocked()?,
                    };
                    let text = if yes { "yes" } else { "no" };
                    if value.is_none() && !j {
                        say!("{text}");
                        return Ok(());
                    }
                    ("wip-counts-blocked".into(), json!(text))
                }
                ("waiting-lane", value) => {
                    let shown = match &value {
                        Some(v) => store.set_waiting_lane(v)?,
                        None => store.waiting_lane()?,
                    };
                    let text = if shown { "shown" } else { "hidden" };
                    if value.is_none() && !j {
                        say!("{text}");
                        return Ok(());
                    }
                    ("waiting-lane".into(), json!(text))
                }
                // a closing note (store/closing.rs) — off by default, so a board that sets
                // nothing checks nothing on the way into DONE
                ("done-needs-note", value) => {
                    let on = match &value {
                        Some(v) => store.set_done_needs_note(v)?,
                        None => store.done_needs_note()?,
                    };
                    let text = if on { "on" } else { "off" };
                    if value.is_none() && !j {
                        say!("{text}");
                        return Ok(());
                    }
                    ("done-needs-note".into(), json!(text))
                }
                ("max-rounds", _) if off => {
                    store.set_max_rounds(None)?;
                    ("max-rounds".into(), serde_json::Value::Null)
                }
                // rework rounds (store/rounds.rs) — uncapped by default: no card ever escalates
                ("max-rounds", Some(value)) => {
                    let n: i64 = value.parse().map_err(|_| {
                        BoardError(format!("max-rounds must be a number, got '{value}' — try 'tb config max-rounds 5'"), Code::InvalidValue)
                    })?;
                    store.set_max_rounds(Some(n))?;
                    ("max-rounds".into(), json!(n))
                }
                ("max-rounds", None) => {
                    let n = store.max_rounds()?;
                    if !j {
                        match n {
                            Some(n) => say!("{n}"),
                            None => say!("off — no card is ever marked escalate"),
                        }
                        return Ok(());
                    }
                    ("max-rounds".into(), n.map_or(serde_json::Value::Null, |n| json!(n)))
                }
                ("done-by", _) if off => {
                    store.set_done_by(None)?;
                    ("done-by".into(), serde_json::Value::Null)
                }
                // who may close a card (store/closing.rs) — an honest-mistake stop, not security
                ("done-by", value) => {
                    let names = match &value {
                        Some(v) => store.set_done_by(Some(v))?,
                        None => store.done_by()?,
                    };
                    if value.is_none() && !j {
                        say!("{}", if names.is_empty() { "anyone".to_string() } else { names.join(",") });
                        return Ok(());
                    }
                    ("done-by".into(), json!(names))
                }
                // a board's own conventions (store/rules.rs): free text, printed by 'tb guide'
                // and shown once to each agent on its first 'tb next' since it was set/changed
                ("rules", _) if off => {
                    store.set_rules(None, &actor)?;
                    ("rules".into(), serde_json::Value::Null)
                }
                ("rules", None) => {
                    let text = store.rules()?;
                    if !j {
                        match &text {
                            Some(t) => {
                                print_lines!("{t}\n");
                                return Ok(());
                            }
                            None => {
                                say!(
                                    "rules is off — this board has no house rules set: 'tb config rules \"TEXT\"' or 'tb config rules --file PATH'"
                                );
                                return Ok(());
                            }
                        }
                    }
                    ("rules".into(), json!(text))
                }
                ("rules", Some(value)) => {
                    let text = store.set_rules(Some(&value), &actor)?;
                    ("rules".into(), json!(text))
                }
                ("done-needs-link", _) if off => {
                    store.set_done_needs_link(None)?;
                    ("done-needs-link".into(), serde_json::Value::Null)
                }
                // a card may not reach done without a link carrying this label (store/links.rs,
                // `Store::transition`) — the same honest-mistake shape as `done-by`, and it sits
                // right beside it in the transition's guard order
                ("done-needs-link", value) => {
                    let label = match &value {
                        Some(v) => store.set_done_needs_link(Some(v))?,
                        None => store.done_needs_link()?,
                    };
                    if value.is_none() && !j {
                        say!("{}", label.as_deref().unwrap_or("off"));
                        return Ok(());
                    }
                    ("done-needs-link".into(), json!(label))
                }
                ("kind", _) if off => {
                    return Err(BoardError(
                        "--off does not go with kind — 'tb config kind default' makes it an ordinary board".to_string(), Code::InvalidValue,
                    ))
                }
                // the board's kind (store/kinds.rs): a NAME for a bundle of settings, and a
                // label only — declaring one writes its settings, and never undoes any
                ("kind", value) => {
                    let kind = match &value {
                        Some(v) => store.apply_kind(v, &actor)?.to_string(),
                        None => store.kind_settings()?.first().map(|(_, v)| v.clone()).unwrap_or_else(|| store.kind().unwrap_or("default").to_string()),
                    };
                    if value.is_none() && !j {
                        say!("{kind}");
                        return Ok(());
                    }
                    ("kind".into(), json!(kind))
                }
                ("card-line", _) if off => {
                    return Err(BoardError("--off does not go with card-line — 'tb config card-line age' is the default".to_string(), Code::InvalidValue))
                }
                ("card-line", value) => {
                    let line = match &value {
                        Some(v) => store.set_card_line(v)?,
                        None => store.card_line()?,
                    };
                    if value.is_none() && !j {
                        say!("{}", line.as_str());
                        return Ok(());
                    }
                    ("card-line".into(), json!(line.as_str()))
                }
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
                    github::check_repo(&repo).map_err(|e| BoardError(e, Code::GithubError))?;
                    store.set_github(Some(&repo))?;
                    ("github".into(), json!(repo))
                }
                ("wip", Some(value)) => {
                    let n: i64 = value.parse().map_err(|_| {
                        BoardError(format!("wip must be a number, got '{value}' — try 'tb config wip 3'"), Code::InvalidValue)
                    })?;
                    store.change_wip(n, &actor)?;
                    ("wip".into(), json!(n))
                }
                ("wip-per-owner", Some(value)) => {
                    let n: i64 = value.trim().parse().map_err(|_| {
                        BoardError(
                            format!("wip-per-owner must be a number, got '{value}' — try 'tb config wip-per-owner 1' (0 turns it off)"),
                            terminal_board::store::Code::InvalidValue,
                        )
                    })?;
                    store.set_wip_per_owner(n, &actor)?;
                    ("wip-per-owner".into(), json!(n))
                }
                ("wip-per-owner", None) => {
                    let n = store.wip_per_owner()?;
                    if !j {
                        match n {
                            0 => say!("wip-per-owner is off — one person may hold as many cards as the board's wip allows"),
                            n => say!("{n}"),
                        }
                        return Ok(());
                    }
                    ("wip-per-owner".into(), json!(n))
                }
                ("actors", value) if off || value.is_some() => {
                    let names = store.set_actors(if off { "" } else { value.as_deref().unwrap_or_default() }, &actor)?;
                    ("actors".into(), json!(names))
                }
                ("actors", None) => {
                    let names = store.actors_allowed()?;
                    if !j {
                        if names.is_empty() {
                            say!("actors is off — any name may write to this board");
                        } else {
                            say!("{}", names.join(", "));
                        }
                        return Ok(());
                    }
                    ("actors".into(), json!(names))
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
                (k @ ("tz" | "due-warn" | "sort"), _) if off => {
                    let instead = match k {
                        "tz" => "'tb config tz local' clears the zone",
                        "due-warn" => "'tb config due-warn 3' is the default",
                        _ => "'tb config sort position' is the default",
                    };
                    return Err(BoardError(format!("--off does not go with {key} — {instead}"), Code::InvalidValue));
                }
                // `sort`: position (the default) or due — the one order of `tb next`, lists and boards
                ("sort", value) => {
                    let sort = match &value {
                        Some(v) => store.set_sort(v)?,
                        None => store.sort()?,
                    };
                    if value.is_none() && !j {
                        say!("{}", sort.as_str());
                        return Ok(());
                    }
                    ("sort".into(), json!(sort.as_str()))
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
                        BoardError(format!("due-warn must be a number of days, got '{value}' — try 'tb config due-warn 3'"), Code::InvalidValue)
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
                // rm: what `tb rm` and the delete key do — delete (default) or archive
                ("rm", None) => {
                    let mode = store.rm_mode()?;
                    if !j {
                        say!("{mode}");
                        return Ok(());
                    }
                    ("rm".into(), json!(mode))
                }
                ("rm", Some(value)) => ("rm".into(), json!(store.set_rm_mode(&value, &actor)?)),
                // file-mode: who may open the board file (see fsperm)
                ("file-mode", None) => {
                    let text = store.file_mode_text()?;
                    if !j {
                        say!("{text}");
                        return Ok(());
                    }
                    ("file-mode".into(), json!(text))
                }
                ("file-mode", Some(value)) => {
                    let said = store.set_file_mode(&value, &actor)?;
                    if !j {
                        say!("{said}");
                        return Ok(());
                    }
                    ("file-mode".into(), json!(store.file_mode_text()?))
                }
                // the pre/post-change hook (`crate::hooks`, store/gate.rs): the board stores a
                // NAME only — what it runs, and whether this machine trusts it, is `tb trust`'s
                // business, on the machine that opens the board, never this one's
                (k @ ("hook" | "hook-after"), _) if off => {
                    let event = if k == "hook" { terminal_board::hooks::Event::PreChange } else { terminal_board::hooks::Event::PostChange };
                    store.set_hook(event, None, &actor)?;
                    (k.into(), serde_json::Value::Null)
                }
                (k @ ("hook" | "hook-after"), value) => {
                    let event = if k == "hook" { terminal_board::hooks::Event::PreChange } else { terminal_board::hooks::Event::PostChange };
                    match value.as_deref() {
                        Some(name) => {
                            let saved = store.set_hook(event, Some(name), &actor)?;
                            (k.into(), json!(saved))
                        }
                        None => {
                            let name = store.hook(event)?;
                            if !j {
                                say!("{}", name.as_deref().unwrap_or("off"));
                                return Ok(());
                            }
                            (k.into(), json!(name))
                        }
                    }
                }
                _ => {
                    return Err(BoardError(format!(
                        "unknown or incomplete setting '{key}' — use 'tb config wip 3', 'config github owner/repo', 'config theme dark|light'"
                    ), Code::UnknownSetting))
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
                    ("card-line", l) if l.as_str() == Some("due") => {
                        say!("card-line is now due — a dated card shows its due date and the days left where its age was")
                    }
                    ("card-line", _) => say!("card-line is now age — every card shows its age in the column"),
                    ("done-by", serde_json::Value::Null) => say!("done-by is off — anyone may close a card"),
                    ("done-by", n) => say!(
                        "done-by is now {} — only they may close a card. It stops an honest mistake, not an attacker: names are self-asserted and --force is logged but open to all",
                        n.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()
                    ),
                    ("done-needs-link", serde_json::Value::Null) => say!("done-needs-link is off — a card may reach done without a link"),
                    ("done-needs-link", label) => say!(
                        "done-needs-link is now {} — a card needs a link with that label ('tb link ID VALUE --label {}') before it may reach done",
                        label.as_str().unwrap_or(""), label.as_str().unwrap_or("")
                    ),
                    ("done-needs-note", v) if v.as_str() == Some("on") => say!(
                        "done-needs-note is now on — a card needs a note written during the stay it is leaving before it can reach DONE"
                    ),
                    ("done-needs-note", _) => say!("done-needs-note is now off — DONE needs no note (the default)"),
                    ("max-rounds", serde_json::Value::Null) => say!("max-rounds is off — no card is ever marked escalate"),
                    ("max-rounds", n) => say!(
                        "max-rounds is now {} — a card sent back more times than that is marked escalate and skipped by 'tb next' / 'tb next --review' (still visible, still workable directly)",
                        n.as_i64().unwrap_or_default()
                    ),
                    ("kind", k) => say!(
                        "kind is now {} — its settings are written; change any of them whenever you like, the settings always decide",
                        k.as_str().unwrap_or("")
                    ),
                    ("rules", serde_json::Value::Null) => {
                        say!("rules cleared — 'tb guide' and 'tb next' no longer show anything extra")
                    }
                    ("rules", _) => say!("rules set — printed by 'tb guide', and shown once to each agent on its next 'tb next'"),
                    ("wip-counts-blocked", v) if v.as_str() == Some("no") => say!(
                        "wip-counts-blocked is now no — a blocked card frees a work slot (up to the WIP limit of them; past that they count again)"
                    ),
                    ("wip-counts-blocked", _) => say!("wip-counts-blocked is now yes — every DOING card uses a work slot, blocked or not"),
                    ("waiting-lane", v) if v.as_str() == Some("shown") => {
                        say!("waiting-lane is now shown — blocked cards get their own WAITING section; their column, JSON and 'tb next' are unchanged")
                    }
                    ("waiting-lane", _) => say!("waiting-lane is now hidden — blocked cards stay in their column, marked in red"),
                    ("sort", s) if s.as_str() == Some("due") => say!(
                        "sort is now due — TODO and REVIEW show the nearest due date first and 'tb next' takes it; cards without a date follow; equal dates keep their position"
                    ),
                    ("sort", _) => say!("sort is now position — every column is in position order and 'tb next' takes the top card"),
                    ("wip-per-owner", n) if n.as_i64() == Some(0) => {
                        say!("wip-per-owner is now off — one person may hold as many cards as the board's wip allows")
                    }
                    ("wip-per-owner", n) => say!(
                        "wip-per-owner is now {} — nobody may hold more than that many DOING cards at once (the board's wip limit still applies to everyone together)",
                        n.as_i64().unwrap_or_default()
                    ),
                    ("actors", v) if v.as_array().is_some_and(|a| a.is_empty()) => {
                        say!("actors is now off — any name may write to this board")
                    }
                    ("actors", v) => say!(
                        "actors is now {} — any other name is refused, so a typo cannot invent an agent; reads are never refused",
                        v.as_array().map(|a| a.iter().filter_map(|n| n.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()
                    ),
                    (kk @ ("hook" | "hook-after"), serde_json::Value::Null) => say!("{kk} is off"),
                    (kk @ ("hook" | "hook-after"), name) => say!(
                        "{kk} is now {} — this machine (and any other that opens this board) must 'tb trust {}' it before it can run",
                        name.as_str().unwrap_or(""), name.as_str().unwrap_or("")
                    ),
                    (k, v) => say!("{k} is now {}", v.as_str().unwrap_or("")),
                }
            }
        }
        Cmd::Github { what: Some(w), .. } if w == "repos" => {
            let repos = github::list_repos().map_err(|e| BoardError(format!("github: {e}"), Code::GithubError))?;
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
            return Err(BoardError(format!("unknown 'github {w}' — try 'tb github' or 'tb github repos'"), Code::UnknownCommand));
        }
        Cmd::Github { refresh, .. } => {
            let repo = store.github_repo()?.ok_or_else(|| {
                BoardError(format!(
                    "github is off for board '{}' — turn it on with 'tb config github owner/repo'",
                    store.name
                ), Code::GithubOff)
            })?;
            let view = store.github_view()?;
            let fresh = view.snap.as_ref().is_some_and(|s| now - s.fetched_at < github::MAX_AGE_SECS);
            if refresh || !fresh {
                let r = github::fetch(&repo, now);
                store.save_github(&r)?;
                if let (Err(e), None) = (&r, &view.snap) {
                    return Err(BoardError(format!("github: {e} — {}", github::fetch_hint(e, "tb github --refresh")), Code::GithubError));
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
        Cmd::Trust { name: None, .. } => {
            let mut entries = terminal_board::hooks::entries()?;
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            if j {
                let arr: Vec<_> = entries
                    .iter()
                    .map(|(n, e)| {
                        let st = terminal_board::hooks::state(e);
                        json!({"name": n, "argv": e.argv, "path": e.path, "state": st.word(), "timeout_secs": e.timeout_secs})
                    })
                    .collect();
                println!("{}", pretty(&json!({"ok": true, "hooks": arr})));
            } else if entries.is_empty() {
                say!("no hooks are recorded on this machine — 'tb trust NAME -- COMMAND' to add one");
            } else {
                for (n, e) in &entries {
                    let st = terminal_board::hooks::state(e);
                    say!("{:<16} {:<11} {}", n, st.word(), e.argv.join(" "));
                }
            }
        }
        Cmd::Trust { name: Some(name), off: true, .. } => {
            let existed = terminal_board::hooks::forget(&name)?;
            if j {
                println!("{}", pretty(&json!({"ok": true, "name": name, "forgotten": existed})));
            } else if existed {
                say!("'{name}' forgotten — a board that still names it will refuse until it is trusted again");
            } else {
                say!("this machine did not know a hook called '{name}'");
            }
        }
        Cmd::Trust { name: Some(name), sha256: Some(hex), .. } => {
            let (path, digest) = terminal_board::hooks::confirm(&name, &hex)?;
            if j {
                println!(
                    "{}",
                    pretty(&json!({"ok": true, "name": name, "path": path.display().to_string(), "sha256": digest, "trusted": true}))
                );
            } else {
                say!("'{name}' trusted: {} ({digest}) — it will run for events this board asks it for", path.display());
            }
        }
        Cmd::Trust { name: Some(name), cmd, timeout, .. } if !cmd.is_empty() => {
            let (path, digest) = terminal_board::hooks::record(&name, &cmd, timeout)?;
            if j {
                println!(
                    "{}",
                    pretty(&json!({"ok": true, "name": name, "path": path.display().to_string(), "sha256": digest, "trusted": false}))
                );
            } else {
                say!(
                    "'{name}' records {} — NOT yet trusted (you cannot trust what you were not shown). Check it, then: tb trust {name} --sha256 {digest}",
                    path.display()
                );
            }
        }
        Cmd::Trust { name: Some(name), .. } => {
            let Some(e) = terminal_board::hooks::entry(&name)? else {
                return Err(BoardError(
                    format!("this machine does not know a hook called '{name}' — record it with 'tb trust {name} -- COMMAND'"),
                    Code::InvalidValue,
                ));
            };
            let st = terminal_board::hooks::state(&e);
            let now_digest = terminal_board::hooks::resolve(e.argv.first().map(String::as_str).unwrap_or_default())
                .ok()
                .and_then(|p| terminal_board::hooks::sha256::of_file(&p).ok());
            if j {
                println!(
                    "{}",
                    pretty(&json!({
                        "ok": true, "name": name, "argv": e.argv, "path": e.path,
                        "state": st.word(), "sha256": now_digest, "timeout_secs": e.timeout_secs
                    }))
                );
            } else {
                say!("{:<11} {}", st.word(), e.argv.join(" "));
                match &st {
                    terminal_board::hooks::State::Untrusted | terminal_board::hooks::State::Changed { .. } => {
                        if let Some(d) = &now_digest {
                            say!("{} — you cannot trust what you were not shown: tb trust {name} --sha256 {d}", e.path.as_deref().unwrap_or(""));
                        }
                    }
                    terminal_board::hooks::State::Trusted => say!("will run for events a board asks it for"),
                    terminal_board::hooks::State::Missing(why) | terminal_board::hooks::State::Unsafe(why) => say!("{why}"),
                }
            }
        }
        Cmd::Boards { .. } | Cmd::Setup { .. } | Cmd::New { .. } => unreachable!("handled above"),
    }
    Ok(())
}

/// `ttyboard home add "x"`: a first argument that isn't a flag or a command is a board name.
fn split_board(mut args: Vec<std::ffi::OsString>) -> Result<(Option<String>, Vec<std::ffi::OsString>), BoardError> {
    let first = args.get(1).and_then(|a| a.to_str()).map(str::to_string);
    match first {
        Some(a) if !a.starts_with('-') && !boards::is_command_word(&a) => {
            boards::validate(&a)?;
            // a bare first word is a board name (`tb work`); a first word followed by a
            // non-command word is a typo'd command — say so instead of silently opening a
            // board that will not exist (`tb frobnicate x`)
            if args.len() > 2
                && args
                    .get(2)
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| !x.starts_with('-') && !boards::is_command_word(x))
            {
                return Err(BoardError(format!(
                    "unknown command '{a}' — run 'tb --help' for every command or 'tb guide' for the manual"
                ), Code::UnknownCommand));
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
                let v = contract::error(&with_board(&format!("argument error: {what} — {hint}"), explicit), Code::Usage);
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
    // warnings raised after the board was opened (a board the picker opened, a late backup)
    print_warnings();
    match parsed {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.0 == import::REPORTED => ExitCode::FAILURE,
        Err(e) => {
            let code = e.1;
            let e = BoardError(with_board(&e.0, explicit.as_deref()), code);
            if jsonout {
                println!("{}", pretty(&contract::error(&e.to_string(), e.1)));
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
