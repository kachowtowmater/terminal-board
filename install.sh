#!/usr/bin/env bash
# Terminal Board installer.
#
#   curl -fsSL https://raw.githubusercontent.com/kachowtowmater/terminal-board/main/install.sh | bash
#   ./install.sh                 from a clone: builds this checkout when cargo is available
#   ./install.sh --uninstall     remove tb, the Claude Code skill and snippet blocks
#
# Downloads the release binary for this machine (checksum-verified), installs it as `tb`,
# then runs `tb setup`. Every question is read from the terminal, so piping into bash works.
set -euo pipefail

REPO="kachowtowmater/terminal-board"
RELEASE_BASE="${TB_RELEASE_BASE:-https://github.com/$REPO/releases}"
REPO_URL="${TB_REPO_URL:-https://github.com/$REPO.git}"
TTY_IN="${TB_TTY:-/dev/tty}"
PREFIX="$HOME/.local/bin"
VERSION=""
YES=0
DRY=0
UNINSTALL=0
NO_SETUP=0
NO_BUILD=0
BINARY=""
AGENTS_MD=""
SETUP_ARGS=()
STATE_DIR="$HOME/.local/state/terminal-board"
STATE_FILE="$STATE_DIR/install.conf"
START_MARK="<!-- terminal-board:start -->"
END_MARK="<!-- terminal-board:end -->"
SKILL_DIR="$HOME/.claude/skills/terminal-board"

usage() {
    cat <<'EOF'
Terminal Board installer

Usage: install.sh [options]      (curl -fsSL …/install.sh | bash -s -- [options])

  --prefix DIR        install tb into DIR (default ~/.local/bin)
  --version vX.Y.Z    install this release (default: the latest; the v is optional)
  --binary PATH       install this tb binary instead of downloading one
  --no-build          never build from source (fail if no release binary fits)
  --no-setup          do not run 'tb setup' afterwards
  --yes               accept defaults, ask nothing (passed on to 'tb setup')
  --github OWNER/REPO, --no-github, --agents, --no-agents, --agents-md PATH
                      passed on to 'tb setup'
  --uninstall         remove tb, the skill and snippet blocks (asks before board data)
  --dry-run           print what would happen; change nothing
  -h, --help          this help
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --yes|-y) YES=1; SETUP_ARGS+=(--yes) ;;
        --dry-run) DRY=1 ;;
        --uninstall) UNINSTALL=1 ;;
        --no-setup) NO_SETUP=1 ;;
        --no-build) NO_BUILD=1 ;;
        --prefix) PREFIX="${2:?--prefix needs a directory}"; shift ;;
        --version) VERSION="${2:?--version needs a tag like v3.1.2}"; VERSION="v${VERSION#v}"; shift ;;
        --binary) BINARY="${2:?--binary needs a path}"; shift ;;
        --github) SETUP_ARGS+=(--github "${2:?--github needs OWNER/REPO}"); shift ;;
        --agents-md) AGENTS_MD="${2:?--agents-md needs a path}"; SETUP_ARGS+=(--agents-md "$2"); shift ;;
        --no-github|--agents|--no-agents) SETUP_ARGS+=("$1") ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1 (see install.sh --help)" >&2; exit 2 ;;
    esac
    shift
done

TB="$PREFIX/tb"

say() { printf '%s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
die() { printf 'install: %s\n' "$*" >&2; exit 1; }

# ask QUESTION DEFAULT(y|s) -> 0 for yes. Reads the terminal; --yes, EOF or no terminal
# take the default.
ask() {
    local q="$1" def="$2" ans="" prompt
    if [ "$def" = y ]; then prompt="[Y]es / [s]kip"; else prompt="[y]es / [S]kip"; fi
    if [ "$YES" = 1 ]; then
        note "$q $prompt: $def (--yes)"
        [ "$def" = y ]
        return
    fi
    printf '    %s %s: ' "$q" "$prompt"
    if ! { [ -r "$TTY_IN" ] && read -r ans <"$TTY_IN"; }; then ans=""; echo; fi
    case "$ans" in
        y|Y|yes|YES) return 0 ;;
        s|S|skip|n|N|no|NO) return 1 ;;
        *) [ "$def" = y ] ;;
    esac
}

# run CMD... : execute, or print it in --dry-run
run() {
    if [ "$DRY" = 1 ]; then
        note "would run: $*"
        return 0
    fi
    "$@"
}

