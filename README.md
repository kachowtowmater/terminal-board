# Terminal Board

**Terminal Board** (`tb`) is a to-do board that lives in your terminal. It has four
columns — **TODO → DOING → REVIEW → DONE** — and you move cards across them as work
progresses. It is made to be shared: you, your teammates and your AI coding agents all use
the same board, from the same keyboard-driven screen or from simple commands. It can also
show your GitHub repository (open pull requests, issues, CI) right next to your cards, and
the live status of your agents.

![Terminal Board in a wide pane: four coloured columns, the GitHub panel and the agents panel](docs/images/view-half-horizontal.png)

**What you get**

- A four-column board you drive with the arrow keys: add, move, edit and finish cards.
- Cards with descriptions, checklists and notes, so every piece of work has its own history.
- Your GitHub repo beside the board: open issues, pull requests, CI, and who is on what.
- A live view of your AI agents, and simple `tb` commands they use to take and finish work.
- It fits whatever space you give it: half the screen, a third, or a small corner.

## Why does this exist?

Because we are terminal junkies.

We live in the terminal. Our editor is there, our git is there, our AI agents are there,
arguing with each other in split panes. Then someone says "just check the board", and we
have to open a browser, find the tab among forty-seven other tabs, wait for a JavaScript
framework to boot, log in again, get a cookie banner, and drag a card with a mouse.
A *mouse*.

So we did the reasonable thing and moved the whole board into the terminal. Now our to-do
list sits in a pane next to the code, the agents can read and update it without asking us,
and we never have to leave the place where we already spend twelve hours a day.

Is this healthy? No. Is it faster? Absolutely.

## Contents

