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

**Escape hatch.** Drop the name, or stop pinning the file. (2.0.0 refused a `TB_BOARD` left
in the environment the same way; later versions let `TB_DB` win with a warning — see
[After 2.0.0](#after-200).)

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

# After 2.0.0

Not released yet. Nothing here needs a change to your scripts; two things are worth knowing
before you upgrade, and one is the way back.

## Board files are private; yours are reported, not changed

**Before.** tb created board files (and so their `-wal`/`-shm` sidecars) mode `0644`: any
user of the machine could read every card.

**Now.** Every file tb creates is `0600`, whatever the umask, in the boards folder and under
`TB_DB`. The boards you already have stay as they are — tb never changes the mode of an
existing file on its own, because a board may be shared with a group on purpose. Instead,
each command that opens such a board prints one warning line (and adds it to `--json` object
output as `warnings`):

`tb: /…/boards/work.db is open to other users (mode 0644) — make it private with 'tb work config file-mode private', or keep it that way with 'tb work config file-mode shared'`

A board path that is a symbolic link is followed: the board is the file the link leads to,
tb creates that file `0600` itself, and the warning names the real file. A link into a folder
that does not exist, or a loop of links, is refused with a hint — tb creates a board file
through a link, never folders — and no mode is ever changed through a link.

Run one of the two, once per board, and the line is gone. `private` changes the file and
its live sidecars and is logged on the board; `shared` records that the mode is deliberate.
Commands, exit codes and stdout are otherwise unchanged.

## `TB_DB` together with `TB_BOARD`

**Before (2.0.0).** With both set, every command was refused — a test harness that pinned
`TB_DB` broke as soon as the environment also named a board.

**Now.** `TB_DB` wins. The pinned file opens, `--json` reports the board as `default`, and tb
prints one warning line (`TB_DB is set, so TB_BOARD=work is ignored …`; `warnings` in
`--json`). A board name **typed on the command line** (`tb work …`, `-b work`) under `TB_DB`
is still refused, exactly as in section 2 above: that is the mistake the refusal exists for.

## A backup before every schema upgrade

When tb opens a board written by an older version and has to change its schema, it first
writes a copy next to the board and says where (stderr, and `warnings` in `--json`):

`<board file>.before-<tb version>.<UTC yyyymmdd-hhmmss>.bak`

The copy is made by SQLite, not by copying the file, so it is one complete database: it
includes cards that were still in the `-wal`, and it has no sidecars of its own. It is mode
`0600`, and it never ends in `.db`, so it is never listed as a board. Deciding, copying and
upgrading happen under the board's write lock, so however many `tb` processes open an older
board at the same moment there is **exactly one** backup, and it always holds the old
schema; the others wait, find the board current, and do nothing. The copy is written as
`….bak.partial` and renamed when complete, so a file named `….bak` is always a whole backup.
The upgrade itself is one transaction. If the backup cannot be written (no room, a read-only folder) **nothing is
upgraded** and the command fails with `cannot back up … — nothing was changed; …`. A board
that is already current is never copied. tb does not delete backups; remove them when you
no longer want the way back.

## Going back to an older tb

1. Stop everything that has the board open (agents, `tb watch`, the full-screen board).
2. Install the version you want — the installer pins one: `install.sh --version v2.0.0`.
3. Often that is all. Schema changes only add nullable columns, and an older tb ignores
   columns it does not know, so it opens an upgraded board as it is. What it cannot do is
   honour data and settings it has never heard of.
4. To get the board **exactly as the older version left it**, put the backup back. Move the
   current files aside first (anything written after the upgrade exists only there):

```sh
cd ~/.local/state/terminal-board/boards          # or the folder of your TB_DB file
mkdir after-upgrade
mv work.db after-upgrade/                        # and work.db-wal / work.db-shm, if present
cp work.db.before-*.bak work.db                  # pick the one you want if there are several
chmod 600 work.db
```

   The backup is a single complete file: there is no `-wal` or `-shm` to bring along.
