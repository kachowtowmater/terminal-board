//! Every `tb …` line in the README's code blocks runs, in order, against temp boards.
//! A code block right after `<!-- no-test -->` is skipped (GitHub, installer, watch).
/// A fake `gh` that answers `repo view` positively (for `config github`'s existence
/// check) and nothing else; shared by tests that pin `TB_GH` to a nonexistent path.
fn fake_gh_ok() -> std::path::PathBuf {
    use std::sync::OnceLock;
    static GH: OnceLock<std::path::PathBuf> = OnceLock::new();
    GH.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("tb-fake-gh-{}", std::process::id()));
        std::fs::write(&p, "#!/bin/sh\ncase \"$1 $2\" in\n  \"repo view\") echo '{\"nameWithOwner\":\"acme/widgets\"}';;\n  *) exit 0;;\nesac\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    })
    .clone()
}
use std::process::Command;

#[test]
fn readme_examples_run() {
    // boards mode (a temp HOME, no TB_DB): the README uses board names (`tb home add …`),
    // which a pinned TB_DB file refuses
    let ran = run_doc_mode(include_str!("../README.md"), "README", true);
    assert!(ran >= 35, "only {ran} README commands found");
}

/// The agent manual's walkthrough: every command it teaches, run in order.
#[test]
fn agent_manual_walkthrough_runs() {
    let md = include_str!("../docs/AGENTS.md");
    let walk = &md[md.find("## Walkthrough").expect("walkthrough section")..];
    let ran = run_doc(walk, "docs/AGENTS.md");
    assert!(ran >= 20, "only {ran} walkthrough commands found");
    // every command the manual names in its tables is one the CLI has
    let help = std::process::Command::new(env!("CARGO_BIN_EXE_tb")).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&help.stdout).to_string();
    for word in ["add", "list", "show", "next", "take", "note", "check", "move", "done", "block", "drop", "rm", "prio", "edit", "config", "boards", "board", "watch", "agents", "sync", "guide", "github"] {
        assert!(md.contains(&format!("tb {word}")), "manual never shows tb {word}");
        assert!(help.contains(word), "help lacks {word}");
    }
}

/// Run every `tb …` line in `md`'s code blocks, in order, against one temp board.
fn run_doc(md: &str, name: &str) -> usize {
    run_doc_mode(md, name, false)
}

/// `boards_mode`: run with a temp HOME (boards-dir mode) instead of a pinned `TB_DB` —
/// for docs whose examples use board names (the README's Boards section).
fn run_doc_mode(md: &str, name: &str, boards_mode: bool) -> usize {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("readme.db");
    let bin = env!("CARGO_BIN_EXE_tb");
    let (mut in_block, mut skip_next, mut skip_block) = (false, false, false);
    let mut ran = 0;
    for line in md.lines() {
        let t = line.trim();
        if t == "<!-- no-test -->" {
            skip_next = true;
            continue;
        }
        if t.starts_with("```") {
            if in_block {
                in_block = false;
            } else {
                in_block = true;
                skip_block = skip_next;
                skip_next = false;
            }
            continue;
        }
        if !in_block || skip_block {
            continue;
        }
        let cmd = t.strip_prefix("$ ").unwrap_or(t);
        if !(cmd == "tb" || cmd.starts_with("tb ")) {
            continue;
        }
        let rest = cmd.strip_prefix("tb").unwrap();
        let mut c = Command::new("sh");
        c.arg("-c").arg(format!("\"$TB_BIN\"{rest}"));
        if boards_mode {
            // boards-dir mode: every board is its own file under this temp HOME
            c.env("HOME", dir.path()).env_remove("TB_DB").env_remove("TTYBOARD_DB").env_remove("XDG_STATE_HOME");
        } else {
            c.env("TB_DB", &db);
        }
        let o = c
            .env("TB_BIN", bin)
            .env("TB_AS", "alice")
            .env("TB_NO_HERDR", "1")
            .env("TB_GH", fake_gh_ok())
            .env_remove("TB_BOARD")
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{name} command failed: {cmd}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        ran += 1;
    }
    ran
}