# ---------- uninstall ----------

strip_block() {
    local f="$1" tmp
    [ -f "$f" ] || return 0
    grep -qF "$START_MARK" "$f" || return 0
    tmp=$(mktemp)
    awk -v s="$START_MARK" -v e="$END_MARK" '
        $0 == s { skip = 1; next }
        $0 == e { skip = 0; next }
        !skip { print }
    ' "$f" >"$tmp"
    cat "$tmp" >"$f"
    rm -f "$tmp"
}

uninstall() {
    say "Terminal Board — uninstall"
    if ! ask "Remove tb, the Claude Code skill and the agent snippet blocks?" y; then
        say "Nothing removed."
        return 0
    fi
    if [ -e "$TB" ]; then run rm -f "$TB"; note "removed $TB"; fi
    if [ -d "$SKILL_DIR" ]; then run rm -rf "$SKILL_DIR"; note "removed $SKILL_DIR"; fi
    local line f
    if [ -f "$STATE_FILE" ]; then
        while IFS= read -r line; do
            case "$line" in
                snippet=*)
                    f="${line#snippet=}"
                    if [ "$DRY" = 1 ]; then note "would remove the snippet block from $f"
                    else strip_block "$f"; note "removed the snippet block from $f"; fi
                    ;;
            esac
        done <"$STATE_FILE"
    fi
    if [ -n "$AGENTS_MD" ] && [ "$DRY" = 0 ]; then strip_block "$AGENTS_MD"; note "removed the snippet block from $AGENTS_MD"; fi
    for f in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.profile" "$HOME/.config/fish/config.fish"; do
        if [ -f "$f" ] && grep -qxF "# Terminal Board" "$f"; then
            note "left the PATH line in $f (under '# Terminal Board'); remove it by hand if you like"
        fi
    done
    if [ -d "$STATE_DIR/boards" ]; then
        if ask "Also delete your boards in $STATE_DIR/boards? This cannot be undone." s; then
            run rm -rf "$STATE_DIR"
            note "deleted $STATE_DIR"
        else
            note "kept your boards in $STATE_DIR/boards"
            run rm -f "$STATE_FILE"
        fi
    elif [ -f "$STATE_FILE" ]; then
        run rm -f "$STATE_FILE"
    fi
    say "Uninstalled."
}

if [ "$UNINSTALL" = 1 ]; then
    uninstall
    exit 0
fi

# ---------- where the binary comes from ----------

# The release target for this machine (TB_TARGET overrides), or "" if there is none.
detect_target() {
    if [ -n "${TB_TARGET:-}" ]; then echo "$TB_TARGET"; return; fi
    local os arch
    os=$(uname -s)
    arch=$(uname -m)
    case "$arch" in
        arm64|aarch64) arch=aarch64 ;;
        x86_64|amd64) arch=x86_64 ;;
        *) echo ""; return ;;
    esac
    case "$os" in
        Darwin) echo "$arch-apple-darwin" ;;
        Linux) echo "$arch-unknown-linux-musl" ;;
        *) echo "" ;;
    esac
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
    else shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

fetch() { # fetch URL FILE
    if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$1"
    else return 1
    fi
}

# download TARGET DIR -> DIR/tb, checksum-verified
download() {
    local target="$1" dir="$2" base asset want got
    if [ -n "$VERSION" ]; then base="$RELEASE_BASE/download/$VERSION"; else base="$RELEASE_BASE/latest/download"; fi
    asset="tb-$target.tar.gz"
    note "downloading $base/$asset"
    if [ "$DRY" = 1 ]; then note "would download and verify $asset (+ .sha256)"; return 0; fi
    fetch "$base/$asset" "$dir/$asset" || { note "download failed"; return 1; }
    fetch "$base/$asset.sha256" "$dir/$asset.sha256" || { note "checksum file missing"; return 1; }
    want=$(cut -d' ' -f1 <"$dir/$asset.sha256")
    got=$(sha256_of "$dir/$asset")
    if [ "$want" != "$got" ]; then note "checksum mismatch for $asset (expected $want, got $got)"; return 1; fi
    note "checksum ok"
    tar -xzf "$dir/$asset" -C "$dir" tb || { note "could not unpack $asset"; return 1; }
    [ -x "$dir/tb" ]
}

