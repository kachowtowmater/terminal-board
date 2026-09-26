//! Board kinds: ONE named bundle of settings, instead of thirteen knobs sold one at a time.
//!
//! Every setting a kind writes is settable on its own and always was. What a kind adds is a
//! NAME for a working combination — the thing that is tested, documented and asked about.
//! Thirteen independent switches are 2^13 boards no golden suite can cover, and a year from
//! now every report would open "with due-warn on and sort position and labels set…". A kind
//! gives the suite two boards to prove and a user one word to say.
//!
//! **The kind is RECORDED on the board** (`config kind`), for three reasons: a golden suite
//! needs a name for what it renders; `tb config` should answer "what kind of board is this?"
//! without the reader diffing thirteen keys; and a report can say `kind deadline` instead of
//! listing them. It is a LABEL, never a lock — nothing reads `kind` to decide behaviour, so
//! a board whose settings were changed afterwards keeps working exactly as its settings say.
//! `tb config` shows such a board as `deadline (changed)` so the name never lies, and
//! `tb config kind default` (or changing the settings back) clears the mark.
//!
//! `--from BOARD` copies another board's SETTINGS, never its cards: a board is its work, and
//! copying work silently would be a surprise nobody asked for.

use super::{Code, Result, Store};
use rusqlite::OptionalExtension;

/// What a board is for. `default` is exactly the board tb always made.
pub const KINDS: [&str; 2] = ["default", "deadline"];

/// The settings a kind writes, in the order `tb config` lists them. `default` writes none:
/// a board of the default kind is byte-identical to one made by any other means.
pub fn bundle(kind: &str) -> Vec<(&'static str, &'static str)> {
    match kind {
        // a board that tracks filing dates: nearest date first, the date on the card line,
        // a week of warning, blocked cards in their own lane and out of the work count
        "deadline" => vec![
            ("sort", "due"),
            ("card-line", "due"),
            ("due-warn", "7"),
            ("waiting-lane", "shown"),
            ("wip-counts-blocked", "no"),
            ("label.todo", "TO PREPARE"),
            ("label.doing", "IN HAND"),
            ("label.review", "WITH REVIEWER"),
            ("label.done", "FILED"),
        ],
        _ => Vec::new(),
    }
}

/// Settings that belong to ONE board and are never copied by `--from`, with the reason.
///
/// A new board must not start life wired to another board's repository, holding another
/// board's list of who may close or verify a card, whether only a verifier may close, or
/// claiming a file mode its own file does not have.
/// Everything else — the look, the ordering, the dates, the WIP limit, the kind — is what
/// somebody copies a board FOR.
pub const NOT_COPIED: [(&str, &str); 5] = [
    ("github", "a new board must not start syncing to another board's issues"),
    ("done-by", "who may close a card is a decision about this board's people"),
    ("verifiers", "who may verify a card is a decision about this board's people"),
    ("verifier-only", "whether only a verifier may close is a decision about this board's people"),
    ("file-mode", "the file's own permissions decide this, and they are set when it is created"),
];

pub fn copyable(key: &str) -> bool {
    !NOT_COPIED.iter().any(|(k, _)| *k == key) && key != crate::setup::SETUP_DONE
}

pub fn known(kind: &str) -> Result<&'static str> {
    let k = kind.trim().to_ascii_lowercase();
    KINDS.iter().copied().find(|v| *v == k).ok_or_else(|| {
        super::BoardError(format!(
            "unknown kind '{}' — tb knows {}: 'tb new board-name --kind deadline'",
            kind.trim(),
            KINDS.join(" and ")
        ), Code::InvalidValue)
    })
}

