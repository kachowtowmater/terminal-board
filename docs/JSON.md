# Terminal Board JSON contract (schema v1)

Apps and agents drive Terminal Board through `tb … --json`. Every object below carries
fixed field names, pinned by golden tests (`tests/contract.rs`). A breaking change bumps
`"v"`; new fields may be added without a bump. All timestamps are **unix seconds**.
Text values are **cleaned** (`text::sanitize_json`, the same cleaner the screen uses):
terminal escape sequences are removed whole — a sequence's payload goes with it — and so is
every control character, **except line breaks and tabs, which are text and are kept**
(`\n` and `\t` in the JSON string). CR becomes a space: it is cursor motion, never text.
Removed: U+0000–U+0008, U+000B–U+000C, U+000E–U+001F, DEL U+007F and the C1 range
U+0080–U+009F. Kept: everything at U+00A0 and above — letters, accents, emoji, CJK. The JSON
stays valid, and no byte in it can move a terminal's cursor. The store keeps text raw.
Board selection works as usual: `tb [BOARD] …`, `-b NAME`, `TB_BOARD`, the saved default board
(`tb boards --default`, below), or `TB_DB=/path/file.db` — in the order `TB_DB` > a named board >
`TB_BOARD` > the saved default board > `default`.
With `TB_DB` set there is a single file — an explicit non-default board name is refused
(`TB_DB is set — board names are ignored; unset TB_DB to use boards`), so `board` never
reports a name that was not opened. A `TB_BOARD` in the environment is not a typed name: with
both set, `TB_DB` wins, `board` is `default`, and the command carries a warning (below).

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
  },
  "actors": [ identity, … ]
}
```

| field | type | notes |
|---|---|---|
| `v` | int | schema version (1) |
| `board` | string | board name |
| `wip` | int | WIP limit for DOING |
| `theme` | `"dark"`\|`"light"` | |
| `layout` | `"auto"`\|`"focus"`\|`"third-h"`\|`"third-v"`\|`"half-h"`\|`"half-v"` | TUI layout preference (older names in a board file are reported as the view they became) |
| `labels` | object | `{todo, doing, review, done}` → each column's display name (its label, else the name in capitals). Chrome: the keys of `columns` and every `card.column` stay the internal names |
| `github.repo` | string\|null | `owner/repo`, null when GitHub is off |
| `github.snapshot` | object\|null | the cached GitHub snapshot (same as `tb github --json` without the per-issue `state`/`who`) |
| `github.error` | string\|null | the last fetch error, shown next to the last good snapshot |
| `github.fails` | int | consecutive failed refreshes; the board UI goes red only after 3 |
| `github.fetched_at` | int | unix seconds of the last good snapshot (0 = never fetched) |
| `sort` | `"position"`\|`"due"` | what orders the `columns` arrays — and what `tb next` takes, which is always the first card of `columns.todo` whose `blocked` is null. `position` (default): by `position`. `due`: todo and review by `due`, nearest first; cards without a `YYYY-MM-DD` date after every dated card; equal dates (or none) by `position`, then `id`. doing is always by `position`. The order never depends on today |
| `columns.*` | card[] | todo/doing/review in the board's order (see `sort`; `position` order unless the board sets `sort due`); **done = every done card, newest first** (the TUI only shows the last 24h — filter on `column_since`) |
| `actors[]` | identity[] | every identity an event in `columns` points at with its `actor_id` (see **identity** below), in `id` order; `[]` when no event has one |

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
  "reviewer": null,
  "due": "2026-10-09",
  "days_left": 3,
  "due_state": "soon",
  "column_label": "DOING",
  "gh_ref": 327,
  "blocked": null,
  "blocked_on": null,
  "blocked_until": null,
  "recheck": false,
  "blocked_on_state": null,
  "created_at": 1789763036,
  "column_since": 1789763036,
  "checklist": [ { "n": 1, "idx": 1, "text": "repro", "done": false } ],
  "round": 1,
  "escalate": false,
  "approved_by": [],
  "events": [
    { "ts": 1789763036, "actor": "bot-2", "kind": "created", "text": "", "actor_id": 4 },
    { "ts": 1789763036, "actor": "bot-2", "kind": "taken", "text": "", "actor_id": 4 }
  ],
  "links": [ { "idx": 1, "label": "brief", "value": "docs/brief.md", "added_by": "bot-2", "added_at": 1789763036 } ]
}
```

