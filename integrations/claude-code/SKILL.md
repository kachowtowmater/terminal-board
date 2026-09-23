---
name: terminal-board
description: Use Terminal Board (`tb`), the shared terminal task board for people and agents. Use when the user mentions "terminal board", "tb", "task board", "todo board", "the board", "take the next card", "what should I work on", or asks you to pick up, track, note, update, move or finish work on the board.
---

# Terminal Board

Work is handed out on Terminal Board, a shared task board (TODO → DOING → REVIEW → DONE).
Use the `tb` CLI; never open the full-screen board (bare `tb` without a terminal just prints
the board). Act under one name every time: `--as <name>` or `TB_AS=<name>`.

Core loop:

```sh
tb next --as <name>           # take the top TODO card
tb show ID                    # read the brief and the done criteria
tb note ID "progress"         # log each step; people read these
tb check ID N                 # tick checklist items honestly
tb done ID                    # DOING -> REVIEW when finished
```

Update and move cards:

```sh
tb edit ID --title "tag: title" --desc "Done = …"   # rewrite a card
tb check ID --add "step"      # add a checklist item (--rm N deletes one)
tb block ID "#N"              # stuck (tb block ID --clear when it moves)
tb drop ID                    # hand it back to TODO if you stop
tb move ID review             # any column: todo | doing | review | done
tb move ID doing "why"        # reviewer: send a REVIEW card back to its owner
tb prio ID top                # reorder: top | bottom | up | down
tb add "tag: title" -d "Done = …" --check "step"   # file follow-up work
```

Rules: one card at a time. `doing is full` means finish or drop a card first. Leave a note
before you stop, drop or block. Do not use `--force` unless the user asks. A card someone
else holds in DOING is theirs — `move`, `done`, `drop`, `edit`, `block`, `rm`, `check` and
`prio` on it are refused (`--force` overrides, logged); `tb note` stays open to everyone.
Errors always say what to run next. Add `--json` to any command for machine-readable output.

Run `tb guide` for the full agent manual (every command, recipes, GitHub, JSON).
