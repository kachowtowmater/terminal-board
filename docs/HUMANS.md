# Terminal Board — the guide for people

This is how you use the board day to day. (AI agents have their own manual:
[AGENTS.md](AGENTS.md), or `tb guide`.)

## Open it

Type `tb` in a terminal. You get the board for the `default` board file, full screen,
refreshed automatically when anyone (you, a teammate, an agent) changes it. `q` quits,
`?` shows every key.

The very first time, `tb` asks a few setup questions first (GitHub, agents); press `s` to
skip them. `tb setup` runs them again later.

## The columns

| column | means |
|---|---|
| **TODO** | waiting to be picked up; the top card is next |
| **DOING** | someone is working on it; limited by the **WIP limit** (3 by default) |
| **REVIEW** | finished, waiting for someone to check it |
| **DONE today** | finished today (older done cards are kept, just not shown) |

## Cards

A card has a title (`tag: title`, e.g. `home: water the plants` — the part before the colon
is its **tag**), a description, a checklist, a history of notes, an owner and a column.

| to | on the board | from the command line |
|---|---|---|
| add a card | `a`, type `tag: title`, Enter | `tb add "home: water the plants"` |
| open it (description, checklist, history) | `enter` (`esc` closes) | `tb show 3` |
| edit title and description | `e` (Tab switches field, Enter saves) | `tb edit 3 --title "…" --desc "…"` |
| delete it | `x`, then `y` | `tb rm 3` |
| keep deleted cards instead | — | `tb config rm archive`, then `tb list --archived` · `tb restore 3` |
| add a note to its history | `n` | `tb note 3 "called the plumber"` |
| mark it blocked | — | `tb block 3 "#5"` · `tb block 3 --clear` |
| give it a due date | — | `tb edit 3 --due 2026-10-09` · `tb edit 3 --due none` · `tb add "…" --due 2026-10-09` |