impl Store {
    /// The kind written on this board; `default` when none is (every older board).
    pub fn kind(&self) -> Result<&'static str> {
        let v: Option<String> =
            self.conn.query_row("SELECT value FROM config WHERE key='kind'", [], |r| r.get(0)).optional()?;
        Ok(v.and_then(|v| known(&v).ok()).unwrap_or("default"))
    }

    /// Does the board still hold every setting its kind writes?
    pub fn kind_intact(&self) -> Result<bool> {
        let kind = self.kind()?;
        for (key, value) in bundle(kind) {
            let have: Option<String> =
                self.conn.query_row("SELECT value FROM config WHERE key=?", [key], |r| r.get(0)).optional()?;
            if have.as_deref() != Some(value) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Write a kind's settings and record the kind. Existing settings the bundle does not
    /// name are left alone; a board that already holds cards keeps them.
    pub fn apply_kind(&self, kind: &str, actor: &str) -> Result<&'static str> {
        let kind = known(kind)?;
        for (key, value) in bundle(kind) {
            self.set_config(key, value)?;
        }
        if kind == "default" {
            // a default board is the board tb always made, down to its event log: declaring
            // the kind a board already is writes nothing at all
            let had = self.conn.execute("DELETE FROM config WHERE key='kind'", [])?;
            if had > 0 {
                Store::log_board(&self.conn, actor, "kind", "kind default")?;
            }
            return Ok(kind);
        }
        self.set_config("kind", kind)?;
        Store::log_board(&self.conn, actor, "kind", &format!("kind {kind}"))?;
        Ok(kind)
    }

    /// Copy another board's settings — never its cards. `kind` travels with them, so a copy
    /// of a deadline board is a deadline board.
    pub fn copy_settings_from(&self, other: &Store, actor: &str) -> Result<(usize, Vec<&'static str>)> {
        let mut st = other.conn.prepare("SELECT key, value FROM config ORDER BY key")?;
        let rows: Vec<(String, String)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut n = 0;
        let mut skipped = Vec::new();
        for (key, value) in &rows {
            if !copyable(key) {
                if let Some((k, _)) = NOT_COPIED.iter().find(|(k, _)| k == key) {
                    skipped.push(*k);
                }
                continue;
            }
            self.set_config(key, value)?;
            n += 1;
        }
        Store::log_board(&self.conn, actor, "kind", &format!("settings copied from '{}'", other.name))?;
        Ok((n, skipped))
    }

    /// `kind` for the `tb config` listing: shown once it is anything but the default, with
    /// `(changed)` when a setting it writes has since been changed, so the name never lies.
    pub fn kind_settings(&self) -> Result<Vec<(String, String)>> {
        let kind = self.kind()?;
        if kind == "default" {
            return Ok(Vec::new());
        }
        let mark = if self.kind_intact()? { "" } else { " (changed)" };
        Ok(vec![("kind".to_string(), format!("{kind}{mark}"))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("b.db")).unwrap();
        (dir, s)
    }

    #[test]
    fn the_default_kind_writes_nothing_at_all() {
        assert!(bundle("default").is_empty());
        let (_d, s) = store();
        let before = s.settings().unwrap();
        s.apply_kind("default", "alice").unwrap();
        assert_eq!(s.settings().unwrap(), before, "a default board is the board tb always made");
        assert_eq!(s.kind().unwrap(), "default");
        assert!(s.kind_settings().unwrap().is_empty(), "the default kind is not listed");
    }

    #[test]
    fn the_deadline_kind_writes_its_bundle_and_is_recorded() {
        let (_d, s) = store();
        s.apply_kind("deadline", "alice").unwrap();
        assert_eq!(s.kind().unwrap(), "deadline");
        assert!(s.kind_intact().unwrap());
        assert_eq!(s.kind_settings().unwrap(), [("kind".to_string(), "deadline".to_string())]);
        // every setting of the bundle is one this lane already shipped
        assert_eq!(s.sort().unwrap().as_str(), "due");
        assert_eq!(s.card_line().unwrap().as_str(), "due");
        assert_eq!(s.due_warn().unwrap(), 7);
        assert!(s.waiting_lane().unwrap());
        assert!(!s.wip_counts_blocked().unwrap());
        assert_eq!(s.label("review").unwrap().as_deref(), Some("WITH REVIEWER"));
        // and it says nothing about anything else
        assert_eq!(s.wip().unwrap(), super::super::DEFAULT_WIP);
        assert_eq!(s.theme().unwrap(), "dark");
        assert!(s.github_repo().unwrap().is_none());
    }

    #[test]
    fn a_kind_is_a_label_not_a_lock() {
        let (_d, s) = store();
        s.apply_kind("deadline", "alice").unwrap();
        // change one setting away from the bundle: the board keeps working, and says so
        s.set_sort("position").unwrap();
        assert_eq!(s.kind().unwrap(), "deadline", "the name it was made with stays");
        assert!(!s.kind_intact().unwrap());
        assert_eq!(s.kind_settings().unwrap(), [("kind".to_string(), "deadline (changed)".to_string())]);
        assert_eq!(s.sort().unwrap().as_str(), "position", "the setting is what decides, always");
        // putting it back clears the mark; so does declaring the board a default one
        s.set_sort("due").unwrap();
        assert!(s.kind_intact().unwrap());
        s.apply_kind("default", "alice").unwrap();
        assert_eq!(s.kind().unwrap(), "default");
        assert_eq!(s.sort().unwrap().as_str(), "due", "declaring a kind never undoes a setting");
    }

    #[test]
    fn settings_are_copied_from_another_board_and_cards_are_not() {
        let (_d1, from) = store();
        from.apply_kind("deadline", "alice").unwrap();
        from.set_wip(5).unwrap();
        from.add("permits: renewal", "", &[], "alice").unwrap();
        let (_d2, to) = store();
        to.add("its own card", "", &[], "bob").unwrap();
        let (n, skipped) = to.copy_settings_from(&from, "bob").unwrap();
        assert!(n >= bundle("deadline").len());
        assert!(skipped.is_empty(), "this board set none of the ones that never travel");
        assert_eq!(to.kind().unwrap(), "deadline", "a copy of a deadline board is one");
        assert_eq!(to.wip().unwrap(), 5);
        assert_eq!(to.card_line().unwrap().as_str(), "due");
        let titles: Vec<String> = to.list().unwrap().into_iter().map(|c| c.title).collect();
        assert_eq!(titles, ["its own card"], "cards are never copied");
    }

    #[test]
    fn an_unknown_kind_is_refused_with_the_command_to_run() {
        let e = known("law").unwrap_err().to_string();
        assert_eq!(e, "unknown kind 'law' — tb knows default and deadline: 'tb new board-name --kind deadline'");
        assert_eq!(known(" DEADLINE ").unwrap(), "deadline");
    }
}
