# Changelog

## Unreleased

### Highlights

- **A board may refuse agents not on its reader list (tb#263).** `tb config readers
  NAME,NAME` (person-only; `off` clears it) stores a reader list on the board. When it is
  set, every command that opens the board — read or write, by name or `TB_DB` — is refused
  (`not_a_reader`) to an agent not acting under a listed name; an identity-less agent, and a
  scrubbed environment inside an agent's process (the kernel's ancestry still names the
  agent), are refused as well. Persons pass, boards without a list are unchanged, and a
  `tb boards` listing simply omits the board for an off-list caller. Reader names are
  self-asserted (an agent can still forge `TB_AS`), so this stops the honest mistake, not a
  determined agent — the real fix is privilege separation.

## 3.3.2 — 2026-10-02

### Highlights

- **A herdr pane label no longer keeps a holder alive (tb#256).** `tb release` / `tb-reap`
  no longer treat a pane LABEL naming the holder as liveness: a label is display text
  anyone can set, so a stopped worker's leftover pane (`b-1069 · … · default#1069`) kept
  its card held. Liveness comes only from a herdr agent name, a tmux session, a live agent
  process, an actor session or a headless pid. `tb-reap`'s liveness version is now 3
  (rules changed; old dry-run lines do not count toward the `--apply` proof).
  `TB_REAP_FAKE_PANES` still switches fixture mode on but is ignored; a follow-up card
  (tb#262) removes it.

## 3.3.1 — 2026-10-02