| field | type | notes |
|---|---|---|
| `id` | int | stable card id |
| `title` | string | without the `tag:` prefix and without a **leading** `gh#N` token; a `gh#N` later in the title stays in the text |
| `tag` | string\|null | the tag: the one given with `--tag` (which may hold digits, spaces and hyphens), else the one parsed from a `tag: title` prefix |
| `description` | string | |
| `column` | `todo`\|`doing`\|`review`\|`done` | |
| `position` | int | order within the column, 0 = top |
| `owner` | string\|null | who holds it |
| `reviewer` | string\|null | who claimed it with `tb next --review`; kept when it reaches DONE, cleared by any other move |
| `due` | string\|null | the due date: a **calendar date** `YYYY-MM-DD`, the text given to `--due` (spaces around it are dropped). It is never an instant and no time zone applies to it, so it reads the same everywhere. (A board file written by something other than tb may hold older free text here; it is reported as is.) |
| `days_left` | int\|null | whole calendar days from the board's today to `due`: 0 = due today, negative = past. Today is the date in the board's `tz` setting (an IANA zone), or in the machine's local zone when unset — it turns over at local midnight there, not at UTC midnight. Null when `due` is null or not a `YYYY-MM-DD` date, and on a `done` card |
| `due_state` | `ok`\|`soon`\|`overdue`\|null | `overdue` = `days_left` < 0 · `soon` = 0 to `due-warn` days (default 3) · `ok` above that. Null exactly when `days_left` is |
| `column_label` | string | what a person reads for `column`: the board's label (`tb config label review "WITH REVIEWER"`), else the name in capitals. **Display only** — `column` is the name every command takes and it never changes |
| `gh_ref` | int\|null | GitHub issue/PR number. A **leading** `gh#N` (first word after the optional `tag:`) is moved out of the stored title; a `gh#N` **later in the title stays in the text** and still sets the link (the first such ref wins). |
| `blocked` | string\|null | what blocks it (e.g. `#7`) |
| `blocked_on` | string\|null | `--on`: who or what the card waits for — `#7` (another card) or a name |
| `blocked_until` | string\|null | `--until`: the local calendar date to look again, `YYYY-MM-DD` |
| `recheck` | bool | the `--until` date has arrived (today or earlier) in the board's `tz` — **derived at read time**, never stored and never a background clock, so a card becomes a recheck simply by being read on the day |
| `blocked_on_state` | `open`\|`done`\|`gone`\|null | only for `--on #ID`: whether that card is still open, is done, or is no longer on the board (deleted or archived). A card blocked on `#ID` is unblocked by tb when that card reaches DONE |
| `created_at`, `column_since` | int | unix seconds |
| `last_event_at` | int | unix seconds of the card's last event (any kind) — compute staleness yourself (the board shows `quiet 1h20m` on a DOING card quiet for 60+ minutes; fixed threshold, no setting) |
| `checklist[]` | `{n, idx, text, done}` | `n` is 1-based and canonical; `idx` is a deprecated alias with the same value (kept so older readers of `tb show --json` don't break; removed no earlier than the next major version) |
| `approved_by` | string[] | everyone who recorded `tb done ID --approve` on this card, oldest first, each once. A record of who checked it — **not** a permission; `done-by` (which says who may close a card) is a separate, self-asserted setting, see README |
| `round` | int | rework round: 1, plus one for every `returned` event (counted from all events, so it never drifts) |
| `escalate` | bool | sent back more times than `config max-rounds` allows — **derived at read time**, never stored, and always `false` on a `done` card. `false` on every board that has not set `max-rounds` (the default). Skipped by `tb next` / `tb next --review`'s automatic pick; never removed from `tb list`, `tb board` or `tb show` — see README |
| `events[]` | `{ts, actor, kind, text, actor_id}` | the last 10, oldest first. `actor` is the short display name, as always; `actor_id` (int\|null) is the `id` of the **identity** behind it — look it up in the top-level `actors[]` of `tb board --json` / `tb show ID --json`. It is null when nothing but the name is known (a person in a plain terminal) and on every event written before identities were recorded. Kinds include `created`, `taken`, `moved`, `returned` (a reviewer sent it back; `text` is the reason, right after its `moved` `review -> doing`), `due` (the due date changed; `text` is `OLD -> NEW`, `none` for no date), `note`, `check`, `link` (`tb link`; `text` is `+ LABEL: VALUE` or `- LABEL: VALUE`), `blocked`, `unblocked`, `dropped`, `edit`, `prio`, `github`, `force`, `approved` (somebody checked the card with `tb done ID --approve`, on any card; `text` is `checked by NAME (…)` and the card does not move), `reviewing` (claimed with `tb next --review`), `unclaimed` (claim released); the set is open — see the forward-compatibility rule above |
| `links[]` | `{idx, label, value, added_by, added_at}` | evidence attached with `tb link ID VALUE --label LABEL`, in the order added. `value` is a path, a sha or a URL as **plain text tb only stores** — never read, resolved or fetched (docs/SCHEMA.md, table `links`). `label` is free text (not a fixed set), lower-cased. `config done-needs-link LABEL` refuses `tb done` (or `tb move ID done`) while no link here has that `label`, case-insensitive; `github`'s own evidence-driven moves are exempt, same as `done-by` |

### identity

Who a name was when it wrote an event — the record behind the short `actor`, so work can be
traced back to the session that did it (docs/SCHEMA.md, table `actors`).

```json
{ "id": 4, "actor": "bot-2", "harness": "claude-code", "model": "model-x", "role": "coder",
  "session": "0b9f6a52-7c1d-4e0a-9f3b-2a6c1d8e4f70", "host": "buildbox",
  "first_seen": 1789763036, "last_seen": 1789766636 }
```

| field | type | notes |
|---|---|---|
| `id` | int | what an event's `actor_id` points at; one id per distinct `(actor, harness, model, role, session, host)`, so every command of one session shares it |
| `actor` | string | the display name, the same string as the event's `actor` |
| `harness` | string\|null | the agent harness, without its version: `$TB_HARNESS`, else what the harness exports, else herdr's record of the pane |
| `model`, `role` | string\|null | `$TB_MODEL`, `$TB_ROLE` — explicit only; no harness exports them and tb never guesses |
| `session` | string\|null | the harness's session id (`$TB_SESSION`, else exported by the harness, else from herdr). Never a path: a session file's path becomes the identifier in its name, or `path-` + 12 hex digits |
| `host` | string\|null | the machine: `$TB_HOST`, else the first label of its host name |
| `first_seen`, `last_seen` | int | unix seconds of this identity's first and latest event |

Every value is **self-reported** — a claim, like the name itself, not proof — and arrives
cleaned of control characters and cut to 64 characters.

## `tb watch --json` — live stream (NDJSON)

One full board object (as above) per line: the first line immediately, then a new line on
every change by anyone (polls SQLite `PRAGMA data_version` every ~300 ms, so writes from
other processes and GitHub cache refreshes both count). Exits cleanly when stdout closes.
A board with open due dates is also sent again when its day turns — local midnight in the
board's `tz` — because every `days_left` moved and a `due_state` may have flipped without a write.

```sh
tb watch --json | while read -r line; do …; done
```

## `tb watch --events --json [--since TS]` — one line per event (opt-in)

Plain `tb watch --json` above is unchanged byte-for-byte; the event stream is opt-in:

```sh
tb watch --events --json                # one NDJSON line per event
tb watch --events --json --since 1789777000   # resume: only events at/after that unix second
```

Each line is `{v, ts, card_id, actor, kind, from, to, text, actor_id, identity}`: `kind` is the event kind
(`created`, `taken`, `moved`, `note`, `check`, …); `from`/`to` are the column transition of
every event that changes a card's column — `created` (null → `todo`), `taken` (`todo` →
`doing`), `dropped` (e.g. `doing` → `todo`) and `moved` (e.g. `doing` → `review`) — so following
them tracks every card's column; they are null for every other kind; `text` is the event's
text (the note, the block reason, …). `--since` resumes after a restart: only events at/after
that unix second are streamed, in `(ts, id)` order — an orchestrator records the last event
it saw and passes the next start second on restart. `actor_id` and `identity` say who `actor`
was: a stream has no `actors[]` to look an id up in, so the whole **identity** object (above)
is on the line — both are null when nothing but the name is known.

