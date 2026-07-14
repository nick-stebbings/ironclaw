#!/bin/bash
# sync-upstream.sh — rebase the local-patches series onto latest nearai/ironclaw.
# Safe by construction: refuses on a dirty tree, snapshots a backup branch,
# stops on conflict. Run from anywhere inside /opt/ironclaw/src.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

BRANCH=local-patches
UPSTREAM=origin/main        # origin = https://github.com/nearai/ironclaw.git

[ "$(git rev-parse --abbrev-ref HEAD)" = "$BRANCH" ] || { echo "!! checkout $BRANCH first"; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "!! working tree dirty — commit or stash first"; exit 1; }

echo "fetching upstream (nearai/ironclaw)..."
git fetch origin --quiet
BASE=$(git merge-base "$BRANCH" "$UPSTREAM")
BEHIND=$(git rev-list --count "$BASE".."$UPSTREAM")
echo "base is $BEHIND commits behind $UPSTREAM (tip $(git log -1 --format=%h "$UPSTREAM"))"
[ "$BEHIND" -eq 0 ] && { echo "already up to date."; exit 0; }

BK="backup/pre-rebase-$(date +%Y%m%d-%H%M)"
git branch "$BK"
echo "backup branch: $BK   (revert anytime with: git reset --hard $BK)"

echo "rebasing $BRANCH onto $UPSTREAM ..."
if git rebase "$UPSTREAM"; then
  echo
  echo "REBASE OK. Now rebuild + canary + deploy:"
  echo "  bash scripts/build-all.sh"
  echo "  sudo systemctl restart ironclaw-reborn@drew-claw   # canary: verify 2.8/health/:3320"
  echo "  sudo systemctl restart ironclaw-reborn@pilot       # then prod"
  echo
  echo "Review series:            git log --oneline $UPSTREAM..$BRANCH"
  echo "Drop any upstreamed patch: git rebase -i $UPSTREAM"
else
  echo
  echo "!! CONFLICT. Resolve, then 'git rebase --continue'."
  echo "   To bail out cleanly: git rebase --abort && git reset --hard $BK"
  exit 1
fi
