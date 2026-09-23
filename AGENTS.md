# AGENTS.md

Two kinds of agents read this file.

## Agents USING Terminal Board to track work

The full manual is [docs/AGENTS.md](docs/AGENTS.md) (also printed by `tb guide`). In short:

```sh
tb next --as <your-name>      # take the top TODO card
tb show ID                    # read the brief
tb note ID "what changed"     # log each step
tb check ID N                 # tick checklist item N
tb edit ID --desc "Done = …"  # update the card
tb move ID review             # move it (todo | doing | review | done)
tb done ID                    # finished: DOING -> REVIEW
tb drop ID                    # stopping: back to TODO
tb block ID "#N"              # stuck
```

Only an independent verifier (`TB_ROLE=verifier`, a name a person put on `tb config verifiers`, or a person)
moves a card from REVIEW to DONE, and nothing reaches DONE any other way — see "Who moves a card"
in the manual.

To give every agent in YOUR project these instructions, run `tb setup --agents-md AGENTS.md`
there (it adds a marked block you can re-run safely), or paste
[integrations/AGENTS-snippet.md](integrations/AGENTS-snippet.md).

## Agents WORKING ON this repository

- Rust 2021, one binary `tb` (`src/main.rs`), library in `src/` (`store.rs` = SQLite,
  `tui.rs` + `tui/layouts.rs` = the board, `setup.rs` = the wizard, `github.rs`, `herdr.rs`).
- Before you finish: `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `shellcheck install.sh tests/install_test.sh scripts/*.sh`,
  `cargo build && tests/install_test.sh target/debug/tb`, `scripts/privacy-check.sh`.
- Layout changes: `tests/layout.rs` renders every view at fixed sizes; the half-h golden is
  `tests/golden/half_h_126x41.txt` (refresh with `TB_UPDATE_GOLDEN=1` only on purpose).
- Every `tb` line in README.md and the walkthrough in docs/AGENTS.md is executed by
  `tests/readme.rs`, so keep examples runnable.
- JSON output is a contract (docs/JSON.md, schema `"v":1`): add fields, never rename.