## Writes — `--json` results

Every write command takes `--json`: `add`, `next`, `take`, `assign`, `note`, `check`, `link`,
`move`, `done`, `block`, `drop`, `rm`, `prio`, `edit`.

Success (exit 0) — the card after the change (for `rm`, the card as it was):

```json
{ "ok": true, "card": { …card… } }
```

On a board set to `tb config rm archive`, `rm` archives instead of deleting and says so with
one more field, `"archived": true` (absent on a plain delete). `tb restore ID --json` answers
like any write, with the restored card. `tb list --archived --json` is an array, most
recently archived first: `[{ "id": 3, "title": "…", "tag": null, "column": "review",
"owner": "bot-1", "archived_at": 1790000000, "archived_by": "lead" }]` — `column` and `owner`
are where the card was, and returns to. Archived cards appear in no other output.

Text from a file — `add … --desc-file PATH|-`, `edit ID --desc-file PATH|-`, `note ID --file PATH|-`
(`-` = standard input) — answers the same `{ "ok": true, "card": … }`; `description` and the
note's `text` carry the file's text, **cleaned** as every text value is (see the top of this
file); the store keeps the bytes as written. Line breaks and tabs survive, so a description
written from a file goes out through `export --json` and back in through `import` or
`edit --from` unchanged. A file that
cannot be used is a runtime failure (exit 1) in the usual shape: no such
file, a directory, not UTF-8, a NUL byte, empty, over 262144 bytes (256 KiB), or `-` with a
terminal on standard input (refused at once, never waited on). Text given twice (`--desc` with
`--desc-file`, note text with `--file`) is an argument error (exit 2).

