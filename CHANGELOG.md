# Changelog

## Unreleased

### A reader that stops early no longer crashes tb
- `tb list | head -1`, `tb config | grep -q …` and the like: when the reader closes the pipe
  before tb has written everything, tb now stops and exits 0 (as `tb watch` already did)
  instead of panicking with `failed printing to stdout: Broken pipe` (exit 101), on every
  command. The installer test reads `tb config` output from a variable, not through a pipe.

### Identity: a blank `--as` is refused, never silently replaced
- `--as ""` or `--as "  "` (usually `--as "$NAME"` with `NAME` unset in a fresh shell) fails
  before any write with `--as is empty — pass your agent name, e.g. --as bot-1` (text and
  `--json`). An absent flag keeps the fallback chain (`TB_AS`, the herdr pane's agent, the
  login name) unchanged.
### The WIP-full message is actor-aware
- `doing is full` is a board-wide limit, but the old hint told every actor to `tb done ID` —
  including an agent holding nothing, whose only obedience path was finishing someone else's
  card. Now: `doing is full (3/3: #3 bot-1, #1 bot-2, #2 bot-3)` plus what the actor can do —
  `finish #3 with 'tb done 3' first` when they hold one, `you hold none; wait, or ask one of
  them to finish` when they don't. Both `next` and `take`, text and `--json`.
## 1.1.0 — 2026-09-18
### Display
- Control characters and terminal sequences in displayed text (card titles, descriptions,
  notes, names, checklist items, GitHub titles and branches, agent labels, echoed errors) are
  removed before they reach the terminal, in the board and in plain CLI output. Tabs and line
  breaks in one-line fields show as spaces, so every card stays on its own line. The store and
  `--json` output keep the text as it was written.
### third-v: spare rows are used, not left blank
- At tall panes (e.g. 52×66) the one-third view left ~5 blank rows between the DONE section
  and the GITHUB panel. Now: spare height first grows the GitHub rows (then AGENTS) up to
  their natural size, and anything still left stretches the last card section instead of
  sitting as a blank band. At 52×56 the render is unchanged.
