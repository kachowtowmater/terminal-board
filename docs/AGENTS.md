# Terminal Board — the agent manual

Terminal Board (`tb`) is a task board that people and AI agents share. Work moves
through four columns:

```text
TODO ──next/take──▶ DOING ──done──▶ REVIEW ──done──▶ DONE
  ▲                   │
  └──────drop─────────┘        (block = a flag on any card: "stuck on #N")
```

You work through the `tb` CLI. Do not open the full-screen board: without a terminal,
bare `tb` prints the board once and exits. `tb guide` prints this manual.

## What a card is

| part | what it is | how you change it |
|---|---|---|
| `#ID` | the number every command takes | — |
| title | `tag: short title`, e.g. `docs: install guide`; `gh#N` links a GitHub issue | `tb edit ID --title "…"` |
| description | the brief: what to do and what "done" means | `tb edit ID --desc "…"` |
| checklist | numbered steps, each open or ticked | `tb check ID N` · `--add` · `--rm` |
| notes | the progress log people read | `tb note ID "…"` |
| owner | who holds it (you, once you take it) | `tb next` · `tb take` · `tb drop` |
| column | todo, doing, review, done | `tb done` · `tb move` · `tb drop` |
| blocked | what it waits on | `tb block ID "…"` · `--clear` |

## Start here: the five commands you need

```sh
tb next --as NAME            # take the top TODO card (atomic: nobody else gets it)
tb show ID                   # read the brief, checklist and history
tb note ID "what changed"    # log each step
tb check ID N                # tick checklist item N when it is really done
tb done ID                   # finished: DOING -> REVIEW
```

## Every command, by task

### Find work and read cards

| do this | run |
|---|---|
| take the top TODO card | `tb next --as NAME` |
| claim the top REVIEW card you did not do | `tb next --review --as NAME` |
| take one specific TODO card | `tb take ID` |
| see every card, by column | `tb list` |
| one card in full (brief, checklist, notes, history) | `tb show ID` |
| the whole board as JSON | `tb board --json` |
| follow changes live (one JSON line per change) | `tb watch --json` |

`tb next` takes the top TODO card that is not blocked. It fails with a hint when TODO is
empty or DOING is full.

### Report progress

| do this | run |
|---|---|
| add a note to the log | `tb note ID "tests pass, opening PR"` |
| tick (or untick) checklist item N | `tb check ID N` |
| add a checklist item | `tb check ID --add "update the docs"` |
| delete checklist item N (the rest renumber) | `tb check ID --rm N` |

### Update a card

| do this | run |
|---|---|
| change the title | `tb edit ID --title "docs: install guide for macOS"` |
| change the description / done criteria | `tb edit ID --desc "Done = …"` |
| mark it stuck, and on what | `tb block ID "#12"` or `tb block ID "waiting for API key"` |
| clear the block | `tb block ID --clear` |
| reorder inside its column | `tb prio ID top` · `bottom` · `up` · `down` |

### Move a card

| do this | run |
|---|---|
| finished your work: DOING → REVIEW | `tb done ID` |
| verified someone else's work: REVIEW → DONE | `tb done ID` |
| hand it back: → TODO, owner cleared | `tb drop ID` |
| put it in any column | `tb move ID todo` · `doing` · `review` · `done` |

`tb move ID doing` respects the WIP limit and makes you the owner if nobody owns the card.
`tb move ID todo` clears the owner.

### Create and delete cards

| do this | run |
|---|---|
| file new work | `tb add "tag: title" -d "Done = …" --check "step one" --check "step two"` |
| file work for a GitHub issue | `tb add "repo: gh#315 short title"` |
| delete a card you created by mistake | `tb rm ID` |

### Boards and settings

| do this | run |
|---|---|
| list boards with counts | `tb boards` |
| use another board | `tb NAME next`, `tb -b NAME next`, or `TB_BOARD=NAME` |
| read the settings (WIP limit, GitHub repo, …) | `tb config` |
| see the agents and the card each holds | `tb agents` |

Leave settings alone unless a person asks you to change them.

## Recipes

**Do a card from start to finish**

```sh
tb next --as NAME            # note the ID it prints
tb show ID                   # read "Done = …" and the checklist
tb note ID "reproduced the bug"
tb check ID 1
tb note ID "fix pushed, CI green"
tb check ID 2
tb done ID                   # -> REVIEW
```

**Stop before finishing:** `tb note ID "stopped at: …, next: …"`, then `tb drop ID`.

**Stuck:** `tb block ID "#12"` (or say what you wait on) and `tb note ID "why"`. Pick up
something else with `tb next`, or wait. `tb block ID --clear` when it moves again.

**Found more work:** file it instead of doing it silently:
`tb add "tag: follow-up title" -d "Done = …"`, then `tb note ID "filed #NEW"`.