`assign ID NAME --json` answers the same `{ "ok": true, "card": … }` shape as `take` — `card.owner`
is `NAME`, never the identity behind `--as`; who ran the assign is only in the event log (`kind:
"assigned"`, `actor` is the assigner, `text` is `"assigned to NAME"`), not in this response.

`config KEY VALUE --json` returns `{ "ok": true, "config": { "key": "wip", "value": 4 } }`.
`config rules "TEXT"|--file PATH --json` returns `{ "ok": true, "config": { "key": "rules",
"value": "TEXT" } }`; `--off` answers `"value": null`; `config rules --json` (no value) reads it
the same way, `null` when unset. `tb next --json` (either form) adds one more field, **only the
first time an agent is shown a board's current rules text**: `"rules": "TEXT"` alongside `"card"`
— absent every other time, including every `--json` response from every other write command, so
existing consumers see no new field until they ask `tb next` on a board that sets `rules`.
`prio --json` on a column that `sort due` orders by date adds `"note"`: position is only the
tie-break there, and the note says where the card is now (`#5 is 6 of 7 in todo (was 7)`).
`config sort --json`, `config tz --json`, `config due-warn --json` and `config done-needs-link --json` (no value) read
one setting in the same shape, default included: `"value": "local"` / `"value": 3` / `"value": null` (off).
`config --json` lists them once set.
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
or `issue gh#11 still open on GitHub` → `… 'tb done 11 --force' to mark it done anyway`.
When the board was chosen **explicitly by name or `-b`**, the command in a hint carries it —
`see 'tb work list' for ids` — so copying the hint into a fresh shell acts on the same board.
The name is left out only when a bare `tb` is certain to reach that board: it is the board
plain `tb` opens (the saved default board, else `default`) and no `TB_BOARD` is set. A board
picked by `TB_BOARD` or by the saved default travels with the environment, so its hints stay bare.

## Warnings — `"warnings": ["…"]`

## `tb import FILE|-` and `tb edit --from FILE|-` — many cards from one file

A **row is the card object above**, so `tb board --json` or `tb show ID --json` output can be
edited and fed back. Documents: an array of cards, `{"cards": […]}`, a whole board object
(`columns.todo`, `doing`, `review`, `done`, in that order) or one card. UTF-8, at most 4 MiB.

| field | `tb import` (new cards, always in TODO) | `tb edit --from` (keyed by `id`) |
|---|---|---|
| `id` | ignored (the board assigns ids) | **required** — the card to change; one row per card |
| `title` | **required**; may carry `tag:` and `gh#N` as `tb add` accepts | optional; without its own `tag:` the card keeps its tag |
| `tag`, `gh_ref` | optional; must agree with what the title carries | optional; `null` clears |
| `description` | optional (trimmed, like every description) | optional |
| `due` | `YYYY-MM-DD` or `null` — the strict date rule | the same; `null` clears |
| `blocked` | text or `null` (`by #7` = `#7`, as `tb block`) | the same; `null` unblocks |
| `checklist` | texts, or `{text, done}` (`n`/`idx` ignored) | ignored |
| everything else | **ignored, with one warning**: `column`, `position`, `owner`, `reviewer`, timestamps, `round`, `escalate`, `days_left`, `due_state`, `events`, unknown fields | the same |

