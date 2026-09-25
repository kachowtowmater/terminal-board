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
| `events[]` | `{ts, actor, kind, text, actor_id}` | the last 10, oldest first. `actor` is the short display name, as always; `actor_id` (int\|null) is the `id` of the **identity** behind it — look it up in the top-level `actors[]` of `tb board --json` / `tb show ID --json`. It is null when nothing but the name is known (a person in a plain terminal) and on every event written before identities were recorded. Kinds include `created`, `taken`, `moved`, `returned` (a reviewer sent it back; `text` is the reason, right after its `moved` `review -> doing`), `due` (the due date changed; `text` is `OLD -> NEW`, `none` for no date), `note`, `check`, `link` (`tb link`; `text` is `+ LABEL: VALUE` or `- LABEL: VALUE`), `blocked`, `unblocked`, `dropped`, `released` (`tb release`: a dead holder's DOING card went back to TODO; `text` is `released HOLDER (liveness evidence): reason`), `edit`, `prio`, `github`, `force`, `approved` (somebody checked the card with `tb done ID --approve`, on any card; `text` is `checked by NAME (…)` and the card does not move), `reviewing` (claimed with `tb next --review`), `unclaimed` (claim released), `hook` (a pre/post-change hook ran and either allowed the change or, for `hook-after`, was attempted; `text` is `NAME allowed|failed in Nms`), `break-glass` (`--break-glass` skipped the pre-change hook this board asked for; `text` is `NAME: why`), `hook-nested` (the change was made by a hook's own `tb` call on this board while that hook ran, so the hook was not asked again; `text` names the hook and its event — also a row in `tb log`); the set is open — see the forward-compatibility rule above |
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
`doing`), `dropped` (e.g. `doing` → `todo`), `released` (`doing` → `todo`) and `moved` (e.g. `doing` → `review`) — so following
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
`sync --json` returns `{ "ok": true, "moves": [ { "card_id": 3, "gh_ref": 20, "from": "doing", "to": "review", "text": "PR gh#20 merged → review (a verifier moves it to done)" } ] }`. Sync never moves a card to `done`.

Failure (non-zero exit), for any command run with `--json` — including **argument errors**
(bad value, missing argument, unknown flag): the parser's plain text never replaces the JSON
object; parse failures answer on stdout with the same shape and exit **2** (usage) instead
of 1 (runtime):

```json
{ "ok": false, "error": "no card #9", "hint": "see 'tb list' for ids", "code": "no_card" }
```

An argument error names what is missing and gives the usage line, e.g. `tb note 1 --json` →
`"error": "argument error: the following required arguments were not provided: <TEXT>"`,
`"hint": "usage: tb note <ID> <TEXT> — see 'tb --help' …"`, `"code": "usage"` (exit 2).

`hint` always says what to run next, e.g. `doing is full (3/3)` → `finish one with 'tb done ID' first`,
or `issue gh#11 still open on GitHub` → `… 'tb done 11 --force' to mark it done anyway`.
When the board was chosen **explicitly by name or `-b`**, the command in a hint carries it —
`see 'tb work list' for ids` — so copying the hint into a fresh shell acts on the same board.
The name is left out only when a bare `tb` is certain to reach that board: it is the board
plain `tb` opens (the saved default board, else `default`) and no `TB_BOARD` is set. A board
picked by `TB_BOARD` or by the saved default travels with the environment, so its hints stay bare.

### `code` — a stable symbol to branch on, never `error`'s prose

