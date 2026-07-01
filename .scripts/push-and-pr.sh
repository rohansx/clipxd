#!/usr/bin/env bash
# Push the redesign branch and open a PR.
# Run as the `clipxd` user (HOME=/opt/clipxd, where the key + config live).

set -euo pipefail

REPO=/home/clipxd/clipxd
BRANCH=design/puffy-clay-redesign
KEY=/opt/clipxd/.ssh/clipxd_deploy

# Home is /opt/clipxd; mirror the key + config from /home/clipxd/.ssh on demand.
[ -f "$KEY" ] || {
  cp /home/clipxd/.ssh/clipxd_deploy "$KEY"
  cp /home/clipxd/.ssh/config /opt/clipxd/.ssh/config
  chmod 600 "$KEY" /opt/clipxd/.ssh/config
}

echo "▶ ssh test (looking for: 'Hi rohansx!')…"
if ! ssh -T -o StrictHostKeyChecking=accept-new git@github.com 2>&1 | grep -qiE 'hi rohansx|successfully authenticated'; then
  echo "✗ GitHub doesn't recognise this key."
  exit 1
fi

cd "$REPO"
git remote set-url github "git@github.com:rohansx/clipxd.git"
git push -u github "$BRANCH"

if command -v gh >/dev/null && gh auth status >/dev/null 2>&1; then
  gh pr create \
    --base master --head "$BRANCH" \
    --title "design: port updated-3d-ClipXD.dc.html + SEO + agent-browser sweep" \
    --body-file .github/pr-design-redesign.md
else
  echo
  echo "✓ branch pushed."
  echo "  gh CLI isn't logged in — open this URL to finish:"
  echo "  https://github.com/rohansx/clipxd/compare/master...${BRANCH}?expand=1"
  echo "  Title: design: port updated-3d-ClipXD.dc.html + SEO + agent-browser sweep"
  echo "  Body:  cat .github/pr-design-redesign.md (or /home/clipxd/PR_BODY.md)"
fi
