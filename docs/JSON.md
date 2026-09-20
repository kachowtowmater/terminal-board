# Terminal Board JSON contract (schema v1)

Apps and agents drive Terminal Board through `tb … --json`. Every object below carries
fixed field names, pinned by golden tests (`tests/contract.rs`). A breaking change bumps
`"v"`; new fields may be added without a bump. All timestamps are **unix seconds**.
Board selection works as usual: `tb [BOARD] …`, `-b NAME`, `TB_BOARD`, or `TB_DB=/path/file.db`.
With `TB_DB` set there is a single file — an explicit non-default board name is refused
(`TB_DB is set — board names are ignored; unset TB_DB to use boards`), so `board` never
reports a name that was not opened.

**Forward compatibility (a rule, pinned by a test):** consumers must **ignore unknown
fields and unknown event kinds** — tb adds fields and event kinds without bumping `"v"`,
and a consumer that hard-fails on them breaks on every minor update. The event `kind`
vocabulary is open (see docs/SCHEMA.md); treat an unknown kind as "something happened to
this card", not as an error.

## `tb board --json` — the whole board

```json
{
  "v": 1,
  "board": "default",
  "wip": 3,
  "theme": "dark",
  "layout": "auto",
  "github": { "repo": "acme/widgets", "snapshot": { "…": "see below" }, "error": null, "fails": 0, "fetched_at": 1789763036 },
  "columns": {
    "todo":   [ card, … ],
    "doing":  [ card, … ],
    "review": [ card, … ],
    "done":   [ card, … ]
  }
}
```

| field | type | notes |
|---|---|---|
| `v` | int | schema version (1) |
| `board` | string | board name |
| `wip` | int | WIP limit for DOING |
| `theme` | `"dark"`\|`"light"` | |
| `layout` | `"auto"`\|`"full"`\|`"sidebar"`\|`"strip"` | TUI layout preference |
| `github.repo` | string\|null | `owner/repo`, null when GitHub is off |
| `github.snapshot` | object\|null | the cached GitHub snapshot (same as `tb github --json` without the per-issue `state`/`who`) |
| `github.error` | string\|null | the last fetch error, shown next to the last good snapshot |
| `github.fails` | int | consecutive failed refreshes; the board UI goes red only after 3 |
| `github.fetched_at` | int | unix seconds of the last good snapshot (0 = never fetched) |
| `columns.*` | card[] | todo/doing/review in `position` order; **done = every done card, newest first** (the TUI only shows the last 24h — filter on `column_since`) |

A bare `tb --json` (not a terminal) prints the same object.

### card

```json
{
  "id": 1,
  "title": "login form rejects",
  "tag": "widgets",
  "description": "",
  "column": "doing",
  "position": 0,
  "owner": "bot-2",
  "due": null,
  "gh_ref": 327,
  "blocked": null,
  "created_at": 1789763036,
  "column_since": 1789763036,
  "checklist": [ { "n": 1, "idx": 1, "text": "repro", "done": false } ],
  "round": 1,
  "events": [
    { "ts": 1789763036, "actor": "bot-2", "kind": "created", "text": "" },
    { "ts": 1789763036, "actor": "bot-2", "kind": "taken", "text": "" }
  ]
}
```

| field | type | notes |
|---|---|---|
| `id` | int | stable card id |
| `title` | string | without the `tag:` prefix and without a **leading** `gh#N` token; a `gh#N` later in the title stays in the text |
| `tag` | string\|null | parsed from `tag: title` |
| `description` | string | |
| `column` | `todo`\|`doing`\|`review`\|`done` | |
| `position` | int | order within the column, 0 = top |
| `owner` | string\|null | who holds it |
| `due` | string\|null | free text |
| `gh_ref` | int\|null | GitHub issue/PR number. A **leading** `gh#N` (first word after the optional `tag:`) is moved out of the stored title; a `gh#N` **later in the title stays in the text** and still sets the link (the first such ref wins). |
| `blocked` | string\|null | what blocks it (e.g. `#7`) |
| `created_at`, `column_since` | int | unix seconds |
| `checklist[]` | `{n, idx, text, done}` | `n` is 1-based and canonical; `idx` is a deprecated alias with the same value (kept so older readers of `tb show --json` don't break; removed no earlier than the next major version) |
| `round` | int | rework round: 1, plus one for every `returned` event (counted from all events, so it never drifts) |
| `events[]` | `{ts, actor, kind, text}` | the last 10, oldest first. Kinds include `created`, `taken`, `moved`, `returned` (a reviewer sent it back; `text` is the reason, right after its `moved` `review -> doing`), `note`, `check`, `blocked`, `unblocked`, `dropped`, `edit`, `prio`, `github`, `force`, `approved` (a review pass recorded with `tb done ID --approve`); the set is open — see the forward-compatibility rule above |

## `tb watch --json` — live stream (NDJSON)

One full board object (as above) per line: the first line immediately, then a new line on
every change by anyone (polls SQLite `PRAGMA data_version` every ~300 ms, so writes from
other processes and GitHub cache refreshes both count). Exits cleanly when stdout closes.

```sh
tb watch --json | while read -r line; do …; done
```

## `tb watch --events --json [--since TS]` — one line per event (opt-in)

Plain `tb watch --json` above is unchanged byte-for-byte; the event stream is opt-in:

```sh
tb watch --events --json                # one NDJSON line per event
tb watch --events --json --since 1789777000   # resume: only events at/after that unix second
```

Each line is `{v, ts, card_id, actor, kind, from, to, text}`: `kind` is the event kind
(`created`, `taken`, `moved`, `note`, `check`, …); `from`/`to` are the column transition of
every event that changes a card's column — `created` (null → `todo`), `taken` (`todo` →
`doing`), `dropped` (e.g. `doing` → `todo`) and `moved` (e.g. `doing` → `review`) — so following
them tracks every card's column; they are null for every other kind; `text` is the event's
text (the note, the block reason, …). `--since` resumes after a restart: only events at/after
that unix second are streamed, in `(ts, id)` order — an orchestrator records the last event
it saw and passes the next start second on restart.

