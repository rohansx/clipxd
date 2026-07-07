#!/usr/bin/env bash
# =============================================================================
# ci-deploy.sh — the *only* command authorised for the clipxd-ci SSH key
# (see /opt/clipxd/.ssh/authorized_keys).  No matter what `appleboy/ssh-action`
# or any other client asks for, sshd hands control to this file with the
# ORIGINAL command in $SSH_ORIGINAL_COMMAND (for audit logging only — we do
# NOT run it).
#
# Flow:
#   1. log start (with cksum so we can prove the wrapper itself wasn't edited)
#   2. fast-forward local master to origin/master (idempotent, never destructive)
#   3. refuse if the fast-forward isn't possible (unmerged local commits)
#   4. fast-forward the sibling /home/clipxd/veyo checkout the same way — clipxd-web/
#      clipxd-recorder depend on it via path deps, so a stale veyo checkout can fail the
#      build outright (a type the new clipxd code needs doesn't exist yet) or, worse,
#      silently ship an old enrichment pipeline under a new clipxd binary
#   5. delegate to the existing deploy/deploy.sh — it builds SPA + Rust
#      binaries, restarts the systemd unit, reloads Caddy
#
# Logging: /var/log/clipxd-ci-deploy.log — one line per state change.
# =============================================================================
set -euo pipefail

LOG=/var/log/clipxd-ci-deploy.log
mkdir -p "$(dirname "$LOG")"
touch "$LOG"
chown clipxd:clipxd "$LOG" 2>/dev/null || true

ts() { date -u +%Y-%m-%dT%H:%M:%SZ; }

# Force the CI session to be the clipxd user; this script is dropped here
# as that user, but be defensive — a wrong owner here would silently run as
# whoever invokes it.
case "$(id -un)" in
  clipxd|root) ;;
  *) echo "$(ts) REFUSED: invoked by $(id -un) (not clipxd or root)" >>"$LOG"
     echo "ci-deploy.sh refused: must run as clipxd or root" >&2
     exit 77 ;;
esac

echo "$(ts) BEGIN  original='${SSH_ORIGINAL_COMMAND:-<none>}' cksum='$(cksum /home/clipxd/.github-actions/ci-deploy.sh 2>/dev/null | head -1 || echo unknown)'" >>"$LOG"

cd /home/clipxd/clipxd

# Discard build-regenerated lockfile drift before the fast-forward. `./deploy/deploy.sh`
# runs `cargo build`, which can rewrite Cargo.lock in the working tree; that leftover then
# blocks the next `--ff-only` merge whenever an incoming commit also touches Cargo.lock
# ("Your local changes to Cargo.lock would be overwritten by merge" → exit 2). The committed
# lockfile is authoritative, so restoring it is safe and idempotent (no-op when already clean).
git checkout -- Cargo.lock 2>/dev/null || true

# Fast-forward local master to origin/master (the new push tip).
# --ff-only refuses to merge if the histories diverged, so we don't
# silently overwrite local dev work on master.
echo "$(ts) fetching+ff master" >>"$LOG"
LOCAL=$(git rev-parse master)
git fetch --quiet github master >>"$LOG" 2>&1
REMOTE=$(git rev-parse github/master)
if [ "$LOCAL" != "$REMOTE" ]; then
  # Try a fast-forward.
  if git merge --ff-only github/master >>"$LOG" 2>&1; then
    NEW=$(git rev-parse master)
    echo "$(ts) fast-forwarded $LOCAL -> $NEW" >>"$LOG"
  else
    echo "$(ts) FAIL: local master diverged from github/master (local=$LOCAL remote=$REMOTE)" >>"$LOG"
    echo "Local master and origin/master have diverged. Resolve manually." >&2
    exit 2
  fi
fi

# Fast-forward the sibling veyo checkout the same way (clipxd's Cargo.toml path-deps into
# it, so a stale checkout there can break or silently stale-out the build). Best-effort: warn
# and continue if the directory is missing rather than failing the whole deploy over it, but
# a real divergence there is just as fatal as one in clipxd itself.
VEYO_DIR=/home/clipxd/veyo
if [ -d "$VEYO_DIR/.git" ]; then
  echo "$(ts) fetching+ff veyo" >>"$LOG"
  (
    cd "$VEYO_DIR"
    VEYO_LOCAL=$(git rev-parse master)
    git fetch --quiet origin master >>"$LOG" 2>&1
    VEYO_REMOTE=$(git rev-parse origin/master)
    if [ "$VEYO_LOCAL" != "$VEYO_REMOTE" ]; then
      if git merge --ff-only origin/master >>"$LOG" 2>&1; then
        echo "$(ts) veyo fast-forwarded $VEYO_LOCAL -> $(git rev-parse master)" >>"$LOG"
      else
        echo "$(ts) FAIL: veyo local master diverged from origin/master (local=$VEYO_LOCAL remote=$VEYO_REMOTE)" >>"$LOG"
        echo "veyo (sibling repo) master has diverged from origin/master. Resolve manually." >&2
        exit 2
      fi
    fi
  )
else
  echo "$(ts) WARN: $VEYO_DIR not found — skipping veyo sync (clipxd build may fail if it needs newer veyo code)" >>"$LOG"
fi

# Sanity: deploy only from master (we already fast-forwarded). Print the SHA
# so the GH Actions log + the deploy log both show what got shipped.
HEAD=$(git rev-parse HEAD)
echo "$(ts) deploying HEAD=$HEAD" >>"$LOG"

# Run the existing deploy/deploy.sh — it auto-detects this box
# (ubuntu-4gb-nbg1-2) and uses sudo internally.
echo "$(ts) running deploy/deploy.sh" >>"$LOG"
if ./deploy/deploy.sh >>"$LOG" 2>&1; then
  echo "$(ts) OK" >>"$LOG"
else
  echo "$(ts) FAIL: deploy/deploy.sh exited $?" >>"$LOG"
  exit 3
fi

echo "✓ deployed HEAD=$(git rev-parse --short HEAD)"