A due date is a calendar date (`YYYY-MM-DD`), kept exactly as typed — it never moves a day
because of a time zone. `tb config tz America/Los_Angeles` sets the zone that decides what
"today" is for the whole board (default: your machine's); `tb config due-warn 5` sets how many
days ahead a card counts as due soon (default 3). More in the README under *Due dates*.

`tb config sort due` turns the board into a deadline queue: TODO and REVIEW show the nearest due
date first (overdue on top, cards without a date last, equal dates in the order you gave them),
and `tb next` hands out that card. Reordering by hand (shift+arrows, `tb prio`) then only orders
cards that share a date — the status line says so. `tb config sort position` is the default.

### Moving cards

| to | keys |
|---|---|
| select a card | arrow keys |
| move to the next / previous column | **Shift+→ / Shift+←** (or `>` / `<`) |
| move up / down in its column | **Shift+↑ / Shift+↓** (or `K` / `J`) |
| mark done | `d`: DOING → REVIEW, and again REVIEW → DONE |

From the command line: `tb move 3 doing`, `tb done 3`, `tb drop 3` (back to TODO),
`tb prio 3 top`.

Nobody approves their own work: `d` on a REVIEW card you moved there yourself asks
`approve your own work? y/n` (the CLI refuses it and takes `--force`), and a card someone
else holds in DOING is not yours to finish or drop — the board asks, the CLI needs
`--force`. Both ways through are written into the card's history.

Sending a REVIEW card back (Shift+← or `<`) asks why on the footer line; the card returns
to its owner in DOING and shows its rework round, `r2`. From the command line:
`tb move 3 doing "the export still drops the header"`.

Some terminals and multiplexers swallow Shift+arrows; `<` `>` `K` `J` always work.

### Checklists

Open a card (`enter`), then: `a` adds an item, ↑↓ select one, `enter` ticks it, `d`
deletes it. From the command line: `tb check 3 --add "balcony"`, `tb check 3 1` (tick item
1), `tb check 3 --rm 1`.

### The WIP limit

DOING holds at most 3 cards by default. When it is full, finish or hand back a card first.
Select DOING and press `+` / `-`, or run `tb config wip 4`.

## Boards

Keep separate boards for separate things. `tb` opens `default`; any other name opens that
board. A board is created by its first `tb <name> add …` (or `tb <name> config …`) — any
other command on a name that does not exist says so and lists the boards you have, so a typo
never leaves a phantom board behind. (With `TB_DB` set there is one file only — board names
are refused; unset `TB_DB` to use boards. A `TB_BOARD` left in the environment is ignored
there, with a one-line warning.)

Board files are private (mode `0600`). A board from an earlier version is readable by other
users of the machine, and tb says so until you run `tb config file-mode private` (or
`tb config file-mode shared`, if that is what you want). Before a newer tb upgrades an older
board it writes a backup next to it and tells you where — see the README, "Where your data lives".

```sh
tb home                      # open the board called "home"
tb home add "call the plumber"
tb -b work list
tb boards                    # every board with its counts
```

`TB_BOARD=work` in your shell makes `work` the default.

On the board, `B` opens the board picker: every board with its counts, `enter` switches to
the one you choose without quitting, `esc` cancels. (`TB_DB` pins one file, so the picker
says so instead of offering a choice.)

## GitHub

Each board can show one GitHub repository: open pull requests with CI status, issues with
who is on them, what merged today and whether `main` is green.

- **Connect a repository:** press `R` on the board and pick one from the list (type to
  filter), or run `tb config github owner/repo`. You need the GitHub CLI (`gh`) logged in.
- **Move around:** `tab` (or ↓ from the last card) goes to the GITHUB panel; ↑↓ select a
  row. `enter` on the repository line opens the picker; `enter` on a pull request or issue
  opens it.
- **Add an issue as a card:** open the issue (`enter`) and press `a`. The card gets
  `gh#N` in its title, which links it to the issue.
- **Open it in the browser:** `o` in the same window.
- **Linked cards move themselves:** an open pull request for the issue moves the card to
  REVIEW; a merged pull request or a closed issue moves it to DONE. Never backwards.
- **Hide / show the panel:** `G`, or `tb config github-panel hidden|shown`.

Red on the board always means a problem: a failing check, a blocked card, or an agent that
went idle while holding a card.

## Watching agents

The AGENTS panel answers two questions: who is working on **this** board, and on what. It
lists the people and agents the board itself knows about — whoever holds a card that is not
done, whoever claimed a card to review it, and anyone who wrote to a card in the last hour —
so it works without any extra tool. Each row shows the card (`#4`, its `gh#`, its title), the
last note on it and how old that note is; a row that reviews a card says `review`, and someone
who holds nothing shows the last thing they did (`last created #7 5m`).

If your agents run in herdr panes, a pane whose agent name is **exactly** a name on the board
adds the live part of the row: `*` working, `-` idle, `x` blocked, and the harness. A name
that only looks similar adds nothing — the row stays, with `-` where the status would be,
rather than show somebody else's status. `!` in red means an agent went idle while still
holding a DOING card — it probably stopped halfway; look at its last note.

Agents in other herdr panes are not listed. They are counted: the header says
`7 agents (4 here, 3 elsewhere)` and the panel ends in `+3 elsewhere (not on this board)`.
"Elsewhere" means only that: tb does not read other boards, so it does not say what they
are doing (enter on that line names them). In a narrow pane a row gives up whole fields,
the least useful first: the status word (the mark already says it), then the note, then its
age, then the card title — the only part that is ever cut — and the card id last. `A` shows
or hides the panel; `tb agents` prints the same list in the terminal.

To hand work to an agent, add a card with a clear description ("Done = …") and a checklist,
and tell the agent: "Your work is on Terminal Board: run `tb next --as <your-name>`… Full
manual: `tb guide`." Its notes appear in the card's history as it works.

## Pane sizes and views

The board adapts to the size of its window. Put it in a small pane next to your editor and
it still works:

| view | shape | what you see |
|---|---|---|
| **focus** | small (under 40 columns or 16 rows) | one card, big: the one you're working on |
| **third-h** | wide and short (e.g. a bottom strip) | columns left, GITHUB and AGENTS right |
| **third-v** | tall and narrow (a side pane) | the columns stacked, then GITHUB, then AGENTS |
| **half-h** | half the screen or more | four columns side by side, panels below |
| **half-v** | half the width, tall | the columns as a 2×2 grid, panels below |

`L` pins a view (and cycles through them); `tb config layout auto` goes back to automatic.
A panel that doesn't fit becomes a one-line bar; `tab` shows it full screen and `esc` comes
back.

## Themes

`T` switches between dark and light (`tb config theme light`). The board paints its own
background, so it looks the same whatever your terminal theme is.

## Tips

- Write the description as "Done = …": it tells whoever picks the card up (person or agent)
  when to stop.
- Put the next thing to do at the top of TODO (`Shift+↑` or `tb prio 3 top`): `tb next`
  always takes the top card.
- A note per step keeps the history useful: "ordered parts", "waiting for the landlord".
- A long brief is easier as a file than as a quoted string: `tb add "tag: title" --desc-file
  brief.md`, `tb edit 3 --desc-file brief.md`, `tb note 3 --file notes.md` (`-` reads a pipe:
  `some-command | tb note 3 --file -`). The text arrives exactly as written — backticks, `$`,
  quotes and blank lines included — up to 256 KiB of UTF-8. The README has the details.
- Back up a board by copying `~/.local/state/terminal-board/boards/`.
- `tb list` prints the board without opening it; handy in scripts.
