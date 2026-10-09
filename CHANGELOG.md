# Changelog

## 3.5.0 — 2026-10-09

### Hooks: a board may name an ordered LIST of pre-change hooks (tb#270, #259)

- **A board may name an ordered LIST of pre-change hooks:** `tb config hook a,b` (comma-
  separated, the same list syntax as `config actors`) runs `a` then `b` before every column
  change, each handed the same proposal; the FIRST refusal wins — later hooks are not run,
  the change is refused with that hook's own last line (`hook_refused`, nothing written).
  Each name keeps its own `tb trust` pin: a changed, untrusted or unknown member refuses
  naming THAT member and the other members keep their trust. A single name (`config hook
  solo`) is stored, printed and runs exactly as before; `--off` clears the whole list;
  `--force` skips no member; `--break-glass` skips the whole list and is logged as before.
  Replaces the fleet-side hook-chain wrapper (tb#270).

## 3.4.3 — 2026-10-05

### Tests: one race-free agent-shell helper in tests/common (tb#286)

The new agent-shell helper in `tests/common` (`agent_shell`) copies the shell ONCE per
test process — a temp file is written, closed, chmodded and renamed, so the exec'd path
never has an open writer — and `tests/tb_reap.rs` wraps its spawns in `retry_exec_busy`,
which retries the exec on `ETXTBSY` (`Text file busy`) with backoff. Linux no longer
loses ~1 in 40 runs to the copy race. Test-only; no shipped behaviour change.

### README: send-back wording under a live verifier claim (tb#281)

The README's 'New in 3.4.1' paragraph now says a send-back is refused `claimed_by_other`
only when ANOTHER verifier holds a live claim — not on every send-back.

## 3.4.2 — 2026-10-05

### Identity guidance: never `git config user.*` from a linked worktree (tb#282)

AGENTS.md and `scripts/commit-identity-check.sh` now say agent identity comes from the
launcher's `GIT_AUTHOR_*`/`GIT_COMMITTER_*` env or a per-commit
`git -c user.email=… user.name=…` — never `git config user.*`, which in a linked worktree
writes the repo-wide `.git/config` shared with every other worktree on the machine.

### Liveness: the `TB_REAP_FAKE_PANES` fixture var is removed (tb#262)

It switched tb-reap's fixture mode on, but since tb#256 deleted probe 4 (pane label) its
value was ignored: no fixture read it. Scripts that set it to switch fixture mode must set
another `TB_REAP_FAKE_*` var (`TB_REAP_FAKE_{AGENTS,TMUX,SESSIONS,PROCS}`). Dropped from
the FIXTURE var list, module doc and every test fixture env.

## 3.4.1 — 2026-10-05

### `tb-reap --apply` refuses to run inside an agent process (tb#232)

`tb-reap --apply` executed from inside an agent's own process (omp / claude / codex / pi in
the kernel's parent chain — the harness a herdr worker or a cron-launched agent runs under)
is refused with exit 1 before any probing: the #227 rule never counts tb's own process tree,
so the holder that launched the reaper is invisible to the probe and a live agent's DOING
card would be released as dead. A plain-shell `--apply` is unchanged and still releases a
genuinely dead holder, and `tb release` keeps the #227 rule (it is an explicit person/
lead decision, not a blind reaper).

### Liveness: a headless pid no longer vouches once it is stale or reused (tb#243, tb#272)

`tb release`/`tb-reap` probe 7 (`mode:headless` with `pid=N`) no longer keeps a holder alive
by a stale or recycled pid. When the note's `log:<path>` file exists and ends with the
`[tb-agent-start] … exit=<N> <holder>` trailer the launcher writes on agent exit, the pid
does not vouch. When the pid is still in the process table, it vouches only if that
process's command line names an agent binary (codex / omp / claude / pi / aider / opencode
/ gemini) or the holder's name as a whole token — a reused pid (e.g. the #243 repro: an
audio SandboxHelper took pid 73509) no longer vouches; a live wrapper with the holder's
name and omp still does. The process probe (4) is also tightened: it vouches only when the
process names the holder as its identity — an env token `TB_AS=<holder>` or the argv pair
`--as <holder>` — never because the name appears as some other token such as
`TB_ROLE=<holder>` or prompt text (default#993).

- **The claim lock covers every move out of REVIEW, and each refusal carries its own
  path (tb#236):** a verifier's send-back (`tb move ID doing "…"`, `tb move ID todo "…"`)
  of a card whose live `reviewer` claim it does not hold is refused `claimed_by_other`
  with the send-back wording (the claimant's own send-back paths are named), an agent
  closing an UNCLAIMED review card is refused `close_needs_claim` until it claims, and
  `tb claim ID` takes a free or STALE claim (a live claim held by another verifier is
  refused; a stale one hands off with an `unclaimed` event). The close-form refusal
  (`"… closes it, or a person frees the claim"`) now scopes to closes only, so a
  send-back refusal is never worded as a close.

## 3.4.0 — 2026-10-02

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
