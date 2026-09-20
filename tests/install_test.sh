#!/usr/bin/env bash
# Checks are single-quoted on purpose: `check` evals them after each run.
# shellcheck disable=SC2016,SC2034
# install.sh + `tb setup` scenarios, each under a fresh temp HOME, with a fake release
# directory (TB_RELEASE_BASE=file://…), a fake `gh`, and scripted answers (TB_TTY).
# Usage: tests/install_test.sh PATH/TO/tb     (from the repo root)
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
TBBIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
PASS=0
FAIL=0
unset TB_DB TB_BOARD TTYBOARD_DB TTYBOARD_BOARD TB_GH TTYBOARD_GH TB_NO_SETUP
export TB_TARGET=test-target TB_NO_HERDR=1 TB_TTY=/dev/null

ok() { PASS=$((PASS + 1)); echo "  ok   $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL $1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

SCRATCH=$(mktemp -d)
trap 'rm -rf "$SCRATCH"' EXIT
# install.sh copied OUT of the clone, so it takes the download path
INSTALL="$SCRATCH/install.sh"
cp "$ROOT/install.sh" "$INSTALL"

# a fake release: latest + v9.9.9, and a broken one with a wrong checksum
REL="$SCRATCH/rel"
make_release() { # DIR
    mkdir -p "$1"
    cp "$TBBIN" "$SCRATCH/tb"
    tar -czf "$1/tb-test-target.tar.gz" -C "$SCRATCH" tb
    (cd "$1" && { sha256sum tb-test-target.tar.gz 2>/dev/null || shasum -a 256 tb-test-target.tar.gz; } >tb-test-target.tar.gz.sha256)
}
make_release "$REL/latest/download"
make_release "$REL/download/v9.9.9"
BADREL="$SCRATCH/bad"
make_release "$BADREL/latest/download"
echo "0000  tb-test-target.tar.gz" >"$BADREL/latest/download/tb-test-target.tar.gz.sha256"
export TB_RELEASE_BASE="file://$REL"

fake_gh() { # DIR AUTH(ok|fail)
    mkdir -p "$1"
    cat >"$1/gh" <<EOF
#!/bin/sh
case "\$1 \$2" in
  "auth status") [ "$2" = ok ] && exit 0; echo "You are not logged into any GitHub hosts." >&2; exit 1 ;;
  "auth login") exit 1 ;;
  "repo view") case "\$3" in acme/widgets|me/first) echo '{"nameWithOwner":"'"\$3"'"}' ;; *) echo "not found" >&2; exit 1 ;; esac ;;
  "repo list") printf 'me/first\nme/second\n' ;;
  "pr list"|"issue list"|"run list") echo '[]' ;;
  api*) echo 0 ;;
  *) exit 1 ;;
esac
EOF
    chmod +x "$1/gh"
}

new_home() {
    HOME=$(mktemp -d "$SCRATCH/home.XXXX")
    export HOME
    FAKE="$HOME/../fake-$RANDOM"
    SHELL=/bin/bash
    export SHELL
    TB="$HOME/.local/bin/tb"
}

answers() { printf '%b' "$1" >"$SCRATCH/answers"; echo "$SCRATCH/answers"; }
tbcfg() { "$TB" config 2>/dev/null; }
setup_done() { [ "$(sqlite_cfg setup_done)" = 1 ]; }
# config value straight from the board file (setup_done is not a user setting)
sqlite_cfg() {
    "$TB" board --json >/dev/null 2>&1 || true
    python3 - "$HOME/.local/state/terminal-board/boards/default.db" "$1" <<'EOF'
import sqlite3, sys
try:
    r = sqlite3.connect(sys.argv[1]).execute("SELECT value FROM config WHERE key=?", (sys.argv[2],)).fetchone()
    print(r[0] if r else "")
except Exception:
    print("")
EOF
}

echo "(a) from the clone: --dry-run --yes builds locally, changes nothing"
new_home
"$ROOT/install.sh" --dry-run --yes >"$SCRATCH/out" 2>&1 || true
check "HOME untouched" '[ -z "$(ls -A "$HOME")" ]'
check "says dry run" 'grep -q "dry run" "$SCRATCH/out"'
check "prefers the local build" 'grep -q "running from a clone" "$SCRATCH/out" && grep -q "would run: cargo build --release" "$SCRATCH/out"'
check "would run setup" 'grep -q "would run: tb setup --yes" "$SCRATCH/out"'

echo "(b) download latest, verify, install; --no-setup"
new_home
"$INSTALL" --yes --no-setup >"$SCRATCH/out" 2>&1
check "tb installed, same binary" 'cmp -s "$TB" "$TBBIN"'
check "used latest/download" 'grep -q "latest/download/tb-test-target.tar.gz" "$SCRATCH/out"'
check "checksum verified" 'grep -q "checksum ok" "$SCRATCH/out"'
check "PATH asked (--yes = skip), rc untouched" 'grep -q "is not on your PATH" "$SCRATCH/out" && [ ! -e "$HOME/.bashrc" ]'
check "no board before setup" '[ ! -e "$HOME/.local/state/terminal-board/boards/default.db" ]'

