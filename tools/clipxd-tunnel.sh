#!/bin/bash
# clipxd-tunnel — expose your clips to the public internet over Tailscale Funnel.
#
# Secure by construction:
#   • A SEPARATE read-only clipxd-web (--public) is what gets exposed: no /ingest, no /render,
#     no /cursor, no clip enumeration. A viewer can only watch + ask the SPECIFIC clip whose
#     (unguessable) link you send. Your full local editor on :8787 is never exposed.
#   • Funnel dials OUT to Tailscale — no inbound port-forwarding / firewall holes. HTTPS is
#     terminated by Tailscale with a real cert for your <host>.ts.net name.
#   • Stop sharing instantly:  tailscale funnel reset
#
# Usage:  tools/clipxd-tunnel.sh [clips-dir]        (default clips-dir: /tmp/rec-clips)
#         CLIPXD_PUBLIC_PORT=8788 tools/clipxd-tunnel.sh
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$HERE/../target/debug/clipxd-web"
CLIPS_DIR="${1:-/tmp/rec-clips}"
PORT="${CLIPXD_PUBLIC_PORT:-8788}"
LOGDIR="${CLIPXD_LOGDIR:-/tmp/clipxd-demo}"; mkdir -p "$LOGDIR"

[ -x "$BIN" ] || { echo "build first:  cargo build -p clipxd-web"; exit 1; }
[ -d "$CLIPS_DIR" ] || { echo "clips dir not found: $CLIPS_DIR"; exit 1; }
command -v tailscale >/dev/null || { echo "tailscale not installed"; exit 1; }

# Public host name for this node (e.g. arch.tailXXXX.ts.net)
HOST="$(tailscale status --json | python3 -c "import json,sys;print(json.load(sys.stdin)['Self']['DNSName'].rstrip('.'))")"
BASE="https://$HOST"

# (Re)start the hardened read-only server, telling it its own public origin.
lsof -ti tcp:"$PORT" 2>/dev/null | xargs -r kill 2>/dev/null || true
sleep 1
CLIPXD_PUBLIC=1 CLIPXD_PUBLIC_BASE="$BASE" \
  setsid nohup "$BIN" "$CLIPS_DIR" --port "$PORT" --public \
  > "$LOGDIR/public.log" 2>&1 < /dev/null &
sleep 2
curl -sf -o /dev/null "http://127.0.0.1:$PORT/net" || { echo "read-only server failed to start; see $LOGDIR/public.log"; exit 1; }
echo "read-only public server up on :$PORT  (mode: $(curl -s "http://127.0.0.1:$PORT/net" >/dev/null && echo READ-ONLY))"

# Expose it. Funnel must be enabled once for the tailnet; until then the CLI prints an enable
# URL and then *waits*, so we run it with a timeout and detect that case instead of hanging.
FUNNEL_OUT="$(timeout 20 tailscale funnel --bg "$PORT" < /dev/null 2>&1 || true)"
if echo "$FUNNEL_OUT" | grep -q 'f/funnel'; then
  URL="$(echo "$FUNNEL_OUT" | grep -oE 'https://login\.tailscale\.com/f/funnel[^[:space:]]*' | head -1)"
  echo
  echo "✗ Tailscale Funnel isn't enabled for your tailnet yet (one-time, ~20s):"
  echo "    1) Open:  $URL"
  echo "    2) Click 'Enable Funnel'."
  echo "    3) Re-run: tools/clipxd-tunnel.sh"
  exit 2
fi
# `funnel status` is authoritative but can lag a beat behind --bg; give it a few tries.
# (capture-then-match, not `| grep -q`: grep -q exits early → SIGPIPE upstream → pipefail false-fail)
ok=""
for _ in 1 2 3 4 5; do
  st="$(tailscale funnel status 2>/dev/null || true)"
  case "$st" in *"$HOST"*) ok=1; break;; esac
  sleep 1
done
if [ -z "$ok" ]; then
  echo "✗ Funnel did not come up. Tailscale said:"; echo "$FUNNEL_OUT" | sed 's/^/    /'
  exit 1
fi

echo
echo "✅ Public over HTTPS (anyone can open — no Tailscale needed on their end):"
echo "   $BASE/clip/<id>"
echo
echo "   Ready links for your current clips:"
shopt -s nullglob
for d in "$CLIPS_DIR"/*/; do
  id="$(basename "$d")"
  [ -f "$d/index.json" ] && echo "     $BASE/clip/$id"
done
echo
echo "   The editor's 🔗 Share button now copies the public link too (if it talks to :$PORT)."
echo "   Stop sharing:  tailscale funnel reset   &&   kill \$(lsof -ti tcp:$PORT)"
