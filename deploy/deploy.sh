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
run_remote() {
  # Run a shell snippet on $SERVER. For localhost, use bash directly.
  if [ "$SERVER" = "localhost" ]; then
    sudo -n bash -c "$1"
  else
    ssh -i "$SSH_KEY" -o StrictHostKeyChecking=accept-new "$SERVER" "$1"
  fi
}
rsync_to() {
  # rsync a local path to $SERVER:$2. For localhost, sudo cp -a.
  local src="$1"; local dst="$2"
  if [ "$SERVER" = "localhost" ]; then
    sudo -n bash -c "mkdir -p '$dst' && rsync -a --delete '$src/' '$dst/'"
  else
    rsync -az -e "ssh -i $SSH_KEY -o StrictHostKeyChecking=accept-new" "$src" "$SERVER:$dst"
  fi
}

# ── 1. SPA ─────────────────────────────────────────────────────────────────
echo "==> 1/4 build the SPA"
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

# ── 3. ship ─────────────────────────────────────────────────────────────────
echo "==> 3/4 ship to $SERVER"
run_remote 'mkdir -p /opt/clipxd /var/www/clipxd'
if [ "$SERVER" = "localhost" ]; then
  sudo -n bash -c "rsync -a --delete '$BIN_DIR/clipxd-web' '$BIN_DIR/clipxd' /opt/clipxd/"
else
  rsync -az -e "ssh -i $SSH_KEY -o StrictHostKeyChecking=accept-new" "$BIN_DIR/clipxd-web" "$BIN_DIR/clipxd" "$SERVER:/opt/clipxd/"
fi
rsync_to "$ROOT/app/dist/" "/var/www/clipxd/"
# Caddyfile with the concrete domain baked in
sed "s/{\\\$CLIPXD_DOMAIN}/$DOMAIN/g" "$ROOT/deploy/Caddyfile" | run_remote 'cat > /etc/caddy/Caddyfile'

# ── 4. restart ───────────────────────────────────────────────────────────────
echo "==> 4/4 set perms + (re)start services"
run_remote '
  chown clipxd:clipxd /opt/clipxd/clipxd-web /opt/clipxd/clipxd 2>/dev/null || true
  chmod +x /opt/clipxd/clipxd-web /opt/clipxd/clipxd
  chown -R clipxd:clipxd /var/www/clipxd
  systemctl enable --now clipxd-web 2>/dev/null || true
  systemctl restart clipxd-web
  systemctl reload caddy 2>/dev/null || systemctl restart caddy
'

echo ""
echo "✅ deployed →  https://$DOMAIN"
echo "   GitHub OAuth callback:        https://$DOMAIN/auth/github/callback"
echo "   tail logs:                    ssh $SERVER 'journalctl -u clipxd-web -f'"
echo "   deploy from laptop:           DOMAIN=$DOMAIN SERVER=$SERVER $ROOT/deploy/deploy.sh"