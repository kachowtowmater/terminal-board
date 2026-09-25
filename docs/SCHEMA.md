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
| `blocked_on` | TEXT NULL | `--on`: who or what it waits for — `#<id>` of another card, or a name. A card blocked on `#<id>` is unblocked by tb when that card reaches DONE |
| `blocked_until` | TEXT NULL | `--until`: a local calendar date (`YYYY-MM-DD`) to look again. "Recheck" is **derived** from it at read time (the board's `tz`); nothing is stored when the date arrives |
| `position` | INTEGER | order within the column, 0 = top |
| `reviewer` | TEXT NULL | who claimed it with `tb next --review`; kept in `done`, cleared by any other move |

### checklist
| column | type | meaning |
|---|---|---|
| `card_id` | INTEGER FK → cards.id | on delete cascade |
| `idx` | INTEGER | 1-based item number (with `card_id` forms the PK) |
| `text` | TEXT | the item |
| `done` | INTEGER | 0 / 1 |

### links
Evidence attached with `tb link ID VALUE --label LABEL` (`tb link ID --rm N` removes one, the
rest renumber — same shape as `checklist`). `value` is a path, a git sha or a URL, as free
text: **tb only stores it — it never reads a file there, never resolves a sha against a repo
and never fetches a URL.** `label` is free text too (not a fixed set), lower-cased, so
`config done-needs-link LABEL` (below) is a plain string match, case-insensitive, against
whatever a caller typed with `--label`.

| column | type | meaning |
|---|---|---|
| `card_id` | INTEGER FK → cards.id | on delete cascade |
| `idx` | INTEGER | 1-based item number (with `card_id` forms the PK) |
| `label` | TEXT | what kind of evidence it is, e.g. `brief`, `verdict`, `commit` — free text, lower-cased |
| `value` | TEXT | the path, sha or URL, kept exactly as given (trimmed) — never parsed or validated as one of the three |
| `added_by` | TEXT | who ran `tb link` |
| `added_at` | INTEGER | unix seconds |

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
| `actor_id` | INTEGER NULL FK → actors.id | the identity behind `actor` (harness, model, role, session, machine); NULL when nothing but the name is known, and on every event written before identities were recorded — nothing is back-filled |
| `ancestry` | TEXT NULL | the kernel's parent chain the writing command ran under (`store::proc`), a JSON list of process names oldest ancestor first. Written on moves into DONE, `force` events and the verifier-config changes only; NULL on everything else and before this column existed — nothing is back-filled |
| `assignee` | TEXT NULL | `kind='assigned'` only: who the card was assigned to, structured (#111) — the self-approval guard reads this directly, never `text`, so a reworded message cannot change who it refuses. NULL for every other kind, and on an `assigned` row written before this column existed (the guard falls back to parsing `text` for those only). Not exposed in JSON — `text` already carries the same name for reading |

Event `kind` vocabulary — **open set; new kinds may appear; ignore what you don't know**:

| kind | text carries |
|---|---|
| `created` | — |
| `taken` | — (the actor takes the card) |
| `assigned` | `assigned to <name>` — `actor` is who ran `tb assign` (the assigner), `cards.owner` is `<name>` (who now holds it) |
| `moved` | `<from> -> <to>` (e.g. `doing -> review`) |
| `note` | the note text |
| `check` | the checklist item ticked/unticked |
| `link` | `+ LABEL: VALUE` (added) or `- LABEL: VALUE` (removed) — `tb link` |
| `blocked` | `by <what>`, plus ` · on <who>` and ` · until <date>` when `--on` / `--until` were given |
| `unblocked` | — · `cleared on done` · `#<id> is done` (the card it waited on finished) |
| `dropped` | — (owner cleared) |
| `edit` | what changed |
| `due` | `<old> -> <new>` due date (`none` = no date), e.g. `none -> 2026-10-09` |
| `prio` | the move within the column |
| `github` | the automation reason (e.g. `PR gh#30 open → review`) |
| `reviewing` | — (the actor claimed it with `tb next --review`) |
| `unclaimed` | the reviewer whose claim was released |
| `returned` | why a REVIEW card was sent back to its owner |
| `approved` | — (a reviewer's `tb done ID --approve`; the card does not move) |
| `force` | what `--force` got past (moving, dropping, editing, blocking, deleting or archiving a held card; ticking, adding, removing a checklist item on one; reordering one; approving your own work) |
| `archived` | — (`tb rm` on a board set to `rm archive`; the card leaves `cards` with this as its last event) |
| `restored` | — (`tb restore ID` brought the card back) |

### board_events
Board-level events (no card). Read with `sqlite3` as below, or — interleaved with `events`,
oldest first, `card_id` null — with `tb log` (docs/JSON.md):

| column | type | meaning |
|---|---|---|
| `id` | INTEGER PK | monotonically increasing |
| `ts` | INTEGER | unix seconds |
| `actor` | TEXT | who did it |
| `kind` | TEXT | `delete`, `wip`, `file-mode`, `archive`, `restore`, `rm` (the setting changed), `rules` (the rules text was set or cleared), `rules-seen` (an agent's first `tb next` since — `text` is the rules text shown), `force` (a held card was deleted or archived), … (same open-set rule as `events`) |
| `text` | TEXT | detail |
| `actor_id` | INTEGER NULL FK → actors.id | as `events.actor_id` |
| `ancestry` | TEXT NULL | as `events.ancestry` — written on the verifier-config changes (`verifiers`, `verifier-only`) only |

### actors
Who a name was: one row per **distinct identity**, shared by every event that identity wrote.
`events.actor` stays the short display name; this is the record behind it, so work can be
traced back to the session that did it. The key is the whole tuple `(actor, harness, model,
role, session, host)`, NULLs included (unique index `actors_identity`): a session writes one
row however many commands it runs. A writer about which nothing but a name is known — a person
in a plain terminal — gets **no row** and a NULL `actor_id`. Every value is self-reported (a
claim, not proof), cleaned of control characters, and at most 64 characters.

| column | type | meaning |
|---|---|---|
| `id` | INTEGER PK | what `actor_id` points at; never reused |
| `actor` | TEXT | the display name, exactly as in `events.actor` |
| `harness` | TEXT NULL | the agent harness, without its version (`claude-code`, …): `$TB_HARNESS`, else what the harness exports (`$AI_AGENT`, `$CLAUDECODE`, `$OMPCODE` — checked before `$CLAUDECODE`, since omp sets that too as a compatibility flag), else herdr's record of the pane |
| `model` | TEXT NULL | `$TB_MODEL`, else — only when `harness` resolved to `pi` — `$PI_MODEL`; every other harness gives tb nothing here, and tb never guesses it |
| `role` | TEXT NULL | `$TB_ROLE` (orchestrator, coder, reviewer, …) — explicit only; no harness exports it |
| `session` | TEXT NULL | the harness's session id: `$TB_SESSION`, else what the harness exports (`$PI_SESSION_ID` when `harness` resolved to `pi`, else `$CLAUDE_CODE_SESSION_ID`; a pi started from a Claude Code shell inherits Claude's id, and pi never takes it), else herdr's record of the pane. **Never a path:** a session reported as the path of a file is stored as the identifier inside the file's name, or else as `path-` + 12 hex digits (a hash of the path) |
| `host` | TEXT NULL | the machine (`$TB_HOST`, else the first label of its host name) |
| `first_seen` | INTEGER | unix seconds of the first event this identity wrote |
| `last_seen` | INTEGER | unix seconds of its latest event |

### board_creator
Who made the board (#137): **at most one row** (`id` is always 1), written once, when tb creates
the board file — by `tb new NAME` or by the first command that makes the board on first use —
and never changed after (`INSERT OR IGNORE`: of two processes creating a board at once, the
first keeps it). Same identity as an `actors` row, from the same sources, but written even when
nothing but a name is known (a person gets `actor` and `created_at`, the rest NULL). A board made
before this table existed has an empty table; `tb boards --json` then reads its creator from
`board-creations.log` (`source: "log"`) and writes nothing back. Travels with the file, so an
archived board keeps it.

| column | type | meaning |
|---|---|---|
| `id` | INTEGER PK | always 1 (`CHECK (id = 1)`) |
| `actor` | TEXT | the name the creating command ran as (`--as`, `TB_AS`, the herdr pane, the login) |
| `harness` | TEXT NULL | as `actors.harness`, for the creating command |
| `model` | TEXT NULL | as `actors.model` |
| `role` | TEXT NULL | as `actors.role` |
| `session` | TEXT NULL | as `actors.session` |
| `host` | TEXT NULL | as `actors.host` |
| `created_at` | INTEGER | unix seconds the board was created |

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
| `key` | TEXT PK | setting name (`wip`, `theme`, `layout`, `github`, `github-panel`, `agents-panel`, `tz`, `due-warn`, `sort`, `rm`, `file-mode`, `card-line`, `label.todo` / `label.doing` / `label.review` / `label.done`, `wip-counts-blocked`, `waiting-lane`, `done-by`, `done-needs-link`, `done-needs-note`, `max-rounds`, `kind`, `hook`, `hook-after`) — a row exists only once the setting is set. `hook` / `hook-after` hold a NAME only (`[a-z0-9_-]{1,32}`), never a command — see README, "Hooks: this machine's own gate on a move" |
| `value` | TEXT | the setting's value |

### archived_cards
Exists only on a board that was ever set to `tb config rm archive` (tb creates it then, not
before). One row per card `tb rm` archived; `tb restore ID` moves the card back and deletes
the row. An archived card is in NO other table, so nothing that reads `cards` can count it.

| column | type | meaning |
|---|---|---|
| `card_id` | INTEGER PK | the card's id — restored under the same id (ids are never reused) |
| `archived_at` | INTEGER | unix seconds |
| `archived_by` | TEXT | who ran `tb rm` |
| `title` | TEXT | the card's title, for listing |
| `tag` | TEXT NULL | its tag |
| `column` | TEXT | the column it was in, and returns to |
| `owner` | TEXT NULL | who held it |
| `card` | TEXT | its `cards` row as a JSON object, column name → value |
| `checklist` | TEXT | its `checklist` rows, a JSON array of such objects |
| `events` | TEXT | its `events` rows (ids included), a JSON array — the whole history |
| `links` | TEXT | its `links` rows, a JSON array of such objects (`'[]'` on a table made before links existed) |

## Reading safely

```sh
DB=~/.local/state/terminal-board/boards/default.db   # or "$TB_DB"
sqlite3 "$DB" "SELECT id, title FROM cards WHERE \"column\"='doing' ORDER BY position"
sqlite3 "$DB" "SELECT ts, actor, kind, text FROM events WHERE card_id=3 ORDER BY ts, id"
# everything one session wrote (a session id from `tb show ID`)
sqlite3 "$DB" "SELECT e.card_id, e.ts, e.kind, e.text FROM events e JOIN actors a ON a.id = e.actor_id WHERE a.session = 'SESSION-ID' ORDER BY e.id"
```

Open the file read-only (`sqlite3 "file:…?mode=ro"`) to be certain you cannot corrupt it.

The file is mode `0600` (tb creates it private; `tb config file-mode` reports and changes
that), so a reader runs as the user who owns the board. When a newer tb upgrades the schema
it first writes `<file>.before-<version>.<UTC date-time>.bak` next to the board: a complete
copy in the OLD schema, which is what an older tb — or a reader pinned to it — can open.