echo "(c) --version picks that release; --prefix"
new_home
"$INSTALL" --yes --no-setup --version v9.9.9 --prefix "$HOME/bin" >"$SCRATCH/out" 2>&1
check "used download/v9.9.9" 'grep -q "download/v9.9.9/tb-test-target.tar.gz" "$SCRATCH/out"'
check "installed to --prefix" '[ -x "$HOME/bin/tb" ]'

echo "(d) bad checksum with --no-build: refuses"
new_home
rc=0
TB_RELEASE_BASE="file://$BADREL" "$INSTALL" --yes --no-setup --no-build >"$SCRATCH/out" 2>&1 || rc=$?
check "exit 1" '[ "$rc" = 1 ]'
check "says checksum mismatch" 'grep -q "checksum mismatch" "$SCRATCH/out"'
check "nothing installed" '[ ! -e "$TB" ]'
check "prints build instructions" 'grep -q "sh.rustup.rs" "$SCRATCH/out"'

echo "(e) missing asset, unsupported target: clear message"
new_home
rc=0
TB_TARGET=nope "$INSTALL" --yes --no-setup --no-build >"$SCRATCH/out" 2>&1 || rc=$?
check "exit 1 + download failed" '[ "$rc" = 1 ] && grep -q "download failed" "$SCRATCH/out"'

echo "(f) install then tb setup --yes --no-github --no-agents"
new_home
"$INSTALL" --yes --no-github --no-agents >"$SCRATCH/out" 2>&1
check "setup ran (numbered steps + summary)" 'grep -q "\[1/5\] Board" "$SCRATCH/out" && grep -q "^Summary" "$SCRATCH/out"'
check "github off + panel hidden" 'grep -Eq "^github +off$" <<<"$(tbcfg)" && grep -Eq "^github-panel +hidden$" <<<"$(tbcfg)"'
check "agents panel hidden" 'grep -Eq "^agents-panel +hidden$" <<<"$(tbcfg)"'
check "setup_done written" 'setup_done'
check "no skill" '[ ! -e "$HOME/.claude/skills/terminal-board" ]'

echo "(g) tb setup --github with a logged-in fake gh"
new_home
fake_gh "$FAKE" ok
"$INSTALL" --yes --no-setup >/dev/null 2>&1
TB_GH="$FAKE/gh" "$TB" setup --yes --github acme/widgets --no-agents >"$SCRATCH/out" 2>&1
check "repo saved" 'grep -Eq "^github +acme/widgets$" <<<"$(tbcfg)"'
check "panel shown" 'grep -Eq "^github-panel +shown$" <<<"$(tbcfg)"'
check "success line" 'grep -q "connected to acme/widgets" "$SCRATCH/out"'

echo "(h) gh not logged in, --yes: clean skip"
new_home
fake_gh "$FAKE" fail
"$INSTALL" --yes --no-setup >/dev/null 2>&1
rc=0
TB_GH="$FAKE/gh" "$TB" setup --yes --github acme/widgets --no-agents >"$SCRATCH/out" 2>&1 || rc=$?
check "exit 0" '[ "$rc" = 0 ]'
check "explains the skip" 'grep -q "gh auth login" "$SCRATCH/out"'
check "github off + hidden" 'grep -Eq "^github +off$" <<<"$(tbcfg)" && grep -Eq "^github-panel +hidden$" <<<"$(tbcfg)"'

echo "(i) skill + snippet; a re-run does not duplicate"
new_home
"$INSTALL" --yes --no-setup >/dev/null 2>&1
printf '# My agents file\n\nkeep this line\n' >"$HOME/AGENTS.md"
"$TB" setup --yes --no-github --agents --agents-md "$HOME/AGENTS.md" >"$SCRATCH/out" 2>&1
"$TB" setup --yes --no-github --agents --agents-md "$HOME/AGENTS.md" >>"$SCRATCH/out" 2>&1
check "skill installed" 'grep -q "^name: terminal-board" "$HOME/.claude/skills/terminal-board/SKILL.md"'
check "one snippet block" '[ "$(grep -c "terminal-board:start" "$HOME/AGENTS.md")" = 1 ] && [ "$(grep -c "terminal-board:end" "$HOME/AGENTS.md")" = 1 ]'
check "snippet body present" 'grep -q "tb next --as" "$HOME/AGENTS.md"'
check "own content kept" 'grep -q "keep this line" "$HOME/AGENTS.md"'
check "agents panel shown" 'grep -Eq "^agents-panel +shown$" <<<"$(tbcfg)"'

echo "(j) tb setup --dry-run changes nothing"
new_home
"$INSTALL" --yes --no-setup >/dev/null 2>&1
"$TB" setup --dry-run --yes --agents --agents-md "$HOME/A.md" >"$SCRATCH/out" 2>&1
check "no board, no files" '[ ! -e "$HOME/.local/state/terminal-board" ] && [ ! -e "$HOME/A.md" ] && [ ! -e "$HOME/.claude" ]'
check "summary says Would" 'grep -q "A real run would" "$SCRATCH/out" && grep -q "Would add the agent snippet" "$SCRATCH/out"'