In `edit --from` an absent field is left alone and a value the card already has is no change
(no event), so a file can be run twice. It follows **the holder rule**, exactly as a single
`tb edit` does: a row naming a DOING card somebody else holds is refused — and because a bulk
edit is all or nothing, that refuses the WHOLE file, with the row named. `--force` overrides
it and writes the same `force` event per card (`edited #ID held by OWNER`). A REVIEW card is
not held, so it needs no override. Events are the single commands' own — `created` +
`imported` (text `row N of FILE`), `edit`, `due`, `blocked` / `unblocked` — by whoever ran it.

Success (exit 0; `--dry-run` answers the same object with `"dry_run": true` and writes nothing):

```json
{ "ok": true, "command": "edit", "source": "dates.json", "dry_run": false,
  "rows": [ { "row": 1, "id": 7, "action": "changed", "title": "docs: write the guide",
              "changes": [ { "field": "due", "from": null, "to": "2026-10-09" } ], "ignored": [] } ],
  "changed": [7], "unchanged": [], "warnings": [] }
```

`action` is `created`, `changed` or `unchanged`; `import` answers `"created": [ids]` instead of
`changed` / `unchanged`. Refused (exit 1) — **nothing was written**; every problem names its row
(from 1), its card when the row names one, and its field:

```json
{ "ok": false, "error": "2 problems in dates.json", "hint": "nothing was written; fix them and check again with 'tb edit --from dates.json --dry-run'",
  "command": "edit", "source": "dates.json", "dry_run": false,
  "problems": [ { "row": 250, "id": 41, "field": "due", "problem": "'2026-02-30' is not a real calendar date", "hint": "use YYYY-MM-DD, …" },
                { "row": 251, "id": 97, "field": "id", "problem": "no card #97", "hint": "see 'tb list' for ids" } ] }
```

A file that cannot be read at all (missing, not JSON, not cards, empty, too big, a terminal
on `-`) is the usual `{ok:false,error,hint}`. One write transaction: a second import at the
same moment waits, then runs whole.

## `tb export` — the whole board, with its history

```sh
tb export --json              # one document, re-importable
tb export --csv               # one row per card, for a spreadsheet
tb export --csv --history     # one row per event instead
```

```json
{ "v": 1, "board": "default", "exported_at": 1790069406, "tz": "America/Los_Angeles",
  "cards": [ { "…card…": null }, "…" ] }
```

Each element of `cards` is **the card object above** — the same fields `tb board --json`
prints — except that `events` holds **every** event, oldest first, not the last ten. `tz` is
the board's zone (`tb config tz`) or null. `cards` is exactly the array `tb import` and
`tb edit --from` accept, so export → edit → import is a round trip (the fields those commands
ignore are listed in their own section above, and are reported as warnings, not errors).

The output is **streamed**: a board with 10,000 cards and 110,000 events exports in 0.74 s in
about 15 MB of memory, because one card is held at a time rather than the whole document.

**CSV** is for a person opening the file in a spreadsheet, and is one-way — `--json` is what
comes back. It carries a UTF-8 **byte-order mark** (without one Excel reads the file as the
local code page and mangles every accent and dash), RFC 4180 quoting with `"` doubled and any
cell holding a comma, a quote or a line break wrapped, and CRLF row ends. Times are local
`YYYY-MM-DD HH:MM` in the board's zone. **A cell that would begin `=`, `+`, `-`, `@`, a tab or
a carriage return is written with a leading apostrophe**, so a card title is text in the sheet
and never a *formula* — card text is written by other people, and a spreadsheet would
otherwise run it. The stored card is unchanged; this is a property of the file.

## `tb log [--json] [--since DATE]` — the board's history

`--json` is an array, oldest first, of `{v, ts, card_id, actor, actor_id, kind, text}` — the
same event fields `tb watch --events` streams, without the live stream's `from`/`to`.
`--since` takes `YYYY-MM-DD`, meaning **local midnight in the board's zone**, or a unix
second; events are selected by their timestamp, so a history written out of order still
answers "everything since Tuesday" correctly.

## `tb list --done [--since DATE]`

