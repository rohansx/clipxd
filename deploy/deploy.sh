#!/usr/bin/env bash
# Ship clipxd to the provisioned Hetzner box: build the SPA + static musl binaries locally,
# ship them, install the Caddyfile, restart the services. Idempotent.
#
# Prereqs (one-time, on the deploy target):
#   - deploy/provision.sh has run
#   - clipxd_deploy pubkey is in /root/.ssh/authorized_keys (or you're running as root locally)
#   - DNS A-record for $DOMAIN → the server IP
#   - /etc/clipxd/clipxd.env is filled in
#
# Usage from laptop (rsync over ssh):
#   DOMAIN=clips.example.com SERVER=root@178.104.122.118 ./deploy/deploy.sh
#
# Usage from the box itself (after `git pull`):
#   cd /home/clipxd/clipxd
#   ./deploy/deploy.sh
#   (auto-detects DOMAIN=clipxd.com and SERVER=localhost; uses sudo for permissions)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOSTNAME_SHORT="$(hostname -s 2>/dev/null || hostname)"

# Default DOMAIN/SERVER based on whether we're on the box or not.
if [ "$HOSTNAME_SHORT" = "ubuntu-4gb-nbg1-2" ]; then
  : "${DOMAIN:=clipxd.com}"
  : "${SERVER:=localhost}"
  LOCAL_BUILD=1
else
  : "${DOMAIN:?set DOMAIN=clips.example.com}"
  : "${SERVER:?set SERVER=root@1.2.3.4}"
  LOCAL_BUILD=0
fi

SSH_KEY="${SSH_KEY:-$HOME/.ssh/clipxd_deploy}"
TARGET="${TARGET:-x86_64-unknown-linux-musl}"

# ── helpers ───────────────────────────────────────────────────────────────
# For localhost: sudo -n the perms. For remote: ssh.
run_remote_mkdir_p() {
  for d in "$@"; do
    if [ "$SERVER" = "localhost" ]; then sudo -n mkdir -p "$d"; else ssh -i "$SSH_KEY" -o StrictHostKeyChecking=accept-new "$SERVER" "mkdir -p '$d'"; fi
  done
}
run_remote() {
  # For the remote case, we trust the caller's script. For localhost, we need each
  # command to be in /etc/sudoers.d/92-clipxd-deploy-paths. To keep the call sites
  # clean, we shell out via `sudo -n sh -c` after widening sudoers to allow /usr/bin/sh.
  if [ "$SERVER" = "localhost" ]; then
    # 'sh' is in the sudoers file; -c runs the inline script as root.
    sudo -n sh -c "$1"
  else
    ssh -i "$SSH_KEY" -o StrictHostKeyChecking=accept-new "$SERVER" "$1"
  fi
}
rsync_to() {
  # rsync a local path to $SERVER:$2. For localhost, sudo rsync.
  local src="$1"; local dst="$2"
  if [ "$SERVER" = "localhost" ]; then
    sudo -n mkdir -p "$dst"
    sudo -n rsync -a --delete "$src/" "$dst/"
  else
    rsync -az -e "ssh -i $SSH_KEY -o StrictHostKeyChecking=accept-new" "$src" "$SERVER:$dst"
  fi
}

# ── 1. SPA ─────────────────────────────────────────────────────────────────
echo "==> 1/4 build the SPA"
if [ ! -d "$ROOT/app/node_modules" ]; then
  echo "    app/node_modules missing — running npm ci (this is a fresh clone?)"
  ( cd "$ROOT/app" && rm -rf dist && npm ci --no-audit --no-fund --prefer-offline )
  chown -R "$(id -un)":"$(id -gn)" "$ROOT/app/dist" "$ROOT/app/node_modules" 2>/dev/null || true
fi
( cd "$ROOT/app" && ./node_modules/.bin/tsc --noEmit && ./node_modules/.bin/vite build )

# ── 2. static binaries ─────────────────────────────────────────────────────
echo "==> 2/4 build static binaries ($TARGET)"
if command -v rustup >/dev/null 2>&1 && ! rustup target list --installed 2>/dev/null | grep -q "$TARGET"; then
  rustup target add "$TARGET"
fi
export CC_x86_64_unknown_linux_musl="${CC_x86_64_unknown_linux_musl:-musl-gcc}"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER:-musl-gcc}"
# Truly-static link flags — see commit history for why each one is needed.
export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static -C link-self-contained=yes -C relocation-model=static -C link-arg=-static -C link-arg=-no-pie"
( cd "$ROOT" && cargo build --release --target "$TARGET" -p clipxd-web -p clipxd-cli )
BIN_DIR="$ROOT/target/$TARGET/release"

# ── 3. stop the service first (so we can overwrite its binary without ETXTBSY) ─
echo "==> 3/4 stop the service, then ship to $SERVER"
run_remote '
  systemctl stop clipxd-web 2>/dev/null || true
  # Synchronous wait so we don'\''t race the cp.
  for i in 1 2 3 4 5 6 7 8 9 10; do
    if ! pgrep -x clipxd-web >/dev/null; then break; fi
    sleep 0.3
  done
  mkdir -p /opt/clipxd /var/www/clipxd
'
if [ "$SERVER" = "localhost" ]; then
  sudo -n cp -a "$BIN_DIR/clipxd-web" /opt/clipxd/
  sudo -n cp -a "$BIN_DIR/clipxd" /opt/clipxd/
else
  rsync -az -e "ssh -i $SSH_KEY -o StrictHostKeyChecking=accept-new" "$BIN_DIR/clipxd-web" "$BIN_DIR/clipxd" "$SERVER:/opt/clipxd/"
fi
rsync_to "$ROOT/app/dist/" "/var/www/clipxd/"
# Caddyfile with the concrete domain baked in
sed "s/{\\\$CLIPXD_DOMAIN}/$DOMAIN/g" "$ROOT/deploy/Caddyfile" | run_remote 'cat > /etc/caddy/Caddyfile'

# ── 4. restart ───────────────────────────────────────────────────────────────
echo "==> 4/4 set perms + (re)start services"
run_remote '
  sudo -n chown clipxd:clipxd /opt/clipxd/clipxd-web /opt/clipxd/clipxd 2>/dev/null || true
  sudo -n chmod +x /opt/clipxd/clipxd-web /opt/clipxd/clipxd
  sudo -n chown -R clipxd /var/www/clipxd 2>/dev/null || true
  systemctl enable --now clipxd-web 2>/dev/null || true
  systemctl restart clipxd-web
  systemctl reload caddy 2>/dev/null || systemctl restart caddy
'

echo ""
echo "✅ deployed →  https://$DOMAIN"
echo "   GitHub OAuth callback:        https://$DOMAIN/auth/github/callback"
echo "   tail logs:                    ssh $SERVER 'journalctl -u clipxd-web -f'"
echo "   deploy from laptop:           DOMAIN=$DOMAIN SERVER=$SERVER $ROOT/deploy/deploy.sh"