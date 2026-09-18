# Changelog

## Unreleased

### Identity
- Inside a herdr pane, tb asks herdr for the agent name of `HERDR_PANE_ID` when there is no
  `--as`, `TB_AS` or `HERDR_AGENT_NAME`. Agents that forgot `--as` after `tb next` were
  logged under the login name; now they are logged under their own. The login name is used
  only outside herdr, or when herdr has no named agent for the pane.

### JSON
- Checklist items have the same shape in `tb show --json` and `tb board --json`: `{n, idx, text, done}`. `n` is the canonical item number; `idx` (what `show` used before) stays as a deprecated alias with the same value, so existing readers keep working.

### Added
- **Reviewers claim cards: `tb next --review --as NAME`.** Atomically claims the top unblocked
  REVIEW card that NAME did not author and nobody else has claimed (same lock as `tb next`,
  so two reviewers never get the same card; no WIP limit). The card shows `review NAME` next
  to its owner; JSON cards gain a nullable `reviewer` field, and the database a nullable
  `reviewer` column (added on open). The reviewer stays on a card that reaches DONE; any other
  move clears it.

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
