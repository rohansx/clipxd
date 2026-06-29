#!/usr/bin/env bash
# Provision a fresh Ubuntu box (Hetzner CX23) to host clipxd. Run ONCE as root on the server:
#   ssh root@SERVER 'bash -s' < deploy/provision.sh
#
# Installs: ffmpeg, a PaddleOCR python venv (CPU OCR), Caddy (auto-TLS reverse proxy), Tailscale
# (to tunnel scene-captions to your home GPU), a swapfile (the 4GB box needs headroom), and the
# clipxd system user + directories + systemd unit. It does NOT start clipxd-web yet — run
# deploy.sh from your machine to ship the binary + SPA, then fill in /etc/clipxd/clipxd.env.
set -euo pipefail

echo "==> apt deps"
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y ffmpeg python3-venv python3-pip curl ca-certificates debian-keyring debian-archive-keyring apt-transport-https rsync \
  libgl1 libglib2.0-0

echo "==> swapfile (2G) — paddle + builds need headroom on 4GB"
if [ ! -f /swapfile ]; then
  fallocate -l 2G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
  grep -q '/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

echo "==> clipxd user + dirs"
id clipxd >/dev/null 2>&1 || useradd --system --create-home --home-dir /opt/clipxd --shell /usr/sbin/nologin clipxd
mkdir -p /opt/clipxd /var/www/clipxd /var/lib/clipxd/clips /etc/clipxd
chown -R clipxd:clipxd /opt/clipxd /var/lib/clipxd

echo "==> PaddleOCR venv (CPU OCR sidecar)"
# paddlepaddle's PyPI wheels max at cp313 — Ubuntu 26.04 ships python3.14 by default.
# Install uv (https://github.com/astral-sh/uv, single binary) and have it provision a
# Python 3.13 venv for us. uv installs Python itself on first use, no apt juggling.
if ! command -v uv >/dev/null; then
  curl -fsSL https://astral.sh/uv/install.sh | sh
  # uv installs to ~/.local/bin by default, but we're root → ~/.cargo/bin? check.
  UV_BIN=$(command -v uv || echo "/root/.local/bin/uv")
  [ -x "$UV_BIN" ] || UV_BIN="/root/.cargo/bin/uv"
fi
UV_BIN=$(command -v uv || echo "/root/.local/bin/uv")

if [ ! -x /opt/clipxd/venv/bin/python ]; then
  "$UV_BIN" venv --python 3.13 --seed /opt/clipxd/venv
fi
# uv's --seed flag injects pip/wheel/setuptools so `python -m pip` works.
# If --seed is missing in an older uv, fall back to `uv pip install` directly:
if ! /opt/clipxd/venv/bin/python -m pip --version >/dev/null 2>&1; then
  VENV_PY="/opt/clipxd/venv/bin/python"
else
  VENV_PY="/opt/clipxd/venv/bin/python"
fi
# paddlepaddle (CPU) + paddleocr; this is the heavy install (~1GB) — the swapfile covers it.
# NB: paddlepaddle wheels max at cp313, so this venv must be ≤ Python 3.13.
"$UV_BIN" pip install --python /opt/clipxd/venv/bin/python "paddlepaddle<3.3" paddleocr
chown -R clipxd:clipxd /opt/clipxd/venv

echo "==> Caddy (auto-TLS reverse proxy)"
if ! command -v caddy >/dev/null; then
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | tee /etc/apt/sources.list.d/caddy-stable.list >/dev/null
  apt-get update -y && apt-get install -y caddy
fi

echo "==> Tailscale (caption tunnel to your home GPU)"
if ! command -v tailscale >/dev/null; then
  curl -fsSL https://tailscale.com/install.sh | sh
fi
echo "    NOTE: join the tailnet with:  tailscale up   (or: tailscale up --authkey tskey-...)"
echo "    then point CLIPXD_CAPTION_URL at your home box, e.g. http://100.94.163.62:8799"

echo "==> systemd unit"
install -m 0644 /dev/stdin /etc/systemd/system/clipxd-web.service <<'UNIT'
[Unit]
Description=clipxd-web (hosted, multi-tenant)
After=network-online.target
Wants=network-online.target

[Service]
User=clipxd
Group=clipxd
EnvironmentFile=/etc/clipxd/clipxd.env
# venv on PATH for the PaddleOCR sidecar; /opt/clipxd holds the clipxd CLI used by render/import
Environment=PATH=/opt/clipxd/venv/bin:/usr/local/bin:/usr/bin:/bin
WorkingDirectory=/opt/clipxd
ExecStart=/opt/clipxd/clipxd-web /var/lib/clipxd/clips --port 8787
Restart=always
RestartSec=2
# hardening
NoNewPrivileges=true
ProtectSystem=full
ReadWritePaths=/var/lib/clipxd

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload

echo "==> env template"
if [ ! -f /etc/clipxd/clipxd.env ]; then
  cat > /etc/clipxd/clipxd.env <<'ENV'
# Fill these in, then: systemctl restart clipxd-web
CLIPXD_AUTH=1
CLIPXD_JWT_SECRET=CHANGE_ME_TO_A_LONG_RANDOM_STRING
CLIPXD_PUBLIC_BASE=https://YOUR_DOMAIN
# scene captions tunneled to your home GPU over Tailscale:
CLIPXD_CAPTION_URL=http://100.94.163.62:8799
# GitHub OAuth app (Settings > Developer settings > OAuth Apps):
GITHUB_CLIENT_ID=
GITHUB_CLIENT_SECRET=
ENV
  chmod 600 /etc/clipxd/clipxd.env
  echo "    wrote /etc/clipxd/clipxd.env (EDIT IT)"
fi

echo ""
echo "✅ provisioned. Next:"
echo "   1) tailscale up         (join your tailnet so captions can reach the home GPU)"
echo "   2) edit /etc/clipxd/clipxd.env   (domain, JWT secret, GitHub creds)"
echo "   3) from your laptop:  deploy/deploy.sh   (ships binary + SPA + Caddyfile, starts services)"
