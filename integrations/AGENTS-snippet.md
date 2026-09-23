## Terminal Board

Work is tracked on Terminal Board (`tb`), a task board shared by people and agents:
TODO → DOING → REVIEW → DONE. Use the `tb` CLI (never the full-screen board). Always act
under the same name: `--as <your-name>` or `TB_AS=<your-name>`.

| to | run |
|---|---|
| take the top TODO card | `tb next --as <your-name>` |
| take a specific card | `tb take ID` |
| see the board / one card | `tb list` · `tb show ID` |
| log progress (people read this) | `tb note ID "what changed"` |
| tick checklist item N | `tb check ID N` |
| add / remove a checklist item | `tb check ID --add "text"` · `tb check ID --rm N` |
| change title / description | `tb edit ID --title "tag: title"` · `tb edit ID --desc "Done = …"` |
| stuck / unstuck | `tb block ID "#N or reason"` · `tb block ID --clear` |
| finished (DOING → REVIEW); a verifier: REVIEW → DONE on someone else's card | `tb done ID` |
| stop and hand it back (→ TODO) | `tb drop ID` |
| move to any column (`done` only from REVIEW) | `tb move ID todo\|doing\|review\|done` |
| send a REVIEW card back | `tb move ID doing "what to fix"` |
| reorder | `tb prio ID top\|bottom\|up\|down` |
| file new work | `tb add "tag: title" -d "Done = …" --check "step"` |
| machine-readable | add `--json` to any command |

Rules: one card at a time; `doing is full` = finish or drop first; a note before you stop,
drop or block; tick only what is done; no `--force` unless a person says so. A card someone
else holds in DOING is theirs — `move`, `done`, `drop`, `edit`, `block`, `rm`, `check` and
`prio` are refused (`--force` overrides, logged); `tb note` stays open to everyone. Every
error says what to run next. Full manual: `tb guide`.

Who moves a card: TODO — any agent files work. DOING — the workers (one session or many).
REVIEW → DONE — only an independent verifier: an agent started with `TB_ROLE=verifier`, a
name on `tb config verifiers`, or a person; never whoever did the work. Nothing reaches DONE
except from REVIEW (GitHub sync included), and every move into DONE records who made it
(name, harness, model, role, session). Any other agent is refused (`not_verifier`): leave
the card in REVIEW.
