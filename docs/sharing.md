# Sharing a clip

Every clip is a URL: `/clip/<id>` is a self-contained **watch + ask** page (video player plus a
box that queries the recording and answers with clickable timestamp citations). Three ways to
hand that URL to someone, in increasing reach:

## 1. LAN (default, nothing leaves your network)
Click **🔗 Share** in the editor → it copies `http://<your-lan-ip>:8787/clip/<id>`. Anyone on the
same wifi/LAN can open it. The editor learns your LAN IP from the server's `/net` endpoint (the
browser only knows whatever host you typed, so the server reports the routable address).

## 2. Public over Tailscale Funnel (HTTPS, anyone, revocable)
Exposes a clip to the **public internet** — the recipient needs nothing installed. The tunnel
dials **outbound** to Tailscale (no port-forwarding, no inbound firewall holes) and HTTPS is
terminated with a real cert for your `<host>.ts.net` name.

```bash
tools/clipxd-tunnel.sh [clips-dir]      # default clips-dir: /tmp/rec-clips
# → https://<host>.tailXXXX.ts.net/clip/<id>
```

**One-time enablement** (Tailscale gates public exposure on purpose): the first run prints an
`https://login.tailscale.com/f/funnel?node=…` link — open it, click **Enable Funnel**, re-run.

**Stop sharing instantly:**
```bash
tailscale funnel reset && kill $(lsof -ti tcp:8788)
```

### Why this is safe to expose
The tunnel points at a **separate, read-only** `clipxd-web --public` on `:8788`, not your editor's
full server. Public mode (`--public` / `CLIPXD_PUBLIC=1`) **drops these routes entirely (404)**:

| Route | Local (`:8787`) | Public (`:8788`, tunneled) |
|---|---|---|
| `GET /clip/:id` + `/video` `/frames` `/index.json` `/query` `/search` `/events` `/zoom.json` | ✅ | ✅ |
| `GET /` | clip list | safe landing (no enumeration) |
| `GET /clips` (enumerate all recordings) | ✅ | ❌ 404 |
| `POST /ingest` `/clip/:id/render` `/clip/:id/cursor` | ✅ | ❌ 404 |

So a public viewer can only watch and ask the **specific** clip whose link you sent. Clip ids are
random (`clp_<8 hex>`), so links are unlisted/unguessable (YouTube-unlisted model) — share the
link only with people who should see it. When `CLIPXD_PUBLIC_BASE` is set (the tunnel script sets
it), `/net` advertises that origin and the editor's 🔗 Share button copies the **public** link.

## 3. Persistent host (your VPS)
For a stable, always-on URL, run the same read-only server on a box with a public address and put
it behind your own TLS (`CLIPXD_PUBLIC=1 CLIPXD_PUBLIC_BASE=https://… clipxd-web <dir> --public`).
Push a clip's folder there to publish it.