The finished cards the board's DONE column shows (the last 24 hours), or — with `--since` —
every card finished at or after local midnight of that date, newest first. `--json` is an
array of card objects, the same shape as `tb list --json`.

`tb export`, `tb log` and `tb list` never write to the board file.

## `tb agents --json`

Some things tb has to say without failing the command: `TB_BOARD` was ignored because
`TB_DB` pins a file; the board file can be opened by other users; the board was backed up
before its schema was upgraded. Each is one line on **stderr** (`tb: …`), and with `--json`
every **object**-shaped result — a write, `board`, `show`, `config`, `sync`, a failure —
also carries them as its last key:

```json
{ "ok": true, "card": { …card… }, "warnings": ["TB_DB is set, so TB_BOARD=work is ignored and the pinned file is used — unset TB_BOARD (or TB_DB) to stop this warning"] }
```

The field is **absent when there is nothing to say** — it is never an empty list — so output
without warnings is unchanged. Array results (`list`, `boards`, `agents`) and `watch` lines
have no place for a field: read their warnings on stderr. A warning never changes the exit
code, and its wording is for people: act on `ok` and the exit code, show `warnings` to someone.

## Filters — `tb list` and `tb board --json`

`--tag`, `--owner`, `--blocked`, `--blocked-on`, `--due-before`, `--column` and `--group tag`.
They combine, and every one that is set has to pass, and they apply to **every** way `tb list`
picks cards: the ordinary list, `--done [--since]` and `--archived`. An archived card keeps
only its title, tag, column and owner, so `--blocked`, `--blocked-on` and `--due-before` are
**refused by name** there rather than ignored. `--all-boards` is refused with `--done` and
`--archived`: "finished today" and "archived" are each one board's own question. A filter is
never accepted and quietly dropped. A filter **removes rows and nothing else**: the cards stay in the order the board defines (position, or `sort due`), so a filtered
result is always a subsequence of the unfiltered one, column by column. `--tag none` and
`--owner none` are the cards without one. `--column` takes the internal name only — a display
label is refused, and the refusal names the column to use. A value that is not a date, not a
column and not a grouping is refused before the board is read.

## `tb mv ID --to BOARD` — a card on another board

```json
{ "ok": true, "from_board": "default", "to_board": "work", "old_id": 3, "id": 12,
  "title": "docs: write the guide", "checklist": 2, "events": 7, "links": 1 }
```