## Writes — `--json` results

Every write command takes `--json`: `add`, `next`, `take`, `note`, `check`, `move`, `done`,
`block`, `drop`, `rm`, `prio`, `edit`.

Success (exit 0) — the card after the change (for `rm`, the card as it was):

```json
{ "ok": true, "card": { …card… } }
```

`config KEY VALUE --json` returns `{ "ok": true, "config": { "key": "wip", "value": 4 } }`.
`sync --json` returns `{ "ok": true, "moves": [ { "card_id": 3, "gh_ref": 20, "from": "doing", "to": "done", "text": "PR gh#20 merged → done" } ] }`.

Failure (non-zero exit), for any command run with `--json` — including **argument errors**
(bad value, missing argument, unknown flag): the parser's plain text never replaces the JSON
object; parse failures answer on stdout with the same shape and exit **2** (usage) instead
of 1 (runtime):

```json
{ "ok": false, "error": "no card #9", "hint": "see 'tb list' for ids" }
```

An argument error names what is missing and gives the usage line, e.g. `tb note 1 --json` →
`"error": "argument error: the following required arguments were not provided: <TEXT>"`,
`"hint": "usage: tb note <ID> <TEXT> — see 'tb --help' …"` (exit 2).

`hint` always says what to run next, e.g. `doing is full (3/3)` → `finish one with 'tb done ID' first`,
or `issue #11 still open on GitHub` → `… 'tb done 11 --force' to mark it done anyway`.
When the board was chosen **explicitly by name or `-b`** and is not `default`, the command in a
hint carries it — `see 'tb work list' for ids` — so copying the hint into a fresh shell acts on
the same board. A board picked by `TB_BOARD` travels in the environment, so its hints stay bare.

## `tb agents --json`

herdr agent panes merged with the board (empty array when herdr is not available).

```json
[ { "name": "bot-2", "harness": "aider", "status": "working", "pane_id": "w:p5", "job": "fix #327", "card_id": 1, "last_note": "tests pass, opening PR", "last_event_at": 1789763036 } ]
```

| field | type | notes |
|---|---|---|
| `name` | string | herdr agent name, else the pane label |
| `harness` | string | `claude`, `aider`, … |
| `status` | string | `working`, `idle`, `done`, `blocked`, `unknown` |
| `pane_id` | string | |
| `job` | string\|null | the last `·` segment of the pane label |
| `card_id` | int\|null | the card it holds (DOING first) |
| `last_note` | string\|null | that card's last note text (what the agent says it is doing) |
| `last_event_at` | int\|null | unix seconds of the card's last event — compute the age yourself; an agent that never notes shows an old age |

## Other read commands

- `tb list --json` — array of cards (without checklist/events).
- `tb show ID --json` — one card with `checklist` (`n`, `idx`, `text`, `done` — the same shape as in `tb board --json`), `round` and all `events`.
- `tb boards --json` — `[{name, default, todo, doing, review, done}]`.
- `tb github --json` — the GitHub snapshot: `{repo, fetched_at, issues_open, prs[], issues[] (+state, who), merged_today[], main_ci}` plus the sync state: `error` (the full text of the last fetch error, null after a good fetch) and `fails` (consecutive failed refreshes — the board header says `synced HH:MM · offline, retrying` or `· gh error`, in red only after 3 in a row, and never adds a row to the panel).
- `tb github repos --json` — `[{name_with_owner, description, pushed_at, is_private, own}]`.