`error` and `hint` are prose for a person: rewording either is not a breaking change, and has
happened before (#25 changed argument errors from plain text to this JSON shape without
bumping `"v"`). Before `code` existed, an agent that needed to tell "the card is held by
someone else" from "no card #N" from "doing is full" apart had no choice but to substring-match
that prose — freezing it into a de-facto contract. `code` is the real, stable contract: a
lowercase snake_case symbol, present and non-empty on **every** `--json` failure object, runtime
or argument error, on every command that can fail. It is carried on the error type itself
(`store::BoardError`'s second field, `store::Code`), not re-derived from the message text, so
adding a new failure path without a code is a compile error, not a runtime gap.

**The vocabulary is OPEN.** New codes are added as tb grows; a consumer that meets one it does
not recognize falls back to `error`'s text and the exit status — exactly as it must for any
future addition. Once shipped, a code's spelling and meaning are a contract: never renamed,
never reused for a different failure. `unknown` is the explicit catch-all — carried today by
error paths that predate this vocabulary or that do not yet warrant their own symbol — and
means exactly the same thing to a consumer as a code it has never seen: read `error`/`hint`.

The five 2.0.0 refusals, pinned by golden tests:

| code | refusal |
|---|---|
| `not_owner` | changing a DOING card held by another actor without `--force` (the holder rule) |
| `db_pinned` | a board name given while `TB_DB` pins one file |
| `no_board` | the named board does not exist |
| `empty_actor` | `--as ""` |
| `reason_required` | sending a REVIEW card back to DOING with no reason |

Everyday failures:

| code | refusal |
|---|---|
| `no_card` | no card with that id, on the board or in the archive |
| `wip_full` | DOING is at the board's `wip` limit |
| `invalid_board_name` | a board name that is not `[a-z0-9_-]{1,32}` |
| `board_name_is_command` | a board name that collides with a command word |
| `gh_issue_open` | `tb done` refused: the linked GitHub issue is still open |
| `github_off` | a GitHub-only command run on a board with no `github` repo configured |
| `github_error` | a GitHub API/network call failed |
| `not_in_review` | an approval (`tb done --approve`) outside REVIEW |
| `not_in_doing` | `tb release ID` on a card that is not in DOING with a holder |
| `holder_alive` | `tb release ID` refused: a liveness probe (the same ones `tb-reap` uses — herdr agent, tmux session, pane label, agent process, live actor session, headless pid) vouches for the card's holder; `error` names which |
| `not_releaser` | `tb release ID` by an agent whose role (`TB_ROLE`) is not `lead`/`orchestrator` (a person may always release) |
| `self_approve` | the actor who did the work tried to approve or review their own card (never-approve-your-own-work) |
| `same_session` | REVIEW -> DONE by a verifier running in the same recorded session (`TB_SESSION`/the harness's session id) as an identity that took the card or moved it into review, in any round; sessions only a harness records, so a person and every old event are never refused (`--force` gets past it, logged) |
| `done_by_restricted` | `config done-by` restricts who may close a card, and the actor is not on the list |
| `not_from_review` | a move into DONE from a column other than REVIEW (`todo -> done`, `doing -> done`): nothing reaches DONE except from REVIEW, whoever asks (`--force` gets past it, logged) |
| `not_verifier` | REVIEW -> DONE by an agent (a harness in its identity) whose role (`TB_ROLE`) is not `verifier`/`reviewer` and whose name is not on `config verifiers`; on unless `config verifier-only off` |
| `person_only` | an agent (a harness in its identity) tried to change `config verifiers` or `config verifier-only` — a person's settings — or to `tb boards delete` |
| `done_needs_note` | `config done-needs-note` requires a fresh note before DONE |
| `done_needs_link` | `config done-needs-link` requires a link with that label before DONE |
| `arg_required` | a required argument or value was not given |
| `unknown_command` | an unrecognized subcommand or command word |
| `unknown_setting` | an unrecognized `tb config` key |
| `invalid_value` | a value given for a recognized field/setting/flag is not one it accepts |
| `db_error` | the database could not be opened, read or written (including "locked, try again") |
| `io_error` | reading or writing a file (settings, text-from-file, stdin, export) failed |
| `terminal_error` | the interactive TUI failed to start or run |
| `board_busy` | a board file could not be locked: something else has it open, or is moving it, past the wait |
| `default_board` | `tb boards archive` / `delete` refused: that is the board a bare `tb` opens right now |
| `board_exists` | `tb boards restore` refused: a live board (or a stray `-wal`/`-shm`) is already at that name |
| `no_archive` | `tb boards restore` / `delete` refused: no archived board has that name |
| `board_live` | `tb boards delete` refused: that name is a live board — archive it first (with `--backups`: a live board of that name exists, and the backups may be its own) |
| `confirm_required` | `tb boards delete` without `--yes`, not on a terminal (or with `--json`) |
| `usage` | a command-line argument failed to parse (clap): missing/extra/malformed flags, an unrecognized subcommand caught at the parser level, wrong arity |
| `hook_refused` | a pre-change hook (`config hook`, `tb trust`) refused the change, could not be run, is untrusted or changed, or this machine does not know it |
| `hook_race` | a pre-change hook allowed the change, but the card changed while the hook ran (someone else took or moved it), so nothing was written — retry the command; the hook is asked again |
| `no_hook` | `tb trust NAME --sha256 HEX` / `--timeout SECS` / `tb trust NAME`: this machine has no hook recorded under that name |
| `unknown` | the open-ended catch-all above |

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
  "code": "invalid_value", "command": "edit", "source": "dates.json", "dry_run": false,
  "problems": [ { "row": 250, "id": 41, "field": "due", "problem": "'2026-02-30' is not a real calendar date", "hint": "use YYYY-MM-DD, …" },
                { "row": 251, "id": 97, "field": "id", "problem": "no card #97", "hint": "see 'tb list' for ids" } ] }
```

A file that cannot be read at all (missing, not JSON, not cards, empty, too big, a terminal
on `-`) is the usual `{ok:false,error,hint,code}`. One write transaction: a second import at
the same moment waits, then runs whole.

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

`--json` is an array, oldest first, of `{v, ts, card_id, actor, actor_id, kind, text, identity}` —
the same event fields `tb watch --events` streams, without the live stream's `from`/`to`.
`identity` is the whole **identity** object behind `actor_id` (harness, model, role, session,
host), inline because a log has no `actors[]` to look an id up in; `null` when none is known.
In plain text, a move into DONE ends with `(by NAME — harness model role session … on host)`:
the trace of who closed the card.
`--since` takes `YYYY-MM-DD`, meaning **local midnight in the board's zone**, or a unix
second; events are selected by their timestamp, so a history written out of order still
answers "everything since Tuesday" correctly.

Interleaved with the card events, oldest first by the same clock, are the board's own —
`mv` leaving a `moved-out` behind on the board a card left (the moved-in half is a card
event, and `tb show` on the new id prints it; the moved-out half has no card of its own to
attach to), a WIP change, a file-mode change, a soft-delete: `card_id` is **`null`** on these
rows, never a card's id repurposed to mean "the board" (`actor_id` is whatever it always is —
null when nothing but the name is known). Plain text marks the same row `board` where a card
row shows `#ID`. Previously nothing printed this half of a move's trail (#106).

## `tb list --done [--since DATE]`

The finished cards the board's DONE column shows (the last 24 hours), or — with `--since` —
every card finished at or after local midnight of that date, newest first. `--json` is an
array of card objects, the same shape as `tb list --json`.

`tb export`, `tb log` and `tb list` never write to the board file.

## Warnings — `"warnings": ["…"]`

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

## Who may write

- `tb config wip-per-owner N` — nobody may hold more than `N` DOING cards at once; `0` (the
  default) turns it off. It is a SECOND limit beside the board-wide `wip`, and both must pass:
  `wip` asks whether the board is full, `wip-per-owner` whether one person is. Each discounts
  blocked cards under `wip-counts-blocked no`, capped by its own number, so blocking everything
  can never hand out unlimited work at either level. The refusal names the cards you hold.
- `tb config actors NAME,NAME,…` (`--off` clears) — a write whose `--as` is not on the list is
  refused, so a typo cannot invent an agent. Matched trimmed and without case; the stored order
  and spelling are kept. **Reads are never refused**, `tb config` is never refused (a list can
  always be corrected), a list that leaves out the person setting it is refused, and the
  `github` name tb's own sync writes under is always allowed. Cards already held by a name that
  is not on the list are untouched and still listed.
- `TB_READONLY=1` (or `--read-only`) — every write is refused with
  `{"ok":false,"error":"read-only mode: \'tb add\' would change the board","hint":"unset TB_READONLY …"}`
  and exit 1. Reads are unaffected. It is enforced at the database connection as well as at the
  command, so a write cannot slip through; a board made by an older tb cannot be upgraded in
  this mode and says so rather than failing obscurely.

Both settings appear in `tb config` (and `tb config --json`) **only once set**, so a board that
uses neither lists exactly what it always did.

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
event records where it came from; the source board's log records where it went. Each event's
`actor_id` is re-keyed to the row the identity has on the destination (the identity is
inserted there when the board does not have it yet — matched on `(actor, harness, model, role,
session, host)`, not the id, which is per board), so a history that names its sessions still
names them. The column and the owner do **not** travel — a moved card lands in `todo`,
unowned. A `--on` that names a card is dropped (that number means a different card over
there); the block's text is kept.

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
- `tb boards --json` — `[{name, default, todo, doing, review, done, created_by}]`; `default` is true on the board plain `tb` opens here. `created_by` (#137) is who made the board, or `null` when nothing records it:
  `{actor, harness, model, role, session, host, at, source}` — `actor` is the name the creating command ran as
  (`--as`/`TB_AS`/…), and `harness`/`model`/`role`/`session`/`host` are that command's identity exactly as tb records
  it for a card's actor (`tb show`'s `actors:`), `null` when unknown (a person in a plain terminal records a name and a
  time only). `at` is RFC 3339 UTC (`"2026-09-25T10:06:25Z"`). `source` is `"board"` when the board's own file
  recorded it — written once, by `tb new NAME` or by the first command that created the board on first use (`tb NAME
  add …`), and kept by an archived board — or `"log"` for a board made before tb recorded this, read from
  `~/.local/state/terminal-board/board-creations.log` (`time<TAB>board<TAB>actor=…<TAB>session=…<TAB>host=…`, the last
  line naming the board; `-` is unknown). `tb boards --long` prints the same as one `created by …` line per board.
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
- `tb boards archive NAME --json` (#80) — `{ok:true, board, archived, restore}`: `archived` is the full path the board's file was moved to, `restore` is the exact `tb boards restore NAME` command that undoes it. Refusals (`{ok:false,error,hint,code}`): the board plain `tb` opens now (`default_board`), under `TB_DB` (`db_pinned`), an unknown board (`no_board`), a name that is not a board name (`invalid_board_name`/`board_name_is_command`), or the board open in another process (`board_busy`). Nothing is ever deleted; the file that moved is byte-identical to what was live.
- `tb boards restore NAME --json` — `{ok:true, board, path, from}`: `path` is where it landed (the boards directory), `from` is the archive file it came from. Refusals: a live board (or a stray `-wal`/`-shm`) already at that name (`board_exists`), no archived board with that name (`no_archive`), under `TB_DB` (`db_pinned`), or the destination held busy past the wait (`board_busy`).
- `tb boards delete NAME --yes --json` — `{ok:true, board, removed, kept_backups}`: `removed` is every file deleted (each archive of that name: `.db`, and `-wal`/`-shm` when there), `kept_backups` the board's schema-upgrade backups left in place (empty with `--backups`, which deletes them). Refusals: no `--yes` (`confirm_required`), a live board (`board_live`), no archived board of that name (`no_archive`), the board plain `tb` opens (`default_board`), an agent (`person_only`), the file open in another process (`board_busy`), under `TB_DB` (`db_pinned`), a name that is not a board name.
- `tb boards --archived --json` — `[{name, archived_at, path, todo, doing, review, done, created_by}]` (`created_by` as for `tb boards --json`, read from the archived file without changing it), oldest archive of a name first (the newest — what `restore` takes — is last for that name). `archived_at` is `"YYYY-MM-DD HH:MM"`, local time when it was archived. Card counts are `null` when the file cannot be read; reading them never changes the file (checked: `md5`/`sha256` identical before and after any number of `--archived` calls).

## Environment variables

`TB_NOW` (legacy `TTYBOARD_NOW`) pins the clock to a unix second so a test or a replay sees
stable timestamps. It changes what tb **writes**, so it validates: unset or empty = the real
clock; anything else must be an integer from `946684800` (2000) up to `4102444800` (one past
the last accepted, `4102444799`) —
a bad value is refused (exit 1) before anything is written, in plain text on stderr or as
`{ "ok": false, "error": "TB_NOW is not a plausible unix second: '…'", "hint": "unset it, …" }`.
The read-only variables (`TB_AS`, `TB_BOARD`, `TB_DB`, `TB_GH`, `TB_TTY`, `TB_NO_HERDR`,
`TB_NO_SETUP`, `TB_STDIN_TIMEOUT`) may stay lenient: a wrong value fails visibly where it is
used, or — `TB_STDIN_TIMEOUT` — is simply not applied.