/// The snippet `tb setup` writes into AGENTS.md / CLAUDE.md and the Claude Code skill teach
/// agents how to take, update, move and finish cards.
#[test]
fn agent_snippet_and_skill_cover_the_card_commands() {
    for (name, text) in [
        ("snippet", include_str!("../integrations/AGENTS-snippet.md")),
        ("skill", include_str!("../integrations/claude-code/SKILL.md")),
        ("root AGENTS.md", include_str!("../AGENTS.md")),
    ] {
        for cmd in ["tb next", "tb show", "tb note", "tb check", "tb edit", "tb move", "tb done", "tb drop", "tb block"] {
            assert!(text.contains(cmd), "{name} does not teach `{cmd}`");
        }
    }
}

/// docs/AGENTS.md must say plainly that card text is other agents' data, not instructions.
#[test]
fn agents_manual_states_notes_are_data_not_instructions() {
    let md = include_str!("../docs/AGENTS.md");
    for phrase in ["DATA written by other agents", "not instructions to you"] {
        assert!(md.contains(phrase), "docs/AGENTS.md lacks `{phrase}`");
    }
}

/// A markdown table must be one block of rows: a paragraph inserted into the middle of one
/// orphans every row after it, which renders as literal `|` text and hides a whole feature.
/// (Found in review: a done-by paragraph split the Cards table in docs/HUMANS.md and the
/// due-date row vanished from the guide.)
#[test]
fn no_documentation_table_is_split_by_a_paragraph() {
    for (name, md) in [
        ("README.md", include_str!("../README.md")),
        ("docs/HUMANS.md", include_str!("../docs/HUMANS.md")),
        ("docs/AGENTS.md", include_str!("../docs/AGENTS.md")),
        ("docs/JSON.md", include_str!("../docs/JSON.md")),
        ("docs/SCHEMA.md", include_str!("../docs/SCHEMA.md")),
        ("CHANGELOG.md", include_str!("../CHANGELOG.md")),
        ("UPGRADING.md", include_str!("../UPGRADING.md")),
    ] {
        let row = |l: &str| l.trim_start().starts_with('|') && l.trim_end().ends_with('|');
        // a table only ever starts after a blank line (or a heading): a row whose previous
        // line is prose is a row that was cut off from its table and renders as plain text
        let mut in_code = false;
        let mut prev = "";
        for (i, line) in md.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                in_code = !in_code;
                prev = line;
                continue;
            }
            if !in_code && row(line) && !prev.trim().is_empty() && !row(prev) && !prev.trim_start().starts_with('#') {
                panic!(
                    "{name}:{} — this table row follows prose ({prev:?}), so it was cut off from its table: every row after the break renders as literal text",
                    i + 1
                );
            }
            prev = line;
        }
    }
}

/// docs/SCHEMA.md must be referenced from JSON.md's forward-compatibility rule.
#[test]
fn json_contract_points_at_the_schema_doc() {
    let md = include_str!("../docs/JSON.md");
    assert!(md.contains("ignore unknown\nfields and unknown event kinds") || md.contains("ignore unknown fields"), "the rule is stated");
    assert!(md.contains("docs/SCHEMA.md"), "the rule names the schema doc");
}

/// No `## ` heading appears twice in the same doc: a duplicate is a stray label — usually a
/// merge that picked up the wrong heading line — and the section under the second occurrence
/// becomes unreachable by name. (Found: docs/JSON.md had `## \`tb agents --json\`` twice, the
/// first one actually the warnings section.)
#[test]
fn no_documentation_heading_is_duplicated() {
    for (name, md) in [
        ("README.md", include_str!("../README.md")),
        ("docs/HUMANS.md", include_str!("../docs/HUMANS.md")),
        ("docs/AGENTS.md", include_str!("../docs/AGENTS.md")),
        ("docs/JSON.md", include_str!("../docs/JSON.md")),
        ("docs/SCHEMA.md", include_str!("../docs/SCHEMA.md")),
    ] {
        let mut seen = std::collections::HashSet::new();
        let mut in_code = false;
        for line in md.lines() {
            if line.trim_start().starts_with("```") {
                in_code = !in_code;
                continue;
            }
            if !in_code && line.starts_with("## ") {
                assert!(seen.insert(line), "{name} has the heading {line:?} twice");
            }
        }
    }
}
