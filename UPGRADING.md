# Upgrading to 2.0.0

Nothing was renamed or removed in 2.0.0, and your boards open unchanged. The JSON contract is
still `"v": 1` and the SQLite schema only gained columns, so readers keep working.

What makes this a major release is that **eight things that used to succeed now behave
differently** — six commands are refused, and two change where their output goes. Every one
of them replaced a silent wrong answer, and every one has a way through. Work through the
list below; if none of these appear in your scripts, the upgrade is a no-op.

A quick way to find out: run your automation once and look for exit codes you did not
expect. Every refusal exits non-zero and prints what to run instead (and, with `--json`,
`{"ok":false,"error":…,"hint":…}`).

---

## 1. Finishing, dropping or moving a card someone else holds

**Before.** Any actor could `tb done` / `tb drop` / `tb move` a card out of DOING, whoever
held it. An off-by-one card id silently moved another agent's work and took over its author
record.

**Now.** Refused unless you hold the card:
`#1 is held by bot-1 — your cards: #2 · … use --force (logged)`.
The full-screen board asks y/n instead of refusing. GitHub automation and REVIEW → DONE
reviewers are unaffected.

**Escape hatch.** `--force`, which works as before and is logged on the card as its own
event. Better, pass the name that holds the card.

```sh
tb done 3 --as bot-2          # before: moved bot-1's card. now: refused
tb done 3 --as bot-2 --force  # same effect as before, and logged
```

## 2. `TB_DB` together with a board name

**Before.** With `TB_DB=/path/file.db` set, *every* board name opened that one file, while
`--json` and the header reported the name you typed. Two boards could blend with no sign.

**Now.** An explicit non-default board name under `TB_DB` is refused:
`TB_DB is set — board names are ignored; unset TB_DB to use boards`. Bare `tb` and the name
`default` still work, and JSON reports the board actually opened.

**Escape hatch.** Drop the name, or stop pinning the file.

```sh
TB_DB=/srv/tb/team.db tb work list   # before: opened team.db, called it "work". now: refused
TB_DB=/srv/tb/team.db tb list        # the same file, no pretence
tb work list                         # unset TB_DB to use real boards
```

## 3. A board name that does not exist

**Before.** Any command on an unknown non-default board showed an empty board, and some
commands created it — so a typo became a phantom board in `tb boards`.

**Now.** Only `add` and `config` (and a bare `tb` in a terminal) create a board. Everything
else fails with `no board 'demo-typo' — boards: … · create it with 'tb demo-typo add "…"'`
and creates nothing. The `default` board is unchanged.

**Escape hatch.** Create the board explicitly, once, before the rest of the script runs.

```sh
tb release list                      # before: an empty board (sometimes created). now: refused
tb release add "release: 2.0.0"      # creates the board, as it always did
```

## 4. A blank `--as`

**Before.** `--as ""` — usually `--as "$NAME"` in a shell where `NAME` is unset — fell
through to the login name, so the work was logged under a person who did not do it.

**Now.** Refused before anything is written:
`--as is empty — pass your agent name, e.g. --as bot-1`.