echo "(k) interactive (TB_TTY answers), every step skipped with enter"
new_home
"$INSTALL" --yes --no-setup >/dev/null 2>&1
TB_TTY=$(answers '\n\n\n\n\n') "$TB" setup >"$SCRATCH/out" 2>&1
check "asked [y]es / [S]kip" 'grep -q "Connect GitHub.*\[y\]es / \[S\]kip" "$SCRATCH/out"'
check "github hidden" 'grep -Eq "^github-panel +hidden$" <<<"$(tbcfg)"'
check "agents hidden, no skill" 'grep -Eq "^agents-panel +hidden$" <<<"$(tbcfg)" && [ ! -e "$HOME/.claude" ]'
check "summary lists the skips" 'grep -q "^Skipped:" "$SCRATCH/out"'

echo "(k2) interactive, no ~/.claude: the skill question is not asked"
new_home
"$INSTALL" --yes --no-setup >/dev/null 2>&1
TB_TTY=$(answers '\n\n~/NOTES.md\n') "$TB" setup >"$SCRATCH/out" 2>&1
check "skill question absent" '! grep -q "Install the Claude Code skill" "$SCRATCH/out"'
check "skip explained" 'grep -q "Claude Code not detected" "$SCRATCH/out"'
check "one skip line, no ~/.claude created" '[ "$(grep -c "^ *- Claude Code skill" "$SCRATCH/out")" = 1 ] && [ ! -e "$HOME/.claude" ]'
check "next answer reaches the snippet prompt" 'grep -q "terminal-board:start" "$HOME/NOTES.md"'

echo "(l) interactive: github yes, pick #1, skill yes, snippet path"
new_home
fake_gh "$FAKE" ok
"$INSTALL" --yes --no-setup >/dev/null 2>&1
# a Claude Code user: ~/.claude exists, so the skill question is asked
mkdir -p "$HOME/.claude"
TB_GH="$FAKE/gh" TB_TTY=$(answers "y\n1\ns\ny\n~/CLAUDE.md\n") "$TB" setup >"$SCRATCH/out" 2>&1
check "list shown numbered" 'grep -q "1) me/first" "$SCRATCH/out"'
check "repo picked from the list" 'grep -Eq "^github +me/first$" <<<"$(tbcfg)"'
check "skill installed" '[ -f "$HOME/.claude/skills/terminal-board/SKILL.md" ]'
check "~ expanded for the snippet" 'grep -q "terminal-board:start" "$HOME/CLAUDE.md"'

echo "(m) --uninstall --yes keeps boards"
"$TB" add "keep me" >/dev/null 2>&1
TB_TTY=/dev/null "$INSTALL" --uninstall --yes >"$SCRATCH/out" 2>&1
check "binary removed" '[ ! -e "$TB" ]'
check "skill removed" '[ ! -e "$HOME/.claude/skills/terminal-board" ]'
check "snippet removed" '! grep -q "terminal-board:start" "$HOME/CLAUDE.md"'
check "boards kept" '[ -f "$HOME/.local/state/terminal-board/boards/default.db" ]'

echo "(n) first run: bare tb in a terminal offers setup; s skips and marks it done"
if command -v expect >/dev/null 2>&1; then
    new_home
    "$INSTALL" --yes --no-setup >/dev/null 2>&1
    unset TB_TTY
    export TERM=xterm-256color
    export TB
    cat >"$SCRATCH/first.exp" <<'EXP'
set timeout 10
set stty_init "rows 41 cols 126"
spawn $env(TB)
expect {
  "Set up Terminal Board now?" { send "s\r" }
  timeout { puts FAIL-PROMPT; exit 1 }
}
expect {
  "TODO" { send "q" }
  timeout { puts FAIL-BOARD; exit 1 }
  eof { puts FAIL-EOF; exit 1 }
}
expect eof
puts FIRST-OK
EXP
    cat >"$SCRATCH/second.exp" <<'EXP'
set timeout 10
set stty_init "rows 41 cols 126"
spawn $env(TB)
expect {
  "Set up Terminal Board" { puts ASKED-AGAIN; exit 1 }
  "TODO" { send "q" }
  timeout { puts FAIL; exit 1 }
}
expect eof
puts SECOND-OK
EXP
    first=$(expect -f "$SCRATCH/first.exp" 2>&1 || true)
    check "first run prompts, then opens the board" 'grep -q FIRST-OK <<<"$first"'
    check "skip still marks setup done" 'setup_done'
    second=$(expect -f "$SCRATCH/second.exp" 2>&1 || true)
    check "second run opens the board directly" 'grep -q SECOND-OK <<<"$second"'
    export TB_TTY=/dev/null
else
    echo "  skip (expect not installed)"
fi

echo
echo "install tests: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
