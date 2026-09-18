#!/usr/bin/env bash
# Privacy check: prints every line that looks like private data and fails if there is any.
#
# Built in (generic): absolute home paths, IP addresses, e-mail addresses (except
# @example.com and GitHub noreply), and credentials (API keys, tokens, private keys).
# Optional: a local, git-ignored `.privacy-words` file with one extra regex per line
# (names, hosts, project names …). Plain words shorter than 5 letters match as whole
# words; everything else matches anywhere, case-insensitively.
#
#   bash scripts/privacy-check.sh          (from anywhere; empty output = clean)
set -euo pipefail
cd "$(dirname "$0")/.."

WORDS_FILE="${PRIVACY_WORDS:-.privacy-words}"

files() {
    find . -type f \
        -not -path './target/*' -not -path './.git/*' \
        -not -path './scripts/privacy-check.sh' -not -name '.privacy-words' -print0
}

scan() { # scan GREP-FLAGS PATTERN
    files | xargs -0 grep -nIE "$1" -e "$2" 2>/dev/null || true
}

generic() {
    # absolute home directories (e.g. /Users/<name>/…, /home/<name>/…)
    scan -i '/(Users|home)/[A-Za-z0-9._-]+'
    # IPv4 addresses (incl. 100.64.0.0/10), except loopback and 0.0.0.0
    scan '' '\b([0-9]{1,3}\.){3}[0-9]{1,3}\b' | grep -vE '\b(127\.0\.0\.1|0\.0\.0\.0)\b' || true
    scan '' '\b100\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\.'
    # e-mail addresses, except the documentation and GitHub noreply domains
    scan '' '[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}' \
        | grep -vE '@(example\.(com|org|net)|users\.noreply\.github\.com)\b' || true
    # credentials
    scan '' '\bsk-[A-Za-z0-9_-]{8,}|\b(ghp|gho|ghs|ghu)_[A-Za-z0-9]{8,}|\bgithub_pat_|\bAKIA[0-9A-Z]{16}\b|-----BEGIN .*PRIVATE KEY|\bxox[bp]-'
}

local_words() {
    [ -f "$WORDS_FILE" ] || return 0
    local w re
    while IFS= read -r w || [ -n "$w" ]; do
        w="${w%%#*}"                       # comments
        w="$(printf '%s' "$w" | sed -E 's/^[[:space:]]+|[[:space:]]+$//g')"
        [ -n "$w" ] || continue
        if printf '%s' "$w" | grep -qE '^[A-Za-z0-9]+$' && [ "${#w}" -lt 5 ]; then
            re="\\b$w\\b"
        else
            re="$w"
        fi
        scan -i "$re"
    done <"$WORDS_FILE"
}

hits=$({ generic; local_words; } | sort -u)
if [ -n "$hits" ]; then
    printf '%s\n' "$hits"
    exit 1
fi