### JSON: argument errors follow the JSON contract
- With `--json` anywhere in argv, argument-parse failures (bad value, missing argument,
  unknown flag) answer `{"ok":false,"error":…,"hint":…}` on **stdout** with exit 2, instead
  of plain text on stderr and an empty stdout. `error` names what is wrong (including the
  missing argument, e.g. `<TEXT>`) and `hint` carries the usage line (`usage: tb note <ID>
  <TEXT> — …`). Without `--json` nothing changes (the parser's message, exit 2);
  `--help`/`--version` are unchanged; runtime failures keep exit 1.
### A mid-title `gh#N` keeps its words
- Only a **leading** `gh#N` (first word after the optional `tag:`) is moved out of the stored
  title. A `gh#N` later in the sentence stays in the text verbatim — the board and JSON keep
  the original wording — and still sets the link (the first such ref wins).
### Focus view: shift+arrows move, and the help names the axis
- In the focus view (small panes) shift+left/right were swallowed by navigation and did
  nothing — a person thought the card moved when it had not. Now shift+left/right moves the
  card and shift+up/down reorders it, same as every other view (`>`/`<` still work).
- The footer in the focus view states its arrow axis (`arrows card/col · shift+<> move`) and
  the full help gains a `focus view arrows` row, so what the keys do agrees in every view.

### Identity
- Inside a herdr pane, tb asks herdr for the agent name of `HERDR_PANE_ID` when there is no
  `--as`, `TB_AS` or `HERDR_AGENT_NAME`. Agents that forgot `--as` after `tb next` were
  logged under the login name; now they are logged under their own. The login name is used
  only outside herdr, or when herdr has no named agent for the pane.

### JSON
- Checklist items have the same shape in `tb show --json` and `tb board --json`: `{n, idx, text, done}`. `n` is the canonical item number; `idx` (what `show` used before) stays as a deprecated alias with the same value, so existing readers keep working.

### Changed
- **Nobody approves their own work.** REVIEW → DONE is refused when you are the card's
  author — whoever moved it DOING → REVIEW, or its owner when GitHub sync made that move —
  on every path (`tb done`, `tb move ID done`, the `d` key). The error is
  `you did this work — ask another person or agent to review it`. `--force` still gets past
  it and is logged on the card as a `force` event. In the full-screen board, `d` or a move
  right on your own REVIEW card asks `you moved this to review yourself — approve your own
  work? y/n` instead: `y` takes the same logged path, so a person working alone is not stuck. **If one agent does both jobs in your setup**, give
  the reviewing step its own name (`tb done ID --as reviewer`) or add `--force`. Names are
  self-asserted, so this stops mistakes, not a hostile agent.

- **Sending work back, with a reason and a count.** `tb move ID doing "why"` sends a REVIEW
  card back to its **same owner** in DOING. The reason is required for that move (and only
  that move) and is logged as a `returned` event; the send-back is not blocked by the WIP
  limit, since it is the owner's existing work. In the full-screen board Shift+← / `<` on a
  REVIEW card asks for the reason. The card shows its rework round, `r2`, counted from
  events; JSON cards (`board`, `show`, write results) gain `round`. **Breaking for
  scripts:** a plain `tb move ID doing` on a REVIEW card is now refused with
  `say why it goes back`.
- **GitHub sync respects a send-back.** A returned card with an open PR stays in DOING until
  the PR is updated after the return (`updatedAt`, now part of the cached snapshot as
  `updated_at`); before, the next sync moved it straight back to REVIEW.

## 1.0.0 — 2026-09-18

The first public release of **Terminal Board** (`tb`): a task board in your terminal,
shared by people and AI agents.

### Install and setup
- One command: `curl -fsSL https://raw.githubusercontent.com/kachowtowmater/terminal-board/main/install.sh | bash`
  downloads the release binary for macOS (arm64, x86_64) or Linux (x86_64, arm64, static),
  verifies its SHA-256 checksum, installs it to `~/.local/bin` (asks before touching your
  shell's PATH) and runs `tb setup`. Falls back to building from source (after asking);
  `./install.sh` in a clone builds that checkout. `--prefix`, `--version`, `--no-setup`,
  `--no-build`, `--dry-run`, `--uninstall` (keeps your boards unless you confirm).
- `tb setup`: the setup wizard, built into the binary — board, GitHub (installs and logs in
  `gh` only after asking; numbered repo list or `owner/repo`, verified; first sync), the
  AGENTS panel, a Claude Code skill, and a marked, re-runnable block of agent instructions
  in any `AGENTS.md` / `CLAUDE.md`. Every step is `[y]es / [s]kip`; external steps default to
  skip; a summary at the end. `--yes`, `--github`, `--no-github`, `--agents`, `--no-agents`,
  `--agents-md`, `--dry-run`. Runs by itself the first time you type `tb` (`s` skips).

### Board
- Four columns — TODO, DOING (with a WIP limit), REVIEW, DONE today — in one SQLite (WAL)
  file per board; named boards (`tb home`, `tb -b work`, `tb boards`).
- Boxed cards in their column's colour; red only for problems (failing CI, blocked cards,
  idle agents holding a card). Dark and light themes (`T`) that paint their own background.
- Add, edit (title + description), delete (with confirmation), notes, checklists,
  block/unblock. Move with Shift+arrows or `<` `>`; reorder with Shift+↑/↓ or `K` `J`.
- `tb next` hands out the top TODO card atomically; the WIP limit is enforced everywhere.

### Five views, picked from the pane's shape
- **focus** (small): one card big — your DOING card, else the top TODO card — with its
  checklist and last note; ←→ cards, ↑↓ columns, `enter` ticks the next item.
- **third-h** (wide and short): columns left, GITHUB over AGENTS on the right.
- **third-v** (tall and narrow): stacked sections, each showing at least two boxed cards
  before GITHUB grows past ~8 rows; then GITHUB and AGENTS.
- **half-h** (half the screen or more): four columns, GitHub tiles and tables, AGENTS.
- **half-v** (half the width, tall): a 2×2 grid sized to its cards, GITHUB (≥ 14 rows) and
  AGENTS.
- One card style per screen (full or dense boxes, never mixed); a panel that doesn't fit
  becomes a one-line bar and `tab` shows it full screen; arrow keys move to whatever is
  next to you on screen; `L` pins a view.

### GitHub
- One repository per board (`R` picker or `tb config github owner/repo`): tiles, pull
  requests (CI, review, branch → issue), issues with their state and owner, merged today,
  main CI. The tables keep a title of at least 30 characters (dropping LABELS, BRANCH, AGE,
  WHO, REVIEW first) and switch to tidy rows on narrow panes.
- Add an issue as a card (`a`), open it in the browser (`o`).
- Cards with `gh#N` follow GitHub: an open PR moves them to REVIEW, a merged PR or closed
  issue to DONE, never backwards (on refresh and `tb sync`). A manual DONE while the issue
  is open needs confirmation (`--force` on the CLI).

### Agents
- The CLI covers every action an agent needs: `next`, `take`, `show`, `note`, `check`
  (tick/add/remove), `edit`, `block`, `move`, `done`, `drop`, `prio`, `add`, `rm`; every
  error says what to run next; `--help` fits one screen.
- `tb guide` prints the agent manual (docs/AGENTS.md): every command by task, recipes,
  rules, identity, GitHub, JSON and common errors; its walkthrough runs in the test suite.
- AGENTS panel from herdr: who works on what, idle-with-card warning; `tb agents`.

### For scripts and apps
- Stable JSON (schema v1): `--json` on every command, `tb board --json`, `tb watch --json`
  (NDJSON on every change), `{"ok":…}` results for writes. See docs/JSON.md.

### Docs
- README for first-time users, docs/HUMANS.md (daily use), docs/AGENTS.md (agents),
  docs/JSON.md. Every command in the README runs in the test suite.
