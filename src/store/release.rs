//! `tb release ID "why"`: free a DOING card whose holder is DEAD, without `--force`.
//!
//! A worker that dies mid-card leaves its card in DOING under a name nobody answers to. The
//! holder rule (only the owner leaves DOING, or `--force`) then keeps everyone else off it,
//! and the only ways out used to be a forced move — which says nothing about whether the
//! holder was actually gone — or relaunching the dead worker under its old name just to drop.
//! `release` is the checked way: it moves the card to TODO, unowned, ONLY when
//!
//! 1. the card exists and is in DOING (`no_card` / `not_in_doing`);
//! 2. a reason is given — it is logged on the card (`reason_required`);
//! 3. the actor may release: a person (no agent harness in its identity) or an agent whose
//!    `TB_ROLE` is `lead` or `orchestrator` — the seats that run workers (`not_releaser`,
//!    [`may_release`]);
//! 4. the holder is dead by the SAME probes `tb-reap` uses (`crate::liveness`: herdr agent,
//!    tmux session, pane label, agent process, live actor session, headless pid) — except a
//!    no-pid `mode:headless` note, which keeps the holder alive only for `tb-reap`'s
//!    automatic scan; a hand release with a reason does not trust it alone. While any of
//!    them vouches for the holder, it is refused (`holder_alive`), naming which one
//!    ([`holder_alive`]). `TB_REAP_FAKE_*` put the probes in fixture mode, as for tb-reap.
//!
//! There is no `--force` here on purpose: a live holder's card is taken with `tb move ID todo
//! --force`, which is logged as exactly that. The release itself goes through
//! `Store::transition` (so a board's pre/post-change hooks see it) and logs one `released`
//! event: `released <holder> (<liveness evidence>): <reason>`.
//!
//! Like every name in tb, a role is self-reported: this stops the honest mistake — a worker
//! freeing a sibling's card — not someone set on getting past it.

use super::actors::{self, Identity};
use super::verifier::is_agent;
use super::{err, BoardError, Card, Code, Result, Store};
use crate::liveness::World;

/// The agent roles that may release a dead holder's card (compared ignoring case).
pub const ROLES: [&str; 2] = ["lead", "orchestrator"];

/// May this identity release a card? A person always; an agent only with a releasing role.
pub fn may_release(who: &Identity) -> bool {
    !is_agent(who) || who.role.as_deref().is_some_and(|r| ROLES.iter().any(|v| v.eq_ignore_ascii_case(r.trim())))
}

/// Why `card`'s holder counts as alive (the probe that vouches for it), or None when it is
/// dead. An explicit `tb release` asks [`World::alive_for_release`]: a no-pid
/// `mode:headless` note alone does NOT vouch for the holder.
pub fn holder_alive(store: &Store, card: &Card) -> Option<String> {
    World::load().alive_for_release(store, card)
}

fn not_releaser_err(id: i64, actor: &str, who: &Identity) -> BoardError {
    let harness = who.harness.as_deref().unwrap_or("an agent");
    let role = match who.role.as_deref() {
        Some(r) => format!("role {r}"),
        None => "no role".to_string(),
    };
    BoardError(
        format!(
            "only a lead, an orchestrator or a person releases #{id} — {actor} is {harness} with {role}. Tell whoever runs this board that its holder looks dead"
        ),
        Code::NotReleaser,
    )
}

impl Store {
    /// `tb release ID "why"` — see the module doc for the checks, in their order.
    pub fn release(&mut self, id: i64, reason: &str, actor: &str) -> Result<Card> {
        let c = self.card(id)?;
        let Some(holder) = c.owner.clone().filter(|_| c.column == "doing") else {
            return err(
                format!("#{id} is in {} — only a DOING card with a holder is released; 'tb drop {id}' or 'tb move {id} todo' for anything else", c.column),
                Code::NotInDoing,
            );
        };
        let reason = reason.trim();
        if reason.is_empty() {
            return err(format!("say why — 'tb release {id} \"its pane is gone\"'"), Code::ReasonRequired);
        }
        let who = actors::current();
        if !may_release(&who) {
            return Err(not_releaser_err(id, actor, &who));
        }
        if let Some(how) = holder_alive(self, &c) {
            return err(
                format!(
                    "#{id}'s holder {holder} is alive ({how}) — ask it to 'tb drop {id}', or wait; a live holder's card is only taken with 'tb move {id} todo --force' (logged)"
                ),
                Code::HolderAlive,
            );
        }
        let evidence = if crate::liveness::fixture_mode() {
            "dead: no probe vouches for it (fixture)"
        } else {
            "dead: no herdr agent, tmux session, pane label, agent process, live session or headless pid"
        };
        let text = format!("released {holder} ({evidence}): {reason}");
        self.release_card(id, &holder, &text, actor)
    }
}
