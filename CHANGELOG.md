## Unreleased

- **PR commits stay no-reply:** on every pull request, CI now runs
  `scripts/commit-identity-check.sh` over the PR's own commits (`base..head`) and fails
  when an author or committer uses an address that is not a GitHub no-reply address.
  The check is documented in README.md and AGENTS.md.

### `tb rm` on open work is a person's or a registered verifier's

- `tb rm ID` on a card whose column is not `done` is refused for an agent (a harness in its
  identity) that is not a REGISTERED verifier — a builder, a lead/orchestrator, or a forged
  `TB_ROLE=verifier` with no registry entry (the #169 rule). It is refused with and without
  `--force`, with the JSON code `rm_verifier_only`; the error points at the way out: move the
  card to REVIEW with a CLOSE note (`tb note ID "CLOSE: why"` then `tb move ID review`). A
  person keeps rm everywhere, and an agent may still rm a DONE card (#225, seen live:
  lead-codemap `rm`'d a REVIEW duplicate card the orchestrator then had to restore).
- The refusal is logged in its own write (a refused rm rolls its transaction back), so
  `tb log` and `tb show` carry the row: `refused #ID "title" (code): why` under the `rm`
  kind, on the board log.

## 3.2.4 — 2026-09-27
