# Terminal Board JSON contract (schema v1)

Apps and agents drive Terminal Board through `tb … --json`. Every object below carries
fixed field names, pinned by golden tests (`tests/contract.rs`). A breaking change bumps
`"v"`; new fields may be added without a bump. All timestamps are **unix seconds**.
Board selection works as usual: `tb [BOARD] …`, `-b NAME`, `TB_BOARD`, or `TB_DB=/path/file.db`.

## `tb board --json` — the whole board

```json
{
  "v": 1,
  "board": "default",
  "wip": 3,
  "theme": "dark",
  "layout": "auto",
  "github": { "repo": "acme/widgets", "snapshot": { "…": "see below" }, "error": null },
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
  "checklist": [ { "n": 1, "text": "repro", "done": false } ],
  "events": [
    { "ts": 1789763036, "actor": "bot-2", "kind": "created", "text": "" },
    { "ts": 1789763036, "actor": "bot-2", "kind": "taken", "text": "" }
  ]
}
```

| field | type | notes |
|---|---|---|
| `id` | int | stable card id |
| `title` | string | without the `tag:` prefix and the `gh#N` token |
| `tag` | string\|null | parsed from `tag: title` |
| `description` | string | |
| `column` | `todo`\|`doing`\|`review`\|`done` | |
| `position` | int | order within the column, 0 = top |
| `owner` | string\|null | who holds it |
| `due` | string\|null | free text |
| `gh_ref` | int\|null | GitHub issue/PR number (`gh#N` in the title) |
| `blocked` | string\|null | what blocks it (e.g. `#7`) |
| `created_at`, `column_since` | int | unix seconds |
| `checklist[]` | `{n, text, done}` | `n` is 1-based |
| `events[]` | `{ts, actor, kind, text}` | the last 10, oldest first. Kinds include `created`, `taken`, `moved`, `note`, `check`, `blocked`, `unblocked`, `dropped`, `edit`, `prio`, `github` |

## `tb watch --json` — live stream (NDJSON)

One full board object (as above) per line: the first line immediately, then a new line on
every change by anyone (polls SQLite `PRAGMA data_version` every ~300 ms, so writes from
other processes and GitHub cache refreshes both count). Exits cleanly when stdout closes.

```sh
tb watch --json | while read -r line; do …; done
```

## Writes — `--json` results

Every write command takes `--json`: `add`, `next`, `take`, `note`, `check`, `move`, `done`,
`block`, `drop`, `rm`, `prio`, `edit`.

Success (exit 0) — the card after the change (for `rm`, the card as it was):

```json
{ "ok": true, "card": { …card… } }
```

`config KEY VALUE --json` returns `{ "ok": true, "config": { "key": "wip", "value": 4 } }`.
`sync --json` returns `{ "ok": true, "moves": [ { "card_id": 3, "gh_ref": 20, "from": "doing", "to": "done", "text": "github: PR #20 merged → done" } ] }`.

Failure (non-zero exit), for any command run with `--json`:

```json
{ "ok": false, "error": "no card #9", "hint": "see 'tb list' for ids" }
```

`hint` always says what to run next, e.g. `doing is full (3/3)` → `finish one with 'tb done ID' first`,
or `issue #11 still open on GitHub` → `… 'tb done 11 --force' to mark it done anyway`.

## `tb agents --json`

herdr agent panes merged with the board (empty array when herdr is not available).

```json
[ { "name": "bot-2", "harness": "aider", "status": "working", "pane_id": "w:p5", "job": "fix #327", "card_id": 1 } ]
```

| field | type | notes |
|---|---|---|
| `name` | string | herdr agent name, else the pane label |
| `harness` | string | `claude`, `aider`, … |
| `status` | string | `working`, `idle`, `done`, `blocked`, `unknown` |
| `pane_id` | string | |
| `job` | string\|null | the last `·` segment of the pane label |
| `card_id` | int\|null | the card it holds (DOING first) |

## Other read commands

- `tb list --json` — array of cards (without checklist/events).
- `tb show ID --json` — one card with `checklist` (`idx`, `text`, `done`) and all `events`.
- `tb boards --json` — `[{name, default, todo, doing, review, done}]`.
- `tb github --json` — the GitHub snapshot: `{repo, fetched_at, issues_open, prs[], issues[] (+state, who), merged_today[], main_ci}`.
- `tb github repos --json` — `[{name_with_owner, description, pushed_at, is_private, own}]`.
