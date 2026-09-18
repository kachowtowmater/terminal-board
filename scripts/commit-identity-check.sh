#!/usr/bin/env bash
# Commit identity check: every commit in RANGE must be authored and committed with a
# GitHub no-reply address, so no personal e-mail ever lands in the public history.
#
#   bash scripts/commit-identity-check.sh origin/main..HEAD
#
# Allowed: <id>+<user>@users.noreply.github.com, <user>@users.noreply.github.com,
# noreply@github.com (merges and edits made on github.com).
# Tip: pin your identity for this repo once:
#   git config user.email "<id>+<user>@users.noreply.github.com"
set -euo pipefail
range="${1:-origin/main..HEAD}"
bad=$(git log --format='%h %an <%ae> / %cn <%ce>' "$range" \
    | grep -vE '<[^>]*@users\.noreply\.github\.com> / [^<]*<([^>]*@users\.noreply\.github\.com|noreply@github\.com)>$' || true)
if [ -n "$bad" ]; then
    echo "These commits use an address that is not a GitHub no-reply address:"
    echo "$bad"
    echo "Fix: set 'git config user.email <id>+<user>@users.noreply.github.com' and rewrite them"
    echo "(git rebase -r --exec 'git commit --amend --no-edit --reset-author' <base>)."
    exit 1
fi
echo "commit identities ok ($range)"
