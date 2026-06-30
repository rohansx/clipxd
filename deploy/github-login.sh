#!/usr/bin/env bash
# github-login.sh — store a GitHub PAT on the box so it can push back to rohansx/clipxd.
# Run as the `clipxd` user. The PAT goes into /home/clipxd/.github-token (0600), and the
# origin remote is rewritten to embed it as `x-access-token:TOKEN@github.com/...`.
#
# Generate the PAT at: https://github.com/settings/personal-access-tokens/new
#   - Resource owner: rohansx
#   - Repository: rohansx/clipxd
#   - Permissions: Contents = Read and Write  (NO admin / NO actions / NO others)
#   - Expiry: 90 days (re-run this script when it expires)

set -euo pipefail

if [ "$(id -un)" != "clipxd" ]; then
  echo "Run as the clipxd user (or with sudo -u clipxd)"
  exit 1
fi

echo "Paste the GitHub PAT (it won't echo back). Then press Enter:"
read -rs TOKEN
echo
[ -n "$TOKEN" ] || { echo "empty token, aborting"; exit 1; }

# Validate the token shape (fine-grained PATs are 80+ chars starting with github_pat_...)
case "$TOKEN" in
  github_pat_*) ;;
  ghp_*)        ;;
  *) echo "warning: token doesn't start with github_pat_ or ghp_; continuing anyway" ;;
esac

# Verify it works against GitHub.
HTTP=$(curl -sS -o /dev/null -w "%{http_code}" -H "Authorization: Bearer $TOKEN" \
  https://api.github.com/repos/rohansx/clipxd)
if [ "$HTTP" != "200" ]; then
  echo "Token rejected by GitHub (HTTP $HTTP). Aborting."
  exit 1
fi
echo "Token verified — has access to rohansx/clipxd."

# Persist.
install -m 0600 /dev/null /home/clipxd/.github-token
printf '%s' "$TOKEN" > /home/clipxd/.github-token
chmod 600 /home/clipxd/.github-token

# Rewrite origin URL (only if not already set this way).
cd /home/clipxd/clipxd
CURRENT=$(git config --get remote.github.url 2>/dev/null || echo "")
WANTED="https://x-access-token:$TOKEN@github.com/rohansx/clipxd.git"
if [ "$CURRENT" != "$WANTED" ]; then
  if git remote get-url github >/dev/null 2>&1; then
    git remote set-url github "$WANTED"
  else
    git remote add github "$WANTED"
  fi
  echo "remote github URL updated"
fi

# Sanity: try to fetch — no commit, no push, just verify auth works.
git fetch github 2>&1 | tail -3
echo
echo "✅ box is now authenticated to push back to GitHub."