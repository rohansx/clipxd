#!/usr/bin/env bash
# Ship clipxd to the provisioned Hetzner box: build the SPA + static binaries locally, rsync
# them up, install the Caddyfile with your domain, and (re)start the services. Idempotent.
#
# Prereqs (one-time): deploy/provision.sh has run on the server; you've authorized the deploy
# key; DNS A-record for $DOMAIN → the server IP; /etc/clipxd/clipxd.env filled in on the server.
#
# Usage:
#   DOMAIN=clips.example.com SERVER=root@178.104.122.118 deploy/deploy.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DOMAIN="${DOMAIN:?set DOMAIN=clips.example.com}"
SERVER="${SERVER:?set SERVER=root@1.2.3.4}"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/clipxd_deploy}"
TARGET="${TARGET:-x86_64-unknown-linux-musl}"   # static binary → runs on any Ubuntu glibc
SSH="ssh -i $SSH_KEY -o StrictHostKeyChecking=accept-new"
RSYNC_E="ssh -i $SSH_KEY -o StrictHostKeyChecking=accept-new"

echo "==> 1/4 build the SPA"
( cd "$ROOT/app" && ./node_modules/.bin/tsc --noEmit && ./node_modules/.bin/vite build )

echo "==> 2/4 build static binaries ($TARGET)"
if ! rustup target list --installed | grep -q "$TARGET"; then
  echo "   adding rust target $TARGET"; rustup target add "$TARGET"
fi
# rusqlite (bundled sqlite, C) cross-compiles with musl-gcc; rustls (no openssl) is musl-clean.
export CC_x86_64_unknown_linux_musl="${CC_x86_64_unknown_linux_musl:-musl-gcc}"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER:-musl-gcc}"
( cd "$ROOT" && cargo build --release --target "$TARGET" -p clipxd-web -p clipxd-cli )
BIN_DIR="$ROOT/target/$TARGET/release"

echo "==> 3/4 ship to $SERVER"
$SSH "$SERVER" 'mkdir -p /opt/clipxd /var/www/clipxd'
rsync -az --delete -e "$RSYNC_E" "$ROOT/app/dist/" "$SERVER:/var/www/clipxd/"
rsync -az -e "$RSYNC_E" "$BIN_DIR/clipxd-web" "$BIN_DIR/clipxd" "$SERVER:/opt/clipxd/"
# Caddyfile with the concrete domain baked in
sed "s/{\$CLIPXD_DOMAIN}/$DOMAIN/g" "$ROOT/deploy/Caddyfile" | $SSH "$SERVER" 'cat > /etc/caddy/Caddyfile'

echo "==> 4/4 set perms + (re)start services"
$SSH "$SERVER" '
  chown clipxd:clipxd /opt/clipxd/clipxd-web /opt/clipxd/clipxd &&
  chmod +x /opt/clipxd/clipxd-web /opt/clipxd/clipxd &&
  chown -R clipxd:clipxd /var/www/clipxd &&
  systemctl enable --now clipxd-web &&
  systemctl restart clipxd-web &&
  systemctl reload caddy 2>/dev/null || systemctl restart caddy
'

echo ""
echo "✅ deployed →  https://$DOMAIN"
echo "   GitHub OAuth callback to register:  https://$DOMAIN/auth/github/callback"
echo "   logs:  ssh $SERVER 'journalctl -u clipxd-web -f'"
