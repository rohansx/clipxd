#!/usr/bin/env bash
# tools/show-ci-secret.sh
# =============================================================================
# Print the deploy-only CI private key in the format GitHub expects when
# you add it as a repository secret called BOX_SSH_KEY.
#
# The matching public key is already in /opt/clipxd/.ssh/authorized_keys
# (with forced-command restriction — see /home/clipxd/.github-actions/ci-deploy.sh),
# so this key can ONLY run the deploy wrapper, never login or read files.
#
# Usage:
#   sudo -u clipxd bash /home/clipxd/clipxd/tools/show-ci-secret.sh
#
# Then in github.com/rohansx/clipxd:
#   Settings → Secrets and variables → Actions → New repository secret
#   Name:  BOX_SSH_KEY
#   Value: (paste the full -----BEGIN OPENSSH PRIVATE KEY----- block below)
# =============================================================================
set -euo pipefail
KEY=/home/clipxd/.ssh/clipxd-ci
if [ ! -f "$KEY" ]; then
  echo "✗ private key not found at $KEY" >&2
  exit 1
fi
echo "# BEGIN clipxd-ci private key for GitHub Actions secret BOX_SSH_KEY"
echo "# Repo:  rohansx/clipxd"
echo "# Perm:  ssh-ed25519, restricted (forced command = ci-deploy.sh)"
echo
cat "$KEY"
echo
echo "# END clipxd-ci private key"
echo
echo "↳ copied to clipboard via:    cat $KEY"