# build SRC_DIR -> SRC_DIR/target/release/tb
build() {
    (cd "$1" && run cargo build --release --locked)
}

in_clone() {
    local here src="${BASH_SOURCE[0]:-}"
    # piped into bash there is no script file: never treat the current directory as a clone
    [ -n "$src" ] && [ -f "$src" ] || return 1
    here=$(cd "$(dirname "$src")" && pwd) || return 1
    [ -f "$here/Cargo.toml" ] && grep -q '^name = "terminal-board"' "$here/Cargo.toml" && echo "$here"
}

say "Terminal Board — install"
[ "$DRY" = 1 ] && say "(dry run: nothing will be changed)"
if [ -x "$TB" ]; then note "found $("$TB" --version 2>/dev/null || echo tb) at $TB; it will be replaced"; fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
NEW=""
CLONE=$(in_clone || true)

if [ -n "$BINARY" ]; then
    [ -x "$BINARY" ] || die "--binary $BINARY is not an executable file"
    NEW="$BINARY"
elif [ -n "$CLONE" ] && [ "$NO_BUILD" = 0 ] && command -v cargo >/dev/null 2>&1; then
    note "running from a clone ($CLONE) with cargo: building it"
    build "$CLONE" || die "cargo build failed"
    NEW="$CLONE/target/release/tb"
else
    TARGET=$(detect_target)
    if [ -n "$TARGET" ] && download "$TARGET" "$WORK"; then
        NEW="$WORK/tb"
    else
        [ -z "$TARGET" ] && note "no release binary for $(uname -s) $(uname -m)"
        if [ "$NO_BUILD" = 0 ] && command -v cargo >/dev/null 2>&1; then
            if ask "Build tb from source instead (git clone + cargo build --release)?" y; then
                run git clone --depth 1 "$REPO_URL" "$WORK/src" || die "git clone failed"
                build "$WORK/src" || die "cargo build failed"
                NEW="$WORK/src/target/release/tb"
            else
                die "nothing installed"
            fi
        else
            say ""
            say "No release binary could be installed. To build from source instead:"
            say "  1. install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
            say "  2. git clone $REPO_URL && cd terminal-board && ./install.sh"
            exit 1
        fi
    fi
fi

run mkdir -p "$PREFIX"
if [ "$DRY" = 1 ]; then
    note "would install tb to $TB"
else
    cp "$NEW" "$TB.tmp" && chmod 755 "$TB.tmp" && mv "$TB.tmp" "$TB"
    note "installed $("$TB" --version 2>/dev/null || echo tb) to $TB"
fi

case ":$PATH:" in
    *":$PREFIX:"*) ;;
    *)
        shell_name=$(basename "${SHELL:-sh}")
        case "$shell_name" in
            zsh) rc="$HOME/.zshrc"; line="export PATH=\"$PREFIX:\$PATH\"" ;;
            bash) rc="$HOME/.bashrc"; line="export PATH=\"$PREFIX:\$PATH\"" ;;
            fish) rc="$HOME/.config/fish/config.fish"; line="fish_add_path $PREFIX" ;;
            *) rc="$HOME/.profile"; line="export PATH=\"$PREFIX:\$PATH\"" ;;
        esac
        note "$PREFIX is not on your PATH. This line would go into $rc:"
        note "  $line"
        if ask "Add it?" s; then
            if [ "$DRY" = 0 ] && ! grep -qxF "$line" "$rc" 2>/dev/null; then
                mkdir -p "$(dirname "$rc")"
                printf '\n# Terminal Board\n%s\n' "$line" >>"$rc"
            fi
            note "added; open a new shell (or run: $line)"
        else
            note "skipped; run tb as $TB or add $PREFIX to PATH yourself"
        fi
        ;;
esac

if [ "$NO_SETUP" = 1 ]; then
    say "Installed. Next: tb setup   (then: tb)"
    exit 0
fi
if [ "$DRY" = 1 ]; then
    note "would run: tb setup ${SETUP_ARGS[*]+${SETUP_ARGS[*]}}"
    exit 0
fi
say ""
if [ -r "$TTY_IN" ] && { : <"$TTY_IN"; } 2>/dev/null; then
    exec "$TB" setup ${SETUP_ARGS[@]+"${SETUP_ARGS[@]}"} <"$TTY_IN"
fi
exec "$TB" setup ${SETUP_ARGS[@]+"${SETUP_ARGS[@]}"}