**Escape hatch.** Pass a real name, or omit the flag: an absent `--as` keeps the old
fallback chain (`TB_AS`, the pane's agent, the login name).

```sh
tb note 3 "done" --as "$NAME"   # with NAME unset: before, logged as you. now: refused
tb note 3 "done" --as bot-1     # say who
TB_AS=bot-1 tb note 3 "done"    # or set it once for the shell
```

## 5. Sending a REVIEW card back without a reason

**Before.** `tb move ID doing` on a REVIEW card moved it back silently, and the owner had to
guess what was wrong.

**Now.** The reason is required for that one move: `say why it goes back`. The reason is
logged as a `returned` event and the card shows its rework round (`r2`). Every other `tb
move` is unchanged.

**Escape hatch.** There is none, by design — pass the reason.

```sh
tb move 3 doing                                   # refused
tb move 3 doing "the export still drops the header"
```

## 6. `tb sync` and unowned TODO cards

**Before.** A TODO card nobody had taken, whose issue had an open pull request, was moved to
REVIEW by `tb sync` — where it had no owner, no author, and could be approved by anyone.

**Now.** Sync leaves it in TODO. The GITHUB panel still shows the open pull request in the
issue's STATE column (`PR gh#62 ok`). Once someone takes the card, the next sync moves it as
before, so every synced REVIEW card has an owner.

**Escape hatch.** Take the card first — one command, and it is what makes the card
accountable.

```sh
tb sync                   # before: the card jumped to REVIEW, ownerless
tb take 3 --as bot-1      # now: claim it,
tb sync                   # and sync moves it as it always did
```

## 7. A reader that closes the pipe early

**Before.** `tb list | head -1`, `tb config | grep -q …` and the like ended with a panic —
`failed printing to stdout: Broken pipe`, exit **101**.

**Now.** tb stops writing and exits **0**, on every command, as `tb watch` already did.

**Escape hatch.** None needed. If a script *tested* for the panic (`|| true`, or a check for
exit 101), that branch is now dead and can go.

```sh
tb list | head -1   # before: exit 101 and a panic message. now: exit 0
```

## 8. Argument errors under `--json`

**Before.** A bad value, a missing argument or an unknown flag printed the parser's plain
text on **stderr** and left stdout empty — even with `--json` in the command line. A JSON
consumer saw nothing and had to guess.

**Now.** With `--json` anywhere in the arguments, the same failures answer
`{"ok":false,"error":…,"hint":…}` on **stdout**. The exit code is still 2, `--help` and
`--version` are unchanged, and without `--json` nothing changed at all.

**Escape hatch.** None needed — but a script that read these errors from stderr should read
stdout instead.

```sh
tb note 3 --json          # before: text on stderr, empty stdout, exit 2
                          # now:    {"ok":false,"error":"…","hint":"usage: tb note <ID> <TEXT> — …"} on stdout, exit 2
```

---

## What did not change

- The JSON contract is `"v": 1`. Fields were added (`reviewer`, `round`, `last_note`,
  `last_event_at`, `unknown_refs`, `unchecked_refs`, `updated_at`); none were renamed or
  removed. Consumers must ignore fields and event kinds they do not know — see
  [docs/JSON.md](docs/JSON.md).
- The database gains nullable columns only, added when the board is opened; boards written by
  1.x open unchanged. See [docs/SCHEMA.md](docs/SCHEMA.md).
- Every command, flag and key that existed in 1.1.0 still exists.

---

# After 2.0.0: cards someone else holds

Not released yet. Three commands that used to succeed are now refused in one situation, and
one name is reserved. If your scripts only touch their own cards, nothing changes.

## `rm`, `edit` and `block` on a card someone else holds

**Before.** `done`, `drop` and `move` refused to take a DOING card away from its holder, but
`tb rm`, `tb edit` and `tb block` did not look: a stale or off-by-one id deleted another
agent's card with its whole history, rewrote its brief, or blocked it — exit 0, no trace.

**Now.** The same rule, the same refusal:
`#1 is held by bot-1 — your cards: none · to delete it anyway use --force (logged)`.
The full-screen board's `x` asks y/n naming the holder. `note`, `check` and `prio` stay open
to everyone, and cards nobody holds (TODO, REVIEW, DONE) are unaffected.

**Escape hatch.** `--force`, new on `rm`, `edit` and `block`, recorded as its own event.

```sh
tb rm 3 --as bot-2            # before: deleted bot-1's card. now: refused
tb rm 3 --as bot-2 --force    # goes through, and is logged
```

If you would rather never lose a card: `tb config rm archive` makes `tb rm` archive instead
(`tb list --archived`, `tb restore ID`). The default is unchanged.

## The name `github`

**Before.** Any command could run `--as github`, and because tb's own sync acts under that
name, it was let past the holder rule.

**Now.** A write (or the full-screen board) under the name `github` is refused:
`'github' is the name tb's own GitHub sync acts under — pass your own name, e.g. --as bot-1`.
`tb sync` itself is unchanged, and reads under that name still work.

