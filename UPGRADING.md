# Upgrading to 3.0.0

3.0.0 changes **who may move a card into DONE**. Nothing was renamed or removed; boards open
unchanged and the JSON contract is still `"v": 1`. Three things that used to succeed are now
refused or behave differently — every one has a way through.

## 1. Nothing reaches DONE except from REVIEW

**Before.** `tb done ID` on a TODO card, and `tb move ID done` from TODO or DOING, moved it
straight to DONE.

**Now.** Refused, for everyone, with code `not_from_review`:
`#3 is in todo — nothing reaches done except from review: take it first ('tb take 3'), then
'tb done 3' moves it to review; a verifier moves it to done (or --force, logged)`.

**Escape hatch.** `--force`, logged as a `force` event on the card. Better: take the card,
`tb done` it into REVIEW, and let a verifier close it.

## 2. REVIEW -> DONE only by a verifier (default ON)

**Before.** Anyone who had not done the work could move a REVIEW card to DONE — including the
orchestrator that dispatched it, or a builder agent.

**Now.** An **agent** — an actor whose recorded identity has a harness (Claude Code, omp, …;
`TB_HARNESS` sets it explicitly) — is refused with code `not_verifier` unless:

- it runs with `TB_ROLE=verifier` (or `reviewer`), or
- its name is on the board's list: `tb config verifiers rv-1,rv-2`.

A **person** (a plain terminal, no harness) always qualifies. The never-approve-your-own-work
rule still applies on top of both. tb sees an agent by its harness: `TB_HARNESS`, `AI_AGENT`
(Claude Code, pi), `OMPCODE` (omp), the `CODEX_*` variables (codex), `CLAUDECODE`, or a herdr
pane's record; a harness that exports none of these counts as a person.

Only a person changes `tb config verifiers` and `tb config verifier-only`: an agent is refused
with code `person_only`, so an agent refused `not_verifier` cannot list itself or switch the
rule off. Scripts that set these from inside an agent session must run them from a plain shell.

**The default is ON.** To keep the 2.x behaviour on a board, a person runs `tb config verifier-only off`
(logged in the board's own log; `on` turns it back). The switch covers only this rule —
section 1 stays. `--force` also gets past it, logged (`closed #N with no verifier role`).

```sh
TB_ROLE=verifier tb next --review --as rv-1   # a verifier claims and closes review work
tb config verifiers rv-1,rv-2                  # or name them on the board
tb config verifier-only off                    # or turn the rule off for this board
```

`tb done ID --approve` is unchanged: anyone who did not do the work may record a check, and
the card stays in REVIEW.

## 3. GitHub sync moves a closed issue or merged PR to REVIEW, not DONE

**Before.** `tb sync` (and the board's refresh) moved a card to DONE when its PR merged or its
issue closed — with no verifier.

**Now.** It moves the card to REVIEW with an event saying why
(`issue gh#21 closed → review (a verifier moves it to done)`), and leaves a card already in
REVIEW alone; a verifier moves it to DONE. `sync --json` never reports a move `to: "done"`.
A card a reviewer sent back stays in DOING for its owner.

---

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

## `tb add -d` trims the blank space around a description

**Before.** `tb add "x: title" -d "  Done = …  "` stored the spaces and the newlines around
the text exactly as given, while `tb edit --desc`, `tb note` and (since 2.0) `--desc-file`
all trimmed theirs. The same brief added and then edited came out as two different strings.

**Now.** Every way to write a description trims the blank space AROUND it and keeps
everything inside — `tb add -d`, `tb edit --desc`, `--desc-file`, and the new `tb import` /
`tb edit --from`. Nothing else about the text changes: tabs, blank lines, Windows line ends
and indentation inside the description are kept byte for byte, as they always were.

```sh
tb add "x: title" -d "  Done = …  " --json   # before: "  Done = …  "   now: "Done = …"
```

**The way through:** a script that depended on the leading or trailing space has to add it
back inside the text (for example as a blank line, which is kept). A description that had no
blank space around it is unaffected, and so is every stored card.

## Many cards from one file: a board named `import`

**Before.** `import` was not a command, so `tb import list` opened a board called `import`.

**Now.** `tb import FILE.json` creates cards from a file, so `import` is a command name and
can no longer name a board. A board already called `import` still has its file, but tb will
not open it by that name.

**The way through:** rename it — move `~/.local/state/terminal-board/boards/import.db`, and
its `-wal`/`-shm` files if they are there, to another name in the same folder:

```sh
cd ~/.local/state/terminal-board/boards && for f in import.db*; do mv "$f" "intake${f#import}"; done
```

## A board named `export` or `log`

**Before.** Neither was a command, so `tb export list` opened a board called `export`.

**Now.** `tb export` writes the board out and `tb log` prints its history, so both are command
names and neither can name a board. A board already called `export` or `log` still has its
file, but tb will not open it by that name.

**The way through:** rename it — move the file, and its `-wal`/`-shm` if they are there, to
another name in the same folder:

```sh
cd ~/.local/state/terminal-board/boards && for f in export.db*; do mv "$f" "outbox${f#export}"; done
```