**Card too big:** add the parts as new cards (`tb add`), note their IDs on the original, and
narrow the original with `tb edit ID --desc "…"`.

**Review someone's card (verifier):** `tb next --review --as NAME` claims the top REVIEW
card you did not do (atomic: two reviewers never get the same card; it skips your own work
and cards another reviewer claimed; `tb move ID review` releases a claim whose reviewer
stopped). `tb show ID`; check the done criteria; then `tb done ID` (→ DONE) with a note of
what you checked, or
`tb move ID todo` with a note of what is missing. You cannot approve a card you did
yourself: whoever moved it to REVIEW (its owner, if GitHub moved it) gets
`you did this work — ask another person or agent to review it`. (In the full-screen
board a person approving their own card is asked `approve your own work? y/n` instead.)

## Rules

- One card at a time. Take the next one only after `tb done` or `tb drop`.
- `doing is full (3/3)` is the WIP limit: finish or drop a card first. Do not raise it.
- Every error message ends with what to run next. Read it and do that.
- Notes are short and factual, one per step: "repro confirmed", "PR #123 opened".
- Tick only what is really done. Never tick ahead.
- Leave a note before you stop, drop or block a card.
- Never approve your own work: REVIEW → DONE is another agent's `tb done`.
- No `--force` unless a person told you to use it.

## Identity

You are, in order: `--as NAME`, `$TB_AS`, `$HERDR_AGENT_NAME`, then — inside a herdr
pane — the herdr agent name of your pane (tb asks herdr for `$HERDR_PANE_ID`), then `$USER`.
Inside a named herdr agent you can leave out `--as`; anywhere else pass it on every command,
because each command usually runs in a fresh shell and an exported `TB_AS` does not last.
Use the same name every time. Names are self-asserted: nothing checks that you are who you
say. The review rule stops honest mistakes, not an agent that lies about its name — so never
pass another agent's name to get past it. The board matches it to your herdr pane in the
AGENTS panel. An idle agent that still holds a DOING card is shown as a warning.

## GitHub

If the board is connected to a repo (`tb config github` prints it):

```sh
tb github                    # PRs, issues with state and owner, merged today, main CI
tb github --json             # the same, for parsing
tb sync                      # apply GitHub evidence to the board now
```

- A card with `gh#N` in its title follows GitHub: an open PR for issue N moves it to REVIEW,
  a merged PR or a closed issue moves it to DONE. Cards never move backwards.
- Name your branch after the issue (`fix/315-flags`) or write `Closes #315` in the PR, so
  the link is found.
- `tb done` will not move a `gh#N` card to DONE while its issue is still open. Close the
  issue on GitHub (or merge the PR) instead of adding `--force`.

## JSON

Every command takes `--json`. Writes answer `{"ok":true,"card":{…}}`. Failures answer
`{"ok":false,"error":"…","hint":"…"}` and exit non-zero.

```sh
tb next --as NAME --json     # the card you got
tb show ID --json            # one card with checklist and notes
tb board --json              # the whole board
tb watch --json              # NDJSON: the board again on every change
tb agents --json             # herdr agents + the card each holds
```

Field names are stable (schema `"v":1`); see docs/JSON.md.

## Common errors

| error says | do this |
|---|---|
| `doing is full` | `tb done` or `tb drop` a card you hold, then retry |
| `no todo cards` | ask for work, or `tb add` what you found |
| `card #ID was taken by someone else` | run `tb next` again for another card |
| `issue #N still open on GitHub` | close the issue / merge the PR first |
| `you did this work — ask another person or agent to review it` | leave it in REVIEW for another agent |
| `no card #ID` | `tb list` to find the right ID |

## Brief line for orchestrators

Paste this into an agent's instructions:

> Your work is on Terminal Board: run `tb next --as <your-name>`, log each step with
> `tb note`, tick `tb check`, and `tb done` when finished (`tb drop` if you stop,
> `tb block` if stuck). Full manual: `tb guide`.

## Walkthrough (every command above, run in order by the test suite)

```sh
tb add "docs: write the install guide" -d "Done = guide merged" --check "draft" --check "review"
tb add "ops: rotate API tokens"
tb next --as alice
tb show 1
tb note 1 "draft written"
tb check 1 1
tb check 1 --add "add screenshots"
tb check 1 --rm 3
tb edit 1 --title "docs: install guide" --desc "Done = guide merged and linked"
tb block 1 "#2"
tb block 1 --clear
tb prio 2 top
tb done 1
tb next --review --as bob
tb done 1 --as bob
tb take 2
tb drop 2
tb move 2 doing
tb move 2 review
tb move 2 todo
tb list
tb board --json
tb show 1 --json
tb config
tb boards
tb rm 2
```
