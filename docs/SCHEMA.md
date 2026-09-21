# Terminal Board schema (the SQLite contract)

One board = one SQLite file — `~/.local/state/terminal-board/boards/<board>.db`, or the
path in `TB_DB`, which overrides everything — in WAL mode.
**This file documents the schema as a supported read-only interface:** dashboards, apps and
orchestrators may open the file read-only and query it. **Writes go through `tb`** — the
tool owns the events log, positions and migrations, and a foreign writer skips the invariants
(WIP limits, event log, `position` ordering).

## Conventions

- All timestamps are **unix seconds** (`INTEGER`).
- Card order inside a column is `position` (0 = top), ascending; new cards go to the bottom.
- Consumers must ignore unknown columns, tables and event kinds — tb adds columns (via
  migration) and event kinds without notice. See docs/JSON.md for the same rule on JSON.
- Schema changes are additive: new columns via `ALTER TABLE … ADD COLUMN`, never renames.
  A column documented here keeps its name and meaning.

## Tables

### cards
| column | type | meaning |
|---|---|---|
| `id` | INTEGER PK | stable card id |
| `title` | TEXT | without the `tag:` prefix and the `gh#N` token |
| `tag` | TEXT NULL | parsed from `tag: title` |
| `description` | TEXT | the brief |
| `column` | TEXT | `todo` / `doing` / `review` / `done` (quoted keyword — `"column"`) |
| `owner` | TEXT NULL | who holds it |
| `due` | TEXT NULL | due date: a local calendar date as text, `YYYY-MM-DD`, exactly as given to `--due` — never a timestamp, so it sorts as text and no time zone applies. tb writes nothing else here; text from another writer is kept as is |
| `gh_ref` | INTEGER NULL | linked GitHub issue/PR number |
| `created_at` | INTEGER | unix seconds |
| `column_since` | INTEGER | unix seconds since the last column move |
| `blocked` | TEXT NULL | what blocks it (e.g. `#7`) |
| `position` | INTEGER | order within the column, 0 = top |
| `reviewer` | TEXT NULL | who claimed it with `tb next --review`; kept in `done`, cleared by any other move |

### checklist
| column | type | meaning |
|---|---|---|
| `card_id` | INTEGER FK → cards.id | on delete cascade |
| `idx` | INTEGER | 1-based item number (with `card_id` forms the PK) |
| `text` | TEXT | the item |
| `done` | INTEGER | 0 / 1 |

### events
Every card change, oldest first per card (`ORDER BY ts, id`).

| column | type | meaning |
|---|---|---|
| `id` | INTEGER PK | monotonically increasing (the resume cursor for `tb watch --events`) |
| `card_id` | INTEGER FK → cards.id | on delete cascade |
| `ts` | INTEGER | unix seconds |
| `actor` | TEXT | who did it (agent name, `github`, a human) |
| `kind` | TEXT | see the vocabulary below |
| `text` | TEXT | detail (empty when the kind carries none) |

Event `kind` vocabulary — **open set; new kinds may appear; ignore what you don't know**:

| kind | text carries |
|---|---|
| `created` | — |
| `taken` | — (the actor takes the card) |
| `moved` | `<from> -> <to>` (e.g. `doing -> review`) |
| `note` | the note text |
| `check` | the checklist item ticked/unticked |
| `blocked` | `by <what>` |
| `unblocked` | — |
| `dropped` | — (owner cleared) |
| `edit` | what changed |
| `due` | `<old> -> <new>` due date (`none` = no date), e.g. `none -> 2026-10-09` |
| `prio` | the move within the column |
| `github` | the automation reason (e.g. `PR gh#30 open → review`) |
| `reviewing` | — (the actor claimed it with `tb next --review`) |
| `unclaimed` | the reviewer whose claim was released |
| `returned` | why a REVIEW card was sent back to its owner |
| `approved` | — (a reviewer's `tb done ID --approve`; the card does not move) |
| `force` | what `--force` got past (moving a held card, approving your own work) |

### board_events
Board-level events (no card):

| column | type | meaning |
|---|---|---|
| `id` | INTEGER PK | monotonically increasing |
| `ts` | INTEGER | unix seconds |
| `actor` | TEXT | who did it |
| `kind` | TEXT | `delete`, … (same open-set rule as `events`) |
| `text` | TEXT | detail |

### github_snapshot
The last GitHub sync.

| column | type | meaning |
|---|---|---|
| `key` | INTEGER PK | always 1 (single row) |
| `fetched_at` | INTEGER | unix seconds of the last good snapshot (0 = never) |
| `json` | TEXT NULL | the cached snapshot, same shape as `tb github --json` |
| `error` | TEXT NULL | the last fetch error |
| `fails` | INTEGER | failed refreshes in a row since the last good snapshot (a good fetch resets it to 0); the board turns the GitHub header red at 3 |

### config
Key/value settings.

| column | type | meaning |
|---|---|---|
| `key` | TEXT PK | setting name (`wip`, `theme`, `layout`, `github`, `github-panel`, `agents-panel`, `tz`, `due-warn`) — a row exists only once the setting is set |
| `value` | TEXT | the setting's value |

## Reading safely

```sh
DB=~/.local/state/terminal-board/boards/default.db   # or "$TB_DB"
sqlite3 "$DB" "SELECT id, title FROM cards WHERE \"column\"='doing' ORDER BY position"
sqlite3 "$DB" "SELECT ts, actor, kind, text FROM events WHERE card_id=3 ORDER BY ts, id"
```

Open the file read-only (`sqlite3 "file:…?mode=ro"`) to be certain you cannot corrupt it.
