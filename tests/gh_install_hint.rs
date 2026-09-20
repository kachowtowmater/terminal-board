//! Issue #1: the setup wizard suggests the gh install command without `sudo` when there is no
//! sudo on the PATH or tb runs as root. Driven through `tb setup` with a controlled PATH.
#![cfg(target_os = "linux")]
use std::path::Path;
use std::process::Command;

fn exe(dir: &Path, name: &str) {
    let p = dir.join(name);
    std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with("Uid:"))
        .and_then(|l| l.split_whitespace().nth(1))
        == Some("0")
}

/// What `tb setup` prints about installing gh, with only `bins` on the PATH (and no gh).
fn suggestion(bins: &[&str]) -> String {
    let home = tempfile::tempdir().unwrap();
    let path = tempfile::tempdir().unwrap();
    for b in bins {
        exe(path.path(), b);
    }
    let o = Command::new(env!("CARGO_BIN_EXE_tb"))
        .args(["setup", "--yes", "--no-agents", "--github", "acme/widgets"])
        .env("HOME", home.path())
        .env("PATH", path.path())
        .env_remove("TB_GH")
        .env_remove("TB_DB")
        .env_remove("TB_BOARD")
        .env("TB_AS", "tester")
        .env("TB_NO_HERDR", "1")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    out.lines()
        .find_map(|l| l.split_once("Install it with: ").map(|(_, c)| c.trim().to_string()))
        .unwrap_or_else(|| panic!("no install suggestion in:\n{out}"))
}

#[test]
fn no_sudo_on_the_path_means_no_sudo_in_the_suggestion() {
    assert_eq!(suggestion(&["apt-get"]), "apt install gh");
    assert_eq!(suggestion(&["pacman"]), "pacman -S github-cli");
    assert_eq!(suggestion(&["dnf"]), "dnf install gh");
}

#[test]
fn with_sudo_the_prefix_depends_on_being_root() {
    let want = if root() { "apt install gh" } else { "sudo apt install gh" };
    assert_eq!(suggestion(&["apt-get", "sudo"]), want, "root = {}", root());
}
