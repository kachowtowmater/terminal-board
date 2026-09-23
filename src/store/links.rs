//! Evidence links on a card (A10) and the DONE guard that requires one (B3, `config
//! done-needs-link`).
//!
//! **Storage: a table, not a column.** A card carries zero or more links (a brief, a verdict,
//! a commit sha, and whatever else an agent decides is evidence), so this is the same shape
//! `checklist` already is — one row per item, ordered by `idx`, addressable for removal — not
//! a single growing blob column. That keeps `docs/SCHEMA.md` a plain column list instead of a
//! schema-inside-a-string, and it is what makes `tb link ID --rm N` possible at all.
//!
//! **The label is free text, like a tag.** The client named three (`brief`, `verdict`,
//! `commit`), but a fixed set would mean a tb release every time someone wants to attach a
//! `screenshot` or a `log`. `clean_label` gives it the same small, safe alphabet `clean_tag`
//! does, lower-cased so `--label Verdict` and `config done-needs-link verdict` agree without
//! surprise. The trade tb already makes for `done-by` applies again: `done-needs-link` names a
//! label nobody validates against a registry, so a typo is a silent miss — the same
//! self-asserted, honest-mistake shape as every other name in tb, not a new kind of risk.
//!
//! **A link is untrusted text tb only stores.** Like a note or a description, the VALUE is
//! kept raw (trimmed, length-bounded) — not stripped at write time — because it already goes
//! through the same display sanitiser every other stored text does: `line()` on the screen,
//! `clean_json` in `--json`, `sanitize_buffer` in the popup (docs/JSON.md). `clean_value` does
//! not parse or judge it as a path, a sha or a URL either — tb never reads a file at that
//! path, never resolves the sha against a repo and never fetches the URL. There is no code
//! path from a link's text to the network or the filesystem; it is a string tb carries and
//! shows, exactly like `blocked_on`, and a `--json` consumer that runs it through a shell or a
//! browser is trusting external text on its own — tb's job stops at storing and displaying it
//! safely, the same boundary every other free-text field already has.

use super::{Code, err, now, Connection, Result, Store};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

/// Longest label, in characters — the same budget `TAG_MAX` gives a tag.
pub const LABEL_MAX: usize = 32;
/// Longest link value, in characters — generous for a long URL or an absolute path.
pub const VALUE_MAX: usize = 2000;

/// `--label LABEL` / `config done-needs-link LABEL` cleaned: lower-cased, whitespace
/// collapsed, the same alphabet `clean_tag` accepts (letters, digits, spaces, hyphens,
/// underscores).
pub fn clean_label(raw: &str) -> Result<String> {
    let label = crate::text::sanitize(raw).split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    if label.is_empty() {
        return err("the label is empty — try '--label brief' (or 'verdict', 'commit', or any word that names the evidence)".to_string(), Code::ArgRequired);
    }
    let n = label.chars().count();
    if n > LABEL_MAX {
        return err(format!("that label is {n} characters, the limit is {LABEL_MAX} — shorten it"), Code::InvalidValue);
    }
    if let Some(bad) = label.chars().find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == ' ')) {
        return err(format!(
            "a label holds letters, digits, spaces, hyphens and underscores — '{bad}' is none of those"
        ), Code::InvalidValue);
    }
    Ok(label)
}