- [Why does this exist?](#why-does-this-exist)
- [Requirements](#requirements)
- [Install](#install)
- [Setup (`tb setup`)](#setup-tb-setup)
- [Your first 5 minutes](#your-first-5-minutes)
- [Keys](#keys)
- [Boards](#boards)
- [GitHub](#github)
- [Agents](#agents)
- [Command-line reference](#command-line-reference)
- [Layouts and themes](#layouts-and-themes)
- [Where your data lives](#where-your-data-lives)
- [Testing](#testing)
- [Troubleshooting](#troubleshooting)
- [Uninstall](#uninstall)
- [For app developers (JSON)](#for-app-developers-json)
- [FAQ](#faq)
- [Docs](#docs)
- [License](#license)

## Requirements

- **macOS** (Apple silicon or Intel) or **Linux** (x86_64 or arm64; Arch/Omarchy,
  Debian/Ubuntu and most others).
- A terminal with colours and Unicode box drawing (almost all modern terminals).
- `curl` (or `wget`) and `tar` for the installer. Nothing else: `tb` is one small program.
- Optional: the **GitHub CLI** (`gh`) if you want the GitHub panel.
- Optional: **herdr** if you run AI agents in herdr panes and want the AGENTS panel.
- Only if you build from source: **Rust** (https://rustup.rs).

## Install

### The one command

Paste this into a terminal:

<!-- no-test -->
```sh
curl -fsSL https://raw.githubusercontent.com/kachowtowmater/terminal-board/main/install.sh | bash
```

What it does, step by step:

1. **Finds the right download** for your computer (macOS or Linux, Apple silicon/arm64 or
   Intel/x86_64) from the project's latest GitHub release.
2. **Checks it**: every download comes with a SHA-256 checksum, and the installer refuses a
   file that doesn't match.
3. **Installs `tb`** into `~/.local/bin`. If that folder isn't on your `PATH`, it shows the
   exact line it would add to your shell's startup file (`~/.zshrc`, `~/.bashrc` or fish
   config) and asks first.
4. **Runs `tb setup`**, the setup wizard (next section).

If there is no ready-made download for your machine (or the download fails), the installer
offers to build `tb` from source with Rust — it asks first — or tells you exactly how to.

### Installer options

Pass options after `bash -s --`, for example:

<!-- no-test -->
```sh
curl -fsSL https://raw.githubusercontent.com/kachowtowmater/terminal-board/main/install.sh | bash -s -- --yes --no-github
```

| option | what it does |
|---|---|
| `--prefix DIR` | install `tb` into DIR instead of `~/.local/bin` |
| `--version v1.0.0` | install that release instead of the latest |
| `--no-setup` | install only; run `tb setup` yourself later |
| `--yes` | ask nothing, take the defaults (also passed to `tb setup`) |
| `--github OWNER/REPO`, `--no-github`, `--agents`, `--no-agents`, `--agents-md PATH` | passed to `tb setup` (see below) |
| `--binary PATH` | install this `tb` binary instead of downloading one |
| `--no-build` | never fall back to building from source |
| `--dry-run` | show what would happen, change nothing |
| `--uninstall` | remove Terminal Board (asks separately before deleting your boards) |

### From a clone (or to hack on it)

<!-- no-test -->
```sh
git clone https://github.com/kachowtowmater/terminal-board
cd terminal-board
./install.sh
```

Inside a clone, with Rust installed, `./install.sh` builds that checkout
(`cargo build --release`) instead of downloading. Fully by hand:

<!-- no-test -->
```sh
cargo build --release
mkdir -p ~/.local/bin
cp target/release/tb ~/.local/bin/tb
tb setup
```

## Setup (`tb setup`)

`tb setup` is a short question-and-answer wizard. The installer runs it for you, and the
very first time you type `tb` it runs by itself (press `s` at the first question to skip
straight to the board). Run it again any time with `tb setup`.

Every question is `[y]es / [s]kip`; press Enter to take the default shown in capitals.
Anything that touches the outside world (installing `gh`, logging in, writing files outside
Terminal Board) defaults to **skip** and is only done after you say yes.

1. **Board.** Creates your default (empty) board.
2. **GitHub** (optional). "Connect GitHub?" If yes, it checks that the GitHub CLI is
   installed (if not, it shows the install command and asks before running it) and logged
   in (logging in happens in GitHub's own `gh auth login`, after asking; Terminal Board never
   sees your password or token). Then pick a repository from a numbered list or type
   `owner/repo`; it is checked on GitHub, saved and synced. If you skip, the GitHub panel is
   hidden; switch it on later with `R` on the board.
3. **AGENTS panel.** Show the live view of your herdr agents? (The default is yes when
   herdr is installed.)
4. **Claude Code skill.** Install a skill so Claude Code knows how to use the board
   (`~/.claude/skills/terminal-board/SKILL.md`). Skipped silently when there is no
   `~/.claude` directory; `tb setup --agents` asks anyway.
5. **Agent instructions.** Type the path of an `AGENTS.md` / `CLAUDE.md` and it adds a
   marked block that teaches any AI agent the `tb` commands. Running it again replaces the
   block instead of adding a second one.

At the end you get a summary of what was done and what was skipped.

| `tb setup` option | what it does |
|---|---|
| `--yes` | ask nothing; take the defaults (GitHub and agent extras are skipped unless you pass their options) |
| `--github OWNER/REPO` | connect this repository (needs `gh`, logged in) |
| `--no-github` | skip GitHub and hide its panel |
| `--agents` | show the AGENTS panel and install the skill |
| `--no-agents` | skip the agent extras and hide the AGENTS panel |
| `--agents-md PATH` | add the agent instructions to this `AGENTS.md` / `CLAUDE.md` |
| `--dry-run` | show what would happen, change nothing |

```sh
tb setup --dry-run --yes
```

## Your first 5 minutes

1. **Open the board.** Type `tb` and press Enter. You see four empty columns.
2. **Add a card.** Press `a`, type `home: water the plants` and press Enter. The part before
   the colon (`home`) becomes the card's **tag**. The card appears in TODO.
3. **Move it.** With the card selected, press **Shift+→** (or `>`) to move it to DOING.
   The arrow keys select cards; Shift+arrows move them. Shift+↑/↓ reorder a column.
4. **Open it.** Press **Enter**. A window shows the description, the checklist and the
   history. Press `e` to edit the title and description.
5. **Checklist.** In the open card, press `a` to add a checklist item ("kitchen"), Enter to
   save. Use ↑/↓ to pick an item and Enter to tick it. `esc` closes the card.
6. **Done.** Press `d`: DOING goes to REVIEW (someone checks it), and `d` again moves it to
   DONE. Because the card is your work, the board first asks `approve your own work? y/n`
   — press `y` (on a shared board, someone else does this step). Press `q` to quit.

Everything you did can also be done from the command line — this is how scripts and AI
agents use the board:

```sh
tb add "docs: write the install guide"
tb add "home: water the plants" -d "the big fern too" --check "kitchen" --check "balcony"
tb list
tb take 2
tb note 2 "kitchen done"
tb check 2 1
tb done 2
tb next --review --as bob
tb done 2 --as bob
tb
```

(`tb` on its own, when not in an interactive terminal, just prints the board.)

## Keys

Press `?` on the board to see all keys at any time.

| key | what it does |
|---|---|
| arrows | select a card (←→ column, ↑↓ card) |
| `a` | add a card (`tag: title`) |
| `e` | edit the card: title, due date, description (Tab moves to the next field; an empty date clears it) |
| `x` | delete the card (asks y/n; names the holder of someone else's card; archives on an archive board) |
| `enter` | open the card: description, checklist, history |
| `d` | done: DOING → REVIEW, REVIEW/TODO → DONE (on your own REVIEW card it asks `approve your own work? y/n`) |
| Shift+← / Shift+→ (or `<` `>`) | move the card to the previous / next column |
| Shift+↑ / Shift+↓ (or `K` `J`) | move the card up / down in its column |
| `n` | add a note to the card's history |
| `+` / `-` | raise / lower the WIP limit (with DOING selected) |
| `tab` / Shift+`tab` | go to the next / previous area: columns, GITHUB, AGENTS |
| ↓ from the last card | into the GITHUB / AGENTS panels |
| `B` | boards: switch to another board without quitting |
| `R` | pick the GitHub repository |
| `G` / `A` | show or hide the GITHUB / AGENTS panel |
| `L` | view: auto, focus, third-h, third-v, half-h, half-v |
| `T` | dark / light theme |
| `?` | help |
| `q` | quit |

In an open card: ↑↓ select a checklist item, `enter` ticks it, `a` adds one, `d` deletes
one, `n` adds a note, `e` edits, `esc` closes.

In the GITHUB panel: ↑↓ select, `enter` on the repository line opens the repo picker,
`enter` on a pull request or issue opens it; there `a` adds it to the board as a card and
`o` opens it in your browser.

**WIP limit.** DOING holds at most 3 cards by default ("work in progress" limit). When it is
full, finish something first. Change it with `+`/`-` or `tb config wip 4`.

## Boards

You can have as many boards as you like. `tb` opens the one called `default`. Give a name
to use another — it is created the first time you add to it:

```sh
tb home add "call the plumber"
tb home list
tb -b home list
tb boards
```

**Choose the board plain `tb` opens.** `tb boards --default home` saves it: from then on
`tb`, `tb add …`, `tb next` and every other command without a board name act on `home`, and
`tb boards` and the board picker mark it with `*`.

```sh
tb boards --default home     # plain tb now opens "home"
tb boards --default          # show it, and where it comes from
tb boards --default --clear  # back to the board called "default"
```

The board has to exist — an unknown or archived board is refused — and if it disappears
later, plain `tb` says so instead of quietly making an empty one. It is your choice on this
machine, so it is kept in `~/.config/terminal-board/config.json` (or the file `TB_CONFIG`
names), never inside a board file that someone might copy. That file is yours alone (mode
`0600`), it is written whole or not at all, and two `tb` commands writing it at the same moment
take turns, so neither loses the other's setting. If it is a symbolic link, tb writes the file
the link points at. If tb cannot read it at all, commands that did not ask about a setting say
so once and carry on as if nothing were set.

**Which board a command uses**, first match wins:
`TB_DB` > a board named on the command line > `TB_BOARD` > the saved default board > `default`.
So `TB_BOARD=work` still changes the default for one shell, `tb home …` or `-b home` beats
both, and with `TB_DB` set there is one file: the saved default is ignored there (`tb boards
--default` says so, and saving one is refused). The command in a hint names its board
whenever a bare `tb` would reach a different one, so a copied hint always acts on the board
you were looking at.

On the board, `B` opens the board picker: the same rows as `tb boards` — name, card counts
and the default board marked — and `enter` switches to the one you choose without quitting
`tb`. (`TB_DB` pins a single file, so board names, and the picker, are off in that mode.)

## GitHub

Terminal Board can show one GitHub repository per board: open pull requests with their CI
status, the newest issues with who is working on them, what was merged today and whether
`main` is green.

**Setup.** Install the GitHub CLI (`brew install gh`, `sudo pacman -S github-cli` or
`sudo apt install gh`) and log in with `gh auth login`. Then either press `R` on the board
and pick your repository, or:

<!-- no-test -->
```sh
tb config github acme/widgets
tb github
tb github repos
tb sync
```

`tb config github` checks the repository exists on GitHub before saving it, so a typo fails
there (`no repo 'acme/widgts' on GitHub (or no access)`) instead of in every later sync.

**The panel** shows four tiles (issues, pull requests, merged today, main CI), a table of
pull requests (CI, review, branch and the issue it fixes) and a table of issues with their
**state**: `PR gh#N` (a pull request is open for it), `in progress` (a card for it is in DOING
or REVIEW), `on board` (a card waits in TODO) or `unclaimed`. Red only ever means a
failure: a failing check (`FAIL`), a blocked card, or an idle agent holding a card.

**Cards follow GitHub.** A card with `gh#N` in its title (e.g. `web: gh#123 fix login`) is
linked to issue or pull request N:

- an open pull request for it → the card moves to **REVIEW**;
- the pull request is merged, or the issue is closed → the card moves to **DONE**;
- cards never move backwards on their own. This happens on every refresh (every minute)
  and whenever you run `tb sync`. A card a reviewer sent back stays in DOING until its pull
  request is updated after that.

If you mark such a card done yourself while its issue is still open, the board asks first
(the command line needs `--force`).

**Turn it off / on:**

<!-- no-test -->
```sh
tb config github --off
tb config github-panel hidden
tb config github-panel shown
```

## Agents

Terminal Board is designed so AI coding agents can take work from it, report progress and
move cards with no extra explanation. Everything an agent needs is a `tb` command:

| the agent wants to | it runs |
|---|---|
| take the next card | `tb next --as its-name` |
| read the brief | `tb show ID` |
| log progress | `tb note ID "what changed"` · a long one: `tb note ID --file notes.md` |
| tick a checklist step | `tb check ID N` |
| update the card | `tb edit ID --desc "Done = …"` · `tb edit ID --desc-file brief.md` · `tb check ID --add "step"` |
| say it is stuck | `tb block ID "#N"` |
| finish / hand back | `tb done ID` / `tb drop ID` |
| move it anywhere | `tb move ID review` |

**Teach your agents.** Three ways, pick any:

- `tb guide` prints the complete agent manual ([docs/AGENTS.md](docs/AGENTS.md)): every
  command, recipes, rules, GitHub and JSON.
- `tb setup --agents-md AGENTS.md` (in your project) adds a block to that file with the
  command table above, so every agent that reads `AGENTS.md` / `CLAUDE.md` knows the board.
  The block is [integrations/AGENTS-snippet.md](integrations/AGENTS-snippet.md).
- `tb setup --agents` installs a Claude Code skill
  ([integrations/claude-code/SKILL.md](integrations/claude-code/SKILL.md)).

```sh
tb guide
```

**Brief line for orchestrators.** Paste this into an agent's instructions:

> Your work is on Terminal Board: run `tb next --as <your-name>`, log each step with
> `tb note`, tick `tb check`, and `tb done` when finished (`tb drop` if you stop,
> `tb block` if stuck). Full manual: `tb guide`.

**The AGENTS panel.** It shows who is working on *this* board and on what: everyone who holds
a card that is not done, reviews one, or wrote to a card in the last hour, each with the card
they are on (`review` when they are reviewing it), its last note and the note's age. The board
itself knows all of that, so the panel works with no extra tool. If you run agents in herdr
panes (a terminal multiplexer for coding agents), a pane whose agent name is exactly a name on
the board adds whether it is working or idle; an agent that went idle while still holding a
card is shown in red — it probably stopped halfway. Agents in other panes are counted, not
listed: the header reads `7 agents (4 here, 3 elsewhere)`. Show or hide the panel with `A` or
`tb config agents-panel shown|hidden`; print the same list with `tb agents`
([more](docs/HUMANS.md#watching-agents)).

**Ownership.** A DOING card someone else holds is theirs: `move`, `done`, `drop`, `edit`,
`block`, `rm`, `check` and `prio` on it are refused for anyone else, with `--force` to go
ahead anyway (each override is logged as its own `force` event). `tb note` stays open to
everyone: a progress note adds to a card, it does not take it over.

## Command-line reference

Every command prints a short answer and, when something is wrong, says what to run next.
Add `--json` to any command for machine-readable output.

```sh
tb add "docs: fix typo in README"
tb list
tb show 3
tb next --as alice
tb take 3
tb note 3 "found the typo"
tb check 3 --add "proofread"
tb check 3 1
tb check 3 --rm 1
tb block 1 "#3"
tb block 1 --clear
tb move 3 review
tb done 3 --as bob
tb drop 1
tb prio 1 top
tb edit 1 --title "docs: write the install guide (v2)" --desc "cover macOS and Linux"
tb rm 1
tb board --json
tb config
tb config wip 4
tb config theme dark
tb config layout auto
tb agents
tb --version
```

| command | what it does |
|---|---|
| `tb add "tag: title" [-d DESC \| --desc-file PATH] [--check ITEM]...` | add a card to TODO |
| `tb list` / `tb show ID` | all cards / one card in full |
| `tb next [--as NAME]` | take the top TODO card (atomic: two people never get the same one) |
| `tb next --review [--as NAME]` | claim the top REVIEW card you did not do yourself (atomic too) |
| `tb take ID` | take a specific TODO card |
| `tb note ID "text"` / `tb note ID --file PATH` | add a note to the card's history |
| `tb check ID N` / `--add TEXT` / `--rm N` | tick, add or remove a checklist item (`--force` on someone else's held card, logged) |
| `tb block ID "#N"` / `--clear` | mark blocked by something / unblock (`next` skips blocked cards) |
| `tb block ID "text" --on NAME\|#ID --until DATE` | say who you wait on and when to look again — see [Waiting on something](#waiting-on-something) |
| `tb config wip-counts-blocked yes\|no` / `tb config waiting-lane shown\|hidden` | whether a blocked card uses a work slot / gives blocked cards their own section |
| `tb move ID todo\|doing\|review\|done` | move a card (`--force` to move someone else's DOING card) |
| `tb move ID doing "why"` | send a REVIEW card back to its owner, with the reason (shows `r2`) |
| `tb done ID [--force]` | DOING → REVIEW, REVIEW/TODO → DONE (REVIEW → DONE only by someone else) |
| `tb done ID --approve` | record that you checked a card — any card; it stays in REVIEW (JSON `approved_by`) |
| `tb config done-by NAME,NAME` / `--off` | who may close a card — an honest-mistake stop, **not security**; see [Who closes a card](#who-closes-a-card) |
| `tb config done-needs-note on\|off` | require a note written during the stay being left before a card may reach DONE — see [Rework rounds and a closing note](#rework-rounds-and-a-closing-note) |
| `tb config max-rounds N` / `--off` | a card sent back more than N times is marked `escalate` and skipped by `tb next` / `tb next --review` — see [Rework rounds and a closing note](#rework-rounds-and-a-closing-note) |
| `tb add … --tag KEY` / `tb edit ID --tag KEY\|none` | set the card's tag explicitly (digits, spaces and hyphens allowed) instead of guessing it from the title |
| `tb drop ID [--force]` | give a card back to TODO (`--force` for someone else's) |
| `tb prio ID top\|bottom\|up\|down` | reorder within the column (`--force` on someone else's held card, logged; `note` is always open to everyone) |
| `tb edit ID [--title T] [--desc D \| --desc-file PATH]` | change title/description |
| `tb add … --due DATE` / `tb edit ID --due DATE\|none` | set, change or clear a card's due date — see [Due dates](#due-dates) |
| `tb rm ID [--force]` | delete a card — or archive it, on a board set to `tb config rm archive` |
| `tb list --archived` / `tb restore ID` | the archived cards / bring one back with its checklist and whole history |
| `tb board --json` / `tb watch --json` | the whole board as JSON / a live stream |
| `tb watch --events --json [--since TS]` | one NDJSON line per event instead of the whole board |
| `tb boards` | list your boards |
| `tb new NAME [--kind default\|deadline]` / `[--from BOARD]` | make a board with a kind's settings, or another board's (settings, not cards) — see [A deadline board in one command](#a-deadline-board-in-one-command) |
| `tb boards --default [NAME]` / `--default --clear` | show, save or clear the board plain `tb` opens |
| `tb config [KEY VALUE]` | show or change settings (wip, theme, layout, github, github-panel, agents-panel; `rm delete\|archive`) |
| `tb config tz ZONE\|local` / `tb config due-warn DAYS` | what "today" is for due dates / how early a date counts as `soon` (no value = print it) |
| `tb config sort position\|due` | what orders the board and what `tb next` takes: the top position (default) or the nearest due date — see [Due dates](#due-dates) |
| `tb config card-line age\|due` | what a card line shows where the age is: the age (default), or the due date and the days left |
| `tb config label COLUMN "TEXT"` / `--off` | a display name for `todo`, `doing`, `review` or `done` — **display only**; see [Due dates](#due-dates) |
| `tb github [--refresh]` / `tb github repos` / `tb sync` | GitHub snapshot / your repos / apply GitHub evidence now |
| `tb agents` | who is on this board and the card each holds or reviews, then the other herdr agents |
| `tb guide` | the manual for AI agents |
| `tb setup` | the setup wizard (GitHub, panels, agent instructions) |

Who you are: `--as NAME`, or `TB_AS`, or `HERDR_AGENT_NAME`, or — inside a herdr pane — the
name herdr gives the agent in that pane, or your login name.

**Showing less of the board.** `tb list` and `tb board --json` take filters, and they combine:

<!-- no-test -->
```sh
tb list --tag docs                   # one tag (--tag none = the cards without one)
tb list --owner alice --blocked      # what alice holds that is stuck
tb list --blocked-on '#7'            # everything waiting on card 7 (a name works too)
tb list --due-before 2026-10-09      # dated before that day
tb list --column todo --group tag    # one column, gathered under each tag
tb board --json --tag docs           # the same on the JSON board
```

Filters apply to every way `tb list` picks cards, including `--done` and `--archived`; on an
archived card, which keeps only its title, tag, column and owner, `--blocked`, `--blocked-on`
and `--due-before` are refused by name rather than ignored. A filter **removes rows and
nothing else** — the list stays in the order the board defines, including under
`tb config sort due`. `--column` takes the internal name (`todo`, `doing`,
`review`, `done`), never a display label; a label is refused and the message names the column
to use. Filters that find nothing say what was asked for and exit 0 — that is an answer.

**A card on the wrong board.** `tb mv ID --to BOARD` sends it with its checklist and its whole
history:

<!-- no-test -->
```sh
tb mv 3 --to work
tb list --all-boards --owner alice   # one person's work, wherever it is
```

The card gets a **new number** on the board it arrives at — ids belong to a board, and the old
one may already be taken there. It lands in TODO and unowned, because the other board has its
own work-in-progress limit and its own people; the event log on the card says where it came
from, and both boards record the move. The destination has to exist already, and a card
somebody else is holding is not moved out from under them without `--force`, which is logged
on the card and on both boards.

A move holds the source board for the whole operation, so a `tb note` or `tb edit` that arrives
while it runs waits and then finds the card gone, rather than being told "noted" and thrown
away; two `tb mv` of the same card at the same moment cannot both succeed.

**Which agent, model and session did the work.** The name on a card stays short. Behind it, tb
records who that name was — harness, model, role, session, machine — so a bad batch of work
can be traced back to the session that wrote it. The harness and the session are picked up by
themselves (from what the harness exports, or from herdr inside a herdr pane); the model and
the role are yours to set, because no harness exports them and tb never guesses:

<!-- no-test -->
```sh
export TB_AS=coder-2 TB_MODEL=model-x TB_ROLE=coder   # in the agent's launch script
tb show 3      # … ends with:  actors:
               #   coder-2 — claude-code model-x coder session 0b9f6a52-… on buildbox
```

It is in `--json` too (`actor_id` on every event, `actors[]` next to them). A person in a
plain terminal sets none of this and nothing is recorded beyond the name; a path is never
stored; the record is self-reported. More in [docs/HUMANS.md](docs/HUMANS.md#which-agent-which-model-which-session).

**Long text from a file.** `-d "…"` and `tb note ID "…"` go through your shell, which eats
backticks, `$` and quotes in a long string. `--desc-file PATH` (on `tb add` and `tb edit`) and
`tb note ID --file PATH` read the text from a file instead, byte for byte; `-` reads standard
input:

<!-- no-test -->
```sh
tb add "docs: install guide" --desc-file brief.md
tb edit 3 --desc-file brief.md
tb note 3 --file findings.md
some-command | tb note 3 --file -
tb edit 3 --desc-file - < brief.md
```

The text must be UTF-8 and at most 256 KiB (262144 bytes). Blank space around it is trimmed;
everything between is kept exactly — tabs, blank lines, Windows line ends (a leading
byte-order mark is dropped). An empty file is refused, so a forgotten pipe can never blank a
description; `--desc ""` still clears one on purpose. With `-`, standard input has to be a
pipe or a redirect: on a terminal tb refuses at once instead of waiting for typing. Give the
text once — `--desc` with `--desc-file`, or note text with `--file`, is an argument error.
Control characters are stored as they are and removed whenever the text is shown, like any
other card text.

**Many cards from one file.** `tb import cards.json` creates cards and `tb edit --from
changes.json` changes existing ones, from a JSON file (`-` reads standard input). A row is the
card object `tb show ID --json` and `tb board --json` already print, so a board can be exported,
edited in a script or a spreadsheet tool, and fed back:

<!-- no-test -->
```sh
tb import cards.json --dry-run     # a report per row; nothing is written
tb import cards.json               # [{"title": "docs: write the guide", "due": "2026-10-09", "checklist": ["draft"]}, …]
tb board --json > board.json       # export, change the dates in the file, then:
tb edit --from board.json          # only what differs changes; [{"id": 7, "due": "2026-10-16"}, …] works too
```

- **All or nothing.** Every row is checked first; one bad row and nothing is written. Each
  problem names its row, its card and its field — `row 250 (#41) due: '2026-02-30' is not a
  real calendar date — use YYYY-MM-DD …` — so a long file is fixed in one pass. `--dry-run`
  does the same work and reports exactly what the real run would do. Two imports at the same
  moment wait for each other; they never mix.
- **import** reads `title` (it may carry `tag:` and `gh#N`, as `tb add` accepts), `tag`,
  `gh_ref`, `description`, `due`, `blocked` and `checklist` (texts, or `{"text", "done"}`).
  New cards always land at the bottom of TODO with new ids, and their history starts with an
  `imported` event that says which row of which file, and who ran it.
- **edit --from** follows the same holder rule as `tb edit`: a card somebody else holds in
  DOING is not rewritten from a file, and because the whole file is one change, one such row
  refuses all of it (add `--force` to override, which is logged on each card). It needs `id`
  and changes `title`, `tag`, `gh_ref`, `description`, `due` and
  `blocked` — only the fields present in a row, `null` clears `due`, `blocked` and `tag`, and a
  value the card already has is no change and logs nothing, so running a file twice is
  harmless. Each change is written, and logged, exactly as `tb edit` and `tb block` would.
  It never moves a card or changes its owner.
- Anything else in a row — `column`, `owner`, `position`, timestamps, `events`, fields tb does
  not know — is **ignored with one warning** that lists the fields. History is never imported.

**Getting the board out again.** `tb export` writes the whole board, with its history, for
someone who will never open a terminal:

<!-- no-test -->
```sh
tb export --json > board.json        # every card with its whole history; tb import reads it back
tb export --csv  > board.csv         # one row per card, for a spreadsheet
tb export --csv --history > log.csv  # one row per event instead
tb log --since 2026-10-09            # what happened since a date (--json for a parser)
tb list --done --since 2026-10-01    # finished work older than today
```

- The JSON is the same card object `tb board --json` prints, but with **every** event instead
  of the last ten, wrapped as `{"v":1, "board", "exported_at", "tz", "cards":[…]}`. That
  `cards` array is exactly what `tb import` and `tb edit --from` read, so export → edit →
  import is a round trip.
- The CSV is meant for Excel or Numbers: a byte-order mark so UTF-8 names and dashes are not
  mangled, RFC 4180 quoting (a description keeps its commas, quotes and line breaks in one
  cell), CRLF row ends, and dates written as local `YYYY-MM-DD HH:MM` in the board's zone.
  A cell that would start `=`, `+`, `-` or `@` gets a leading apostrophe, so a card titled
  `=cmd|…` is text in the sheet and never a formula. It is one-way; `--json` is what comes back.
- `--since` takes a calendar date and means **local midnight in the board's zone** (`tb config
  tz`), not a UTC instant, so "since Tuesday" is your Tuesday. A unix second works too.
- All of these only read: they never change the board file.
- Accepted documents: a JSON array of cards, `{"cards": […]}`, the whole `tb board --json`
  object (its columns in board order), or one card object. At most 4 MiB of UTF-8.

### A deadline board in one command

```sh
tb new filings --kind deadline
tb filings add "permits: renew the fire permit" --due 2026-10-09
tb filings add "tax: file the quarterly return" --due 2026-10-15
tb filings next --as anna
tb filings block 2 "waiting for the signed copy" --on "the other side" --until 2026-10-06
tb filings config
tb new matters --from filings
```

Everything below this section is a setting you can change one at a time. A **kind** is a name
for a combination that works together, so you do not have to know all of them on day one:

| kind | what it writes | what it is for |
|---|---|---|
| `default` | nothing at all | the board tb has always made: a priority queue, `tb next` takes the top card |
| `deadline` | `sort due`, `card-line due`, `due-warn 7`, `waiting-lane shown`, `wip-counts-blocked no`, and the column labels TO PREPARE / IN HAND / WITH REVIEWER / FILED | a board of filing dates: the nearest date first, the date on every card line, a week of warning, and whatever you are waiting on in its own section |

`tb new NAME` with no kind makes exactly the board it always did. `tb new NAME --from BOARD`
copies another board's **settings and not its cards**, which is how you give a second matter
the same shape as the first. Three settings belong to one board and never travel — the
command says which it left behind:

| not copied | why |
|---|---|
| `github` | a new board must not start syncing to another board's issues |
| `done-by` | who may close a card is a decision about that board's people |
| `file-mode` | the file's own permissions decide it, and they are set when it is created |

`new` is a command word, so `tb new …` always means the command. A board **called** `new`
(one an older version let you make) is still yours: it is listed by `tb boards` and opens with
`tb -b new` or `TB_BOARD=new`.

**A kind is a label, not a lock.** The settings are yours: change any of them whenever you
like and the board follows the setting, not the name. `tb config` then shows
`kind deadline (changed)` so the name never claims more than it should; putting the setting
back, or `tb config kind default`, clears the mark. Declaring a kind writes its settings and
never undoes one, so it is safe on a board that already holds work.

### Due dates

For a board that tracks deadlines — filing dates, renewals, anything with a day on it:

```sh
tb deadlines add "permits: renew the fire permit" --due 2026-10-09
tb deadlines edit 1 --due 2026-10-16
tb deadlines config tz America/Los_Angeles
tb deadlines config due-warn 5
tb deadlines show 1 --json
tb deadlines edit 1 --due none
```

A due date is a **calendar date**: `YYYY-MM-DD`, stored as you typed it (spaces around it are
dropped). tb never
turns it into a point in time, so it cannot slip to the day before or the day after — not when
the board is read in another time zone, not at 23:59, not across a daylight-saving change.
Anything that is not a real date (`2026-02-30`, `10/09/2026`, `tomorrow`) is refused with the
command to run instead, and nothing is written. Every change is in the card's history
(`due: 2026-10-09 -> 2026-10-16`).

The one thing a time zone decides is what **today** is. `tb config tz America/Los_Angeles`
pins that for the board: everyone who reads it, wherever they sit, counts days from the same
local midnight. `tb config tz local` (the default) uses each machine's own zone. From today,
`--json` output gives every card `days_left` (whole calendar days: 0 = due today, negative =
past) and `due_state` — `overdue`, `soon` (due within `due-warn` days; 3 unless you change it)
or `ok`. A finished card carries neither. `tb config tz` and `tb config due-warn` with no value
print the one in force; `tb config` lists them once the board sets them.

#### A deadline queue: `tb config sort due`

```sh
tb deadlines add "tax: file the quarterly return" --due 2026-10-15
tb deadlines config sort due
tb deadlines list
tb deadlines prio 1 top
tb deadlines next --as alice
```

A board is a priority queue by default: `tb next` takes the top card, and you order cards by
hand (`tb prio`, shift+arrows). `tb config sort due` makes it a deadline queue instead. TODO and
REVIEW show the **nearest due date first** — so an overdue card is on top — and `tb next` (and
`tb next --review`) takes that card, skipping blocked ones as always. Cards without a date come
after every dated card. Cards with the same date, or with none, keep their position order, so
the order is always the same for everyone: `tb next`, `tb list`, `tb board`, every `--json`
board and the full-screen board all sort with one function and cannot disagree. The order
never depends on what today is — dates are compared as dates. DOING stays in position order and
DONE newest first.

Position still matters as the tie-break, so `tb prio` still works — and on a due-sorted column
it tells you where the card really is: `#1 is now at position 1 in todo — this board sorts by
due date, so position only orders cards with the same date (or none): #1 is 2 of 2 in todo
(unchanged); its date decides the rest — 'tb deadlines edit 1 --due DATE'`.
`tb config sort position` goes back; `tb config sort` prints the one in force.
#### The look: the due mark, the card line, your own column names

```sh
tb deadlines config card-line due
tb deadlines config label review "WITH REVIEWER"
tb deadlines config label review
tb deadlines list
tb deadlines config label review --off
```

A card that is due within `due-warn` days, or overdue, carries a **loud mark** on its card line —
`! due in 2d`, `! due today`, `! overdue 3d` (bold on the board, red once overdue) — in the
full-screen board, the focus view and `tb list`. A finished card never does. On a narrow card
line the mark is the last thing to go: it outlives the tag, the checklist count and the age, and
it shrinks in whole words (`! overdue 3d` → `! late 3d` → `! late` → `!`), never inside one.

`tb config card-line due` puts the date where the age is: `permits - alice - due Oct 27 - 18d`
(the days left; a date in another year is written in full). A card without a date still shows
its age. `tb config card-line age` is the default.

`tb config label review "WITH REVIEWER"` names a column in your own words (up to 24
characters; control characters are removed). **Labels are display only.** Every command still
takes the plain names — `tb move 5 review` — and JSON `column` is `todo`, `doing`, `review` or
`done` for ever; the label travels next to it as `column_label` (and `labels` on the board
object). A message that names a column names the one to type: `#5 is now in review (shown as
WITH REVIEWER)`, and typing the label gets `'with reviewer' is a display label, not a column —
the column is review: 'tb move 5 review'`. In a narrow header a label gives way in whole words
and the count is never pushed off; when not even its first word fits, the plain name is shown.
`tb config label review` prints it; `--off` clears it. A column that the board orders by due
date says `by due` in its header.

### Who closes a card

```sh
tb office add "filing: proof of service"
tb office config done-by anna,ben
tb office config done-by
tb office take 1 --as anna
tb office done 1 --as anna
tb office done 1 --approve --as ben
tb office config done-by --off
```

`tb config done-by anna,ben` says who may close a card: tb then refuses to move a card into
DONE as anybody else, and names the people to ask. It guards **every** way into DONE, so
moving a card out of review first is not a way round it.

**It is an honest-mistake stop, not security.** Names in tb are **self-asserted** — `--as` is
whatever the caller types — so this catches the slip of the wrong person closing a card, and
nothing more. `--force` gets past it and is open to everyone; it is recorded as its own event
with the name that used it, and answering `y` to the full-screen board's `approve your own
work?` is the same `--force` by another route. A board that needs real authority needs it outside tb: file
permissions, a repository, a person. (`tb`'s older never-approve-your-own-work rule works the
same way, and still applies: a name on the `done-by` list cannot close its own work either.)
The GitHub sync is exempt — a merged PR closing its card is evidence, not a person.

`tb done ID --approve` records that somebody **checked** a card and leaves it in REVIEW. It
works on every card, not only one linked to an issue, and `done-by` does not gate it: noting
"I looked at this" is not closing it. Each checker is listed once in JSON as `approved_by`.

### Rework rounds and a closing note

```sh
tb office config done-needs-note on
tb office config max-rounds 5
tb office move 1 doing "add the rollback step"        # round 2 — shows r2
tb office done 1 --as anna                             # doing -> review; column_since resets
tb office note 1 "checked the rollback step, looks right"
tb office done 1                                       # closes: a note was written this stay
```

**`tb config done-needs-note on`** refuses to move a card into DONE until somebody has written
a note (`tb note`) **during the stay it is leaving** — a note kept from an earlier round does
not count, so a note from round 1 cannot silently stand in for round 3's close (running `tb
done 1` again right after the send-back above, before the `tb note`, is refused with exactly
that reason and what to run next). Off is the default: a board that sets nothing checks
nothing. `--force` gets past it, logged, the same bargain `done-by` makes; the GitHub sync is
exempt — a merged PR is its own trace.

**`tb config max-rounds 5`** marks a card **`escalate`** (JSON) once it has been sent back more
than 5 times, so a loop between a worker and a reviewer cannot run forever unnoticed.
`escalate` is **derived**, like `recheck` and `due_state` — nothing is stored, and it is
`false` again the moment the card reaches DONE. `tb next` and `tb next --review` skip an
escalated card in their automatic pick; it is never hidden from `tb list`, `tb board` or
`tb show`, and `tb take ID` / `tb move` / `tb done` still work on it directly — only being
handed it by accident is what stops.

### Tags you choose

```sh
tb office add "due 10/9 (file by 10/6): prepare the brief" --tag "00-key 2"
tb office edit 2 --tag "client-a matter 7"
tb office edit 2 --tag none
```

tb guesses a tag from a `tag: title` prefix, and that guess is deliberately narrow: letters,
digits, hyphens and underscores, no spaces, up to 20 characters — so `00-key 2: x`, and a
title whose colon comes later, get no tag at all. `--tag KEY` sets it **explicitly** and
allows digits, spaces and hyphens, so a title like `due 10/9 (file by 10/6): …` can carry a
real tag. `--tag none` clears it.

When you give `--tag`, tb guesses nothing about the title: it is kept exactly as typed,
colon and all (a leading `gh#N` is still pulled out, as always). A tag that no title could
have produced also survives a later title edit, instead of being quietly dropped.

### Waiting on something

```sh
tb deadlines block 2 "waiting for the signed copy" --on "#1" --until 2026-10-09
tb deadlines block 2 "their counsel has it" --on "the other side" --until 2026-10-20
tb deadlines config wip-counts-blocked no
tb deadlines config waiting-lane shown
tb deadlines block 2 --clear
```

`tb block ID "text"` is unchanged. `--on NAME|#ID` says **who** you are waiting for and
`--until DATE` **when to look again**; both ride next to the block text, so a board can be
asked what it is waiting on instead of being read as prose (`blocked_on`, `blocked_until` in
JSON). When the `--until` date arrives the card is a **recheck** — worked out when you read
the board, in the board's `tz`; nothing is stored and no clock runs in the background.

**A card blocked `--on #7` unblocks itself when card 7 reaches DONE**, in the same breath as
that move, recorded as `#7 is done`. Only DONE does that: deleting or archiving card 7 leaves
your block standing and reports it as `gone` (tb does not decide on its own that "we are
waiting on this" is void), and reopening a finished card does not block anything again.
Blocking a card on one that is already done is refused — nothing would ever lift it.

`tb config wip-counts-blocked no` frees the work slot of a blocked card, so waiting on the
other side does not stall the board. At most as many blocked cards as the WIP limit are
discounted, so DOING can never exceed twice the limit however much is blocked.

`tb config waiting-lane shown` gives blocked cards their own **WAITING** section in
`tb board`, with what each is waiting on; each column keeps its real count and says how many
of its cards are there (`TODO (3) · 1 waiting`), so no card is drawn twice. It is display
only: JSON, the columns, the counts, `tb next` and every command are unaffected.

A board whose cards carry **no due dates**, and which sets none of these settings, looks and
behaves exactly as it did before. Giving a card a due date is the opt-in: from then on it shows
its mark when it is close, with `due-warn` (3 days) deciding how close that is.

## Layouts and themes

Give Terminal Board any pane you like: half the screen, a third, or a small corner. It picks
the best layout for that shape and switches as soon as you resize.

**Half the screen, wide.** All four columns side by side, with GitHub and your agents below.

![Half the screen, wide](docs/images/view-half-horizontal.png)

**Half the screen, tall.** The columns as a 2 × 2 grid, then GitHub and agents.

<img src="docs/images/view-half-vertical.png" alt="Half the screen, tall" width="440">

**A third of the screen, wide.** A strip along the bottom: the columns on the left, GitHub
and your agents stacked on the right.

![A third of the screen, wide](docs/images/view-third-horizontal.png)

<table>
<tr>
<td valign="top" width="50%">
<b>A third of the screen, tall.</b> A side pane: everything in one column, cards first.<br><br>
<img src="docs/images/view-third-vertical.png" alt="A third of the screen, tall" width="100%">
</td>
<td valign="top">
<b>Focus.</b> A small corner: just the card you're working on, big, with its checklist.<br><br>
<img src="docs/images/view-focus.png" alt="Focus view" width="100%">
</td>
</tr>
</table>

If something doesn't fit, it shrinks to a one-line bar instead of disappearing. Press
`Tab` to open it full screen and `Esc` to come back.

**Handy keys**

- `L` pins a layout you like (press again to cycle, back to automatic).
- `T` switches between the dark and light theme.
- The arrow keys always move to whatever is next to you on screen.

<details>
<summary>Exact sizes (for the curious)</summary>

| layout | picked when |
|---|---|
| focus | fewer than 40 columns or 16 rows |
| third, wide | at least 80 columns and fewer than 30 rows |
| third, tall | at most 62 columns and 30+ rows |
| half, wide | wide and 30+ rows |
| half, tall | 63+ columns, 30+ rows and taller than wide |

The same settings from the command line: `tb config layout auto|focus|third-h|third-v|half-h|half-v`
and `tb config theme dark|light`.
</details>

## Where your data lives

- Boards: `~/.local/state/terminal-board/boards/<name>.db` (one SQLite file per board).
- **Back up** by copying that folder (ideally while `tb` is closed).
- Your own settings on this machine (the saved default board): `~/.config/terminal-board/config.json`,
  readable only by you. `TB_CONFIG=/path/to/file.json` uses another file.
- `TB_DB=/path/to/file.db` makes `tb` use a specific file. In that mode board names are
  not available (every name would alias the same file): an explicit non-default name fails
  with `TB_DB is set — board names are ignored; unset TB_DB to use boards`. A `TB_BOARD`
  left in the environment is not a typed name: `TB_DB` wins, and tb says so in one warning
  line (`TB_DB is set, so TB_BOARD=work is ignored …`; with `--json`, a `warnings` field).
- **Board files are private.** Every file tb creates — a board, its `-wal`/`-shm` sidecars,
  a backup — is mode `0600`, whatever your umask, in the boards folder and under `TB_DB`
  alike. A board path may be a symbolic link (`boards/work.db -> /mnt/secure/work.db`): the board
  is the file the link leads to, and tb creates *that* file `0600`; a link into a folder that
  does not exist, or a loop of links, is refused, and tb never changes the mode of anything
  through a link. A board made by an earlier version is `0644`; tb never changes the mode of an
  existing file on its own (you may share a board with a group on purpose). It tells you —
  one warning line naming the file — until you choose:

  ```sh
  tb config file-mode            # private (0600), or what is wrong with it
  tb config file-mode private    # the board file and its sidecars become 0600; logged on the board
  tb config file-mode shared     # it is shared on purpose: tb leaves the mode alone and stops saying so
  ```
- **A backup is written before a board's schema is upgraded.** When a newer tb opens a board
  an older one wrote and has to add columns, it first copies the board next to itself as
  `<file>.before-<version>.<UTC date-time>.bak` and says where. The copy is one complete
  file (it includes cards still in the `-wal`, and needs no sidecars), and there is exactly
  one however many `tb` processes open the board at that moment. If the copy cannot be
  written, nothing is upgraded and the command fails. Going back to an older version:
  [UPGRADING.md](UPGRADING.md#going-back-to-an-older-tb).

## Testing

Tests and scripted replays can pin the clock: `TB_NOW=<unix seconds>` (legacy name
`TTYBOARD_NOW`) makes every card timestamp and event use that second instead of the real
one, so fixtures are stable whatever the time of day. Because the value is written into a
real board, it is validated: unset or empty means the real clock, and anything else must be
an integer between `946684800` (2000-01-01) and `4102444800` (one past the last accepted,
`4102444799`) — anything else
exits non-zero, names the variable and the range, and writes nothing.

## Troubleshooting

- **`tb: command not found`.** `~/.local/bin` isn't on your `PATH`. Add
  `export PATH="$HOME/.local/bin:$PATH"` to `~/.zshrc` or `~/.bashrc` (fish:
  `fish_add_path ~/.local/bin`) and open a new terminal.
- **GitHub panel says it can't fetch.** Run `gh auth status`; if you are not logged in, run
  `gh auth login`. Check the repository name with `tb config github`.
- **Shift+arrows don't move cards.** Some terminals and multiplexers (tmux, herdr) swallow
  them. Use `<` `>` to move between columns and `K` `J` to move up and down.
- **Colours look wrong.** Try the other theme with `T`. Terminal Board paints its own
  background, so it looks the same in any terminal theme.
- **The GITHUB or AGENTS panel is gone.** It was hidden: press `G` or `A`, or
  `tb config github-panel shown`.

## Uninstall

<!-- no-test -->
```sh
curl -fsSL https://raw.githubusercontent.com/kachowtowmater/terminal-board/main/install.sh | bash -s -- --uninstall
```

(or `./install.sh --uninstall` from a clone). This removes `tb`, the Claude Code skill and
the agent-instruction blocks it added. It asks separately before deleting your boards (the
default is to keep them).

## For app developers (JSON)

`tb board --json`, `tb watch --json` (a live stream, one board per line) and `--json` on
every command give stable JSON. The shapes are documented in [docs/JSON.md](docs/JSON.md).

<!-- no-test -->
```sh
tb watch --json
```

## FAQ

**Can two people (or agents) use the same board at once?** Yes. The board is a single file
that handles concurrent writers, and `tb next` never hands the same card to two people.

**Does it need the internet?** No. Only the optional GitHub panel talks to GitHub, through
the `gh` tool.

**Can I use it without GitHub or agents?** Yes. Skip both in `tb setup`; you get a plain
four-column board.

**Where are the settings?** Per board, in the board file. See them with `tb config`.

**Is my GitHub token stored?** No. Terminal Board only runs the `gh` command; `gh` keeps
your login.

## Docs

- [docs/HUMANS.md](docs/HUMANS.md) — using the board day to day (people).
- [docs/AGENTS.md](docs/AGENTS.md) — the agent manual, also `tb guide` (AI agents).
- [docs/JSON.md](docs/JSON.md) — the JSON contract for scripts and apps.
- [docs/SCHEMA.md](docs/SCHEMA.md) — the SQLite file as a read-only interface.
- [UPGRADING.md](UPGRADING.md) — coming from 1.x: what is refused now, and the way through.
- [CHANGELOG.md](CHANGELOG.md) — what changed in each release.

## License

MIT — see [LICENSE](LICENSE).
