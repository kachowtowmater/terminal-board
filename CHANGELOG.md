# Changelog

## Unreleased

### GitHub
- A one-off `gh` failure no longer shakes the board: no extra row, the last good snapshot
  stays, and the panel header quietly reads `synced HH:MM · offline, retrying` (network/
  timeout errors) or `synced HH:MM · gh error` (everything else). The header turns red —
  the same style as other problems — only after 3 consecutive failed refreshes. The full
  error text stays in `tb github` and `--json` (`error`, `fails` = consecutive failures,
  snapshot `fetched_at` so readers can tell how stale the data is).

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