/// The link value: trimmed and length-bounded, otherwise kept RAW — like a note or a
/// description, not sanitised at write time, because the display sanitiser already covers it
/// everywhere it is shown (see the module docs). Never parsed, never checked against the
/// filesystem or the network. Internal spacing is kept (a path may hold one).
pub fn clean_value(raw: &str) -> Result<String> {
    let value = raw.trim().to_string();
    if value.is_empty() {
        return err("the link is empty — try 'tb link ID PATH|SHA|URL --label brief'".to_string(), Code::ArgRequired);
    }
    let n = value.chars().count();
    if n > VALUE_MAX {
        return err(format!("that link is {n} characters, the limit is {VALUE_MAX}"), Code::InvalidValue);
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LinkItem {
    pub idx: i64,
    pub label: String,
    pub value: String,
    pub added_by: String,
    pub added_at: i64,
}

pub(super) fn row_link(r: &rusqlite::Row) -> rusqlite::Result<LinkItem> {
    Ok(LinkItem { idx: r.get(0)?, label: r.get(1)?, value: r.get(2)?, added_by: r.get(3)?, added_at: r.get(4)? })
}

/// The `done-needs-link` label this board requires before a card reaches DONE; None = off
/// (today's behaviour).
pub(super) fn required_label(conn: &Connection) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT value FROM config WHERE key='done-needs-link'", [], |r| r.get(0)).optional()?)
}

/// Does card `id` already carry a link with `label` (case-insensitive, as every stored label
/// already is)?
pub(super) fn has_label(conn: &Connection, id: i64, label: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM links WHERE card_id=? AND label=? COLLATE NOCASE",
        params![id, label],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// The refusal when a card has no link `label` yet: what is missing and the one command that
/// fixes it, same shape as `closing::not_allowed`.
pub(super) fn missing_link_err(id: i64, label: &str) -> super::BoardError {
    super::BoardError(format!(
        "#{id} has no link labeled '{label}' — attach one first: 'tb link {id} PATH|SHA|URL --label {label}', or 'tb done {id} --force' if you mean it (logged)"
    ), Code::DoneNeedsLink)
}

impl Store {
    /// `tb link ID VALUE --label LABEL`: attach a link (a path, a sha or a URL) as evidence.
    /// A card may carry any number of links, including several with the same label. Returns
    /// the item as stored, numbered after whatever the card already has.
    pub fn add_link(&self, id: i64, value: &str, label: &str, actor: &str) -> Result<LinkItem> {
        self.card(id)?;
        let value = clean_value(value)?;
        let label = clean_label(label)?;
        // reads (the next idx) then writes: needs `BEGIN IMMEDIATE`, same reason as `add` (#85).
        // `&self`, so `unchecked_transaction` + the ROLLBACK/BEGIN IMMEDIATE trick
        // (`transaction_with_behavior` needs `&mut Connection`) — see `store.rs::add_tagged`.
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("ROLLBACK; BEGIN IMMEDIATE")?;
        let n: i64 = tx.query_row("SELECT COALESCE(MAX(idx), 0) + 1 FROM links WHERE card_id=?", [id], |r| r.get(0))?;
        let ts = now();
        tx.execute(
            "INSERT INTO links(card_id, idx, label, value, added_by, added_at) VALUES (?,?,?,?,?,?)",
            params![id, n, label, value, actor, ts],
        )?;
        Store::log(&tx, id, actor, "link", &format!("+ {label}: {value}"))?;
        tx.commit()?;
        Ok(LinkItem { idx: n, label, value, added_by: actor.to_string(), added_at: ts })
    }

    /// Delete link `n` and renumber the ones after it (`checklist`'s own pattern).
    pub fn remove_link(&self, id: i64, n: i64, actor: &str) -> Result<()> {
        let links = self.links_of(id)?;
        let Some(item) = links.iter().find(|l| l.idx == n) else {
            return err(format!("card #{id} has no link {n} (it has {}) — see 'tb show {id}'", links.len()), Code::Unknown);
        };
        // a write transaction from the start, consistent with every other write path (#85)
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("ROLLBACK; BEGIN IMMEDIATE")?;
        tx.execute("DELETE FROM links WHERE card_id=? AND idx=?", params![id, n])?;
        // two steps so the (card_id, idx) key never collides mid-update
        tx.execute("UPDATE links SET idx = -(idx - 1) WHERE card_id=? AND idx>?", params![id, n])?;
        tx.execute("UPDATE links SET idx = -idx WHERE card_id=? AND idx<0", params![id])?;
        Store::log(&tx, id, actor, "link", &format!("- {}: {}", item.label, item.value))?;
        tx.commit()?;
        Ok(())
    }

    /// Every link on card `id`, in the order they were added.
    pub fn links_of(&self, id: i64) -> Result<Vec<LinkItem>> {
        let mut st = self.conn.prepare("SELECT idx, label, value, added_by, added_at FROM links WHERE card_id=? ORDER BY idx")?;
        let v = st.query_map([id], row_link)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// `done-needs-link` on this board: the label a card must carry a link for before it may
    /// reach DONE; None when the board sets nothing (today's behaviour).
    pub fn done_needs_link(&self) -> Result<Option<String>> {
        required_label(&self.conn)
    }

    /// `config done-needs-link LABEL` sets it; `None` (`--off`) clears it. Returns the label
    /// now in force.
    pub fn set_done_needs_link(&self, label: Option<&str>) -> Result<Option<String>> {
        let Some(label) = label else {
            self.conn.execute("DELETE FROM config WHERE key='done-needs-link'", [])?;
            return Ok(None);
        };
        let label = clean_label(label)?;
        self.set_config("done-needs-link", &label)?;
        Ok(Some(label))
    }

    /// The `done-needs-link` row of `tb config`: listed once the board sets it, so a board
    /// that sets nothing lists exactly what it always did.
    pub fn link_settings(&self) -> Result<Vec<(String, String)>> {
        Ok(match self.done_needs_link()? {
            Some(label) => vec![("done-needs-link".to_string(), label)],
            None => Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_is_cleaned_lowercased_and_bounded() {
        for (raw, want) in [("Brief", "brief"), ("  VERDICT  ", "verdict"), ("code-review 2", "code-review 2")] {
            assert_eq!(clean_label(raw).unwrap(), want);
        }
        let e = clean_label("").unwrap_err().to_string();
        assert!(e.starts_with("the label is empty"), "{e}");
        let e = clean_label(&"x".repeat(33)).unwrap_err().to_string();
        assert!(e.contains("the limit is 32"), "{e}");
        for bad in ["a:b", "a/b", "a#b"] {
            let e = clean_label(bad).unwrap_err().to_string();
            assert!(e.contains("letters, digits, spaces, hyphens and underscores"), "{bad}: {e}");
        }
        assert_eq!(clean_label("a\x1b[31mb").unwrap(), "ab", "control characters are removed");
    }

    #[test]
    fn a_value_is_trimmed_and_bounded_but_kept_raw_like_a_note() {
        assert_eq!(clean_value("  /srv/docs/My Documents/brief.txt  ").unwrap(), "/srv/docs/My Documents/brief.txt");
        // NOT stripped here — the store keeps text raw; `tests/links.rs` proves the display
        // sanitiser still removes this from `tb show` and `tb show --json`
        assert_eq!(clean_value("a\x1b[31mb").unwrap(), "a\x1b[31mb");
        let e = clean_value("   ").unwrap_err().to_string();
        assert!(e.starts_with("the link is empty"), "{e}");
        let e = clean_value(&"x".repeat(2001)).unwrap_err().to_string();
        assert!(e.contains("the limit is 2000"), "{e}");
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("b.db")).unwrap();
        (dir, s)
    }

    #[test]
    fn links_are_added_listed_and_removed_with_renumbering() {
        let (_d, s) = store();
        let id = s.add("widgets: fix it", "", &[], "alice").unwrap();
        let a = s.add_link(id, "docs/brief.md", "brief", "alice").unwrap();
        let b = s.add_link(id, " https://ci.example/run/9 ", "verdict", "bob").unwrap();
        assert_eq!((a.idx, a.label.as_str(), a.value.as_str(), a.added_by.as_str()), (1, "brief", "docs/brief.md", "alice"));
        assert_eq!((b.idx, b.label.as_str()), (2, "verdict"));
        let links = s.links_of(id).unwrap();
        assert_eq!(links.len(), 2);
        s.remove_link(id, 1, "alice").unwrap();
        let links = s.links_of(id).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!((links[0].idx, links[0].label.as_str()), (1, "verdict"), "the survivor renumbers to 1");
        let e = s.remove_link(id, 9, "alice").unwrap_err().to_string();
        assert!(e.contains("has no link 9"), "{e}");
    }

    #[test]
    fn a_card_may_carry_more_than_one_link_with_the_same_label() {
        let (_d, s) = store();
        let id = s.add("widgets: fix it", "", &[], "alice").unwrap();
        s.add_link(id, "one sha", "commit", "alice").unwrap();
        s.add_link(id, "two sha", "commit", "alice").unwrap();
        assert_eq!(s.links_of(id).unwrap().len(), 2);
        assert!(has_label(&s.conn, id, "COMMIT").unwrap(), "label match is case-insensitive");
    }

    #[test]
    fn done_needs_link_is_off_until_set_and_clears_with_off() {
        let (_d, s) = store();
        assert_eq!(s.done_needs_link().unwrap(), None);
        assert!(s.link_settings().unwrap().is_empty());
        assert_eq!(s.set_done_needs_link(Some("Verdict")).unwrap().as_deref(), Some("verdict"));
        assert_eq!(s.done_needs_link().unwrap().as_deref(), Some("verdict"));
        assert_eq!(s.link_settings().unwrap(), [("done-needs-link".to_string(), "verdict".to_string())]);
        assert_eq!(s.set_done_needs_link(None).unwrap(), None);
        assert_eq!(s.done_needs_link().unwrap(), None);
    }

    /// The `Store::transition` guard end to end: off by default, refuses DONE without a
    /// matching link once set (case-insensitive), passes once one is attached, `--force`
    /// (`move_to_forced`-equivalent via the WIP-free `done` path is exercised in tests/links.rs
    /// through the CLI) and the `github` exemption — the same shape `done-by` already has.
    #[test]
    fn done_needs_link_blocks_done_until_a_matching_link_is_attached() {
        let (_d, mut s) = store();
        let id = s.add("widgets: fix it", "", &[], "bob").unwrap();
        s.take(id, "bob").unwrap();
        s.done(id, "bob").unwrap(); // -> review
        // off by default: today's behaviour, unaffected
        s.move_to(id, "done", "carol").unwrap();

        let id2 = s.add("widgets: fix it too", "", &[], "bob").unwrap();
        s.take(id2, "bob").unwrap();
        s.done(id2, "bob").unwrap(); // -> review
        s.set_done_needs_link(Some("verdict")).unwrap();
        let e = s.move_to(id2, "done", "carol").unwrap_err().to_string();
        assert!(e.contains("has no link labeled 'verdict'") && e.contains(&format!("tb link {id2}")), "{e}");
        assert_eq!(s.card(id2).unwrap().column, "review", "refused: the card did not move");
        // a link under a DIFFERENT label still refuses it
        s.add_link(id2, "docs/brief.md", "brief", "bob").unwrap();
        assert!(s.move_to(id2, "done", "carol").is_err());
        // the matching label, any case, lets it through
        s.add_link(id2, "https://ci.example/run/1", "VERDICT", "bob").unwrap();
        s.move_to(id2, "done", "carol").unwrap();
        assert_eq!(s.card(id2).unwrap().column, "done");
    }

    /// The sync moves a card on evidence (a merged PR), not as a person closing it — the same
    /// exemption `done-by` makes, even with no link attached at all.
    #[test]
    fn the_github_sync_is_exempt_from_done_needs_link() {
        let (_d, mut s) = store();
        s.set_done_needs_link(Some("verdict")).unwrap();
        let id = s.add("widgets: gh#7 fix it", "", &[], "bob").unwrap();
        s.take(id, "bob").unwrap();
        s.done(id, "bob").unwrap(); // -> review
        s.move_to(id, "done", "github").unwrap();
        assert_eq!(s.card(id).unwrap().column, "done");
    }
}