`id` is the card's number on the board it arrived at, and it is **not** `old_id`: ids belong to
a board. The card, its checklist, its links and its whole history travel, with each event's
original actor and time (and each link's original `added_by`/`added_at`), and a `moved-in`
event records where it came from; the source board's log records where it went. The column and
the owner do **not** travel — a moved card lands in `todo`, unowned. A `--on` that names a card
is dropped (that number means a different card over there); the block's text is kept.

Refused (exit 1, the usual `{ok,error,hint}`): a destination that does not exist (tb never
creates one), the board the card is already on, a card somebody else holds in DOING (add
`--force`, which is logged as a `force` event on the moved card and on both boards' logs), and
any move under `TB_DB`, which pins a single file.

A move holds the **source** board's write lock for the whole operation — read, far-end write,
delete. So a write to the source that arrives during a move waits and then finds the card gone
(a refusal), instead of being acknowledged and then destroyed by the delete; and two moves of
the same card cannot both succeed.

## `tb list --all-boards --owner NAME`

Every board on this machine, filtered the same way, as an array of card objects with one extra
key: `board`, the board each card is on. A board that cannot be read is named on stderr and
skipped. Refused under `TB_DB`.

## `tb agents --json`

Who is on **this** board, then the herdr agents that are not — the list the AGENTS panel
shows, in the same order. The board says who: the owner of every card that is not `done`, the
reviewer of every `review` card, and every actor with a card event in the last hour (the
`github` sync is not one). herdr only adds the live fields, and only from a pane whose agent
name is exactly that name (ASCII case aside; never a pane label or a title). So the array is
useful without herdr, and empty only when nobody is on the board and herdr shows no agents.

```json
[ { "name": "bot-2", "harness": "aider", "status": "working", "pane_id": "w:p5", "job": "fix #327", "card_id": 1, "last_note": "tests pass, opening PR", "last_event_at": 1789763036, "on_board": true, "card_role": "owner" },
  { "name": "rev-1", "harness": "-", "status": "-", "pane_id": "", "job": null, "card_id": 4, "last_note": "reading the diff", "last_event_at": 1789763100, "on_board": true, "card_role": "reviewer" },
  { "name": "bot-9", "harness": "codex", "status": "working", "pane_id": "w:p7", "job": null, "card_id": null, "last_note": null, "last_event_at": null, "on_board": false, "card_role": null } ]
```

| field | type | notes |
|---|---|---|
| `name` | string | the name as the board has it; for an agent that is not on the board, the herdr agent name, else the pane label |
| `harness` | string | `claude`, `aider`, …; `-` when no herdr pane has exactly this name |
| `status` | string | `working`, `idle`, `done`, `blocked`, `unknown`; `-` when no herdr pane has exactly this name |
| `pane_id` | string | empty when no herdr pane has exactly this name |
| `job` | string\|null | the last `·` segment of the pane label |
| `card_id` | int\|null | the card it is on: its DOING card, else the card it reviews, else another open card it owns |
| `last_note` | string\|null | that card's last note text (what the agent says it is doing) |
| `last_event_at` | int\|null | unix seconds of the card's last event — compute the age yourself; an agent that never notes shows an old age |
| `on_board` | bool | true for the board's own actors (listed first); false for a herdr agent that is none of them — tb does not read other boards, so it says nothing about what those are doing |
| `card_role` | string\|null | what `card_id` is to it: `owner` or `reviewer`; null without a card |

## Other read commands

- `tb list --json` — array of cards (without checklist/events; with `days_left` and `due_state`). In id order, as always — except on a board set to `sort due`, where it is in the board's order (todo, doing, review, done; each as `columns.*` above), so it agrees with `tb next`.
- `tb show ID --json` — one card with `checklist` (`n`, `idx`, `text`, `done` — the same shape as in `tb board --json`), `links` (`idx`, `label`, `value`, `added_by`, `added_at`), `round`, all `events` (each with its `actor_id`) and `actors[]`: the **identity** of everyone who wrote one of them (`[]` when no event has one).
- `tb boards --json` — `[{name, default, todo, doing, review, done}]`; `default` is true on the board plain `tb` opens here.
- `tb boards --default --json` — `{ok, default, source, setting, missing}`: `default` is the board plain `tb` opens
  in this environment, `source` says why (`"TB_DB"` | `"TB_BOARD"` | `"setting"` | `"builtin"`), `setting` is the
  saved default board (string, or null when none is saved — and always null under `TB_DB`, where it is not
  read). `missing` is true when a board is saved but its file is gone — plain `tb` then refuses, and the
  text answer says so instead of claiming it opens it. `tb boards --default NAME --json` and
  `--default --clear --json` answer the same object after the change. Refusals (exit 1, the usual `{ok:false,error,hint}`): an unknown or archived board, a name that is
  not a board name, a name together with `--clear`, any change under `TB_DB`, a settings file that is not a
  JSON object (never overwritten). The setting lives in `~/.config/terminal-board/config.json` (`TB_CONFIG`
  names another file), never in a board file.
- `tb github --json` — the GitHub snapshot: `{repo, fetched_at, issues_open, prs[], issues[] (+state, who), merged_today[], main_ci}` plus the sync state: `error` (the full text of the last fetch error, null after a good fetch) and `fails` (consecutive failed refreshes — the board header says `synced HH:MM · offline, retrying` or `· gh error`, in red only after 3 in a row, and never adds a row to the panel).
- `tb github repos --json` — `[{name_with_owner, description, pushed_at, is_private, own}]`.

## Environment variables

`TB_NOW` (legacy `TTYBOARD_NOW`) pins the clock to a unix second so a test or a replay sees
stable timestamps. It changes what tb **writes**, so it validates: unset or empty = the real
clock; anything else must be an integer from `946684800` (2000) up to `4102444800` (one past
the last accepted, `4102444799`) —
a bad value is refused (exit 1) before anything is written, in plain text on stderr or as
`{ "ok": false, "error": "TB_NOW is not a plausible unix second: '…'", "hint": "unset it, …" }`.
The read-only variables (`TB_AS`, `TB_BOARD`, `TB_DB`, `TB_GH`, `TB_TTY`, `TB_NO_HERDR`,
`TB_NO_SETUP`) may stay lenient: a wrong value fails visibly where it is used.
