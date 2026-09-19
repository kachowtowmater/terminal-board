//! Every `tb …` line in the README's code blocks runs, in order, against one temp board.
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
    let ran = run_doc(include_str!("../README.md"), "README");
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
        let o = Command::new("sh")
            .arg("-c")
            .arg(format!("\"$TB_BIN\"{rest}"))
            .env("TB_BIN", bin)
            .env("TB_DB", &db)
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
