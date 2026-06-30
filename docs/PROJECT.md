# clipxd — Hetzner box runbook

This is the live, current state of the `clipxd` project on the Hetzner CX23 at `178.104.122.118` (Tailscale: `100.98.137.90`).

**If you're reading this from your phone after SSH'ing in, jump to the section you need.**

---

## Access

| From | How |
|---|---|
| Phone (Termius / Termux / Blink Shell) | `ssh clipxd@100.98.137.90` (Tailscale IP — works whenever your phone is online) |
| Laptop | `ssh clipxd@100.98.137.90` or `ssh rsx@178.104.122.118` |
| Repo | https://github.com/rohansx/clipxd — the source of truth |

The Tailscale SSH daemon is **not** enabled on either side (you'd need a UI session with `sudo tailscale up --ssh=true`). Instead we use a **bound-to-Tailscale** sshd on the laptop (`ListenAddress 100.94.163.62`) and `sshd` on the box — they both have the same `clipxd_deploy` key in `authorized_keys`. The Hetzner box's public IP (`178.104.122.118`) has sshd also bound to all interfaces so the laptop can ssh to it.

**You're reading this because you ssh'd as `clipxd`. Welcome.**

---

## What's running

```
/opt/clipxd/
  clipxd-web            # static musl binary (12 MB) — backend + share-layer HTTP
  clipxd                # CLI for render/import (5 MB)
  venv/                 # Python venv with paddlepaddle + paddleocr
systemd unit:
  clipxd-web.service    # reads /etc/clipxd/clipxd.env, serves :8787
other:
  Caddy                 # :443 → clipxd-web :8787 + serves SPA at /var/www/clipxd
  Tailscale             # joins your tailnet; box is 100.98.137.90
  PaddleOCR venv        # /opt/clipxd/venv/bin/python (CPU OCR sidecar)
  Moondream captioning  # see CLoudCaption section below
```

URL: `https://clipxd.com` (Live ✓)

---

## Git workflow — **GitHub is the source of truth**

```
┌──────────────┐  push   ┌────────────┐  clone/pull  ┌──────────────┐
│  laptop (you)│────────▶│   GitHub   │◀─────────────│ Hetzner box  │
│  gh CLI      │         │ rohansx/   │              │ clipxd user  │
└──────────────┘         │   clipxd   │              └──────────────┘
                         └────────────┘
```

Rules:
1. **Never** edit `/home/clipxd/clipxd/` directly on the box unless you want to lose those changes. The box is a build target, not the source.
2. Push to GitHub from the laptop first. Then pull on the box.
3. If you must edit on the box (e.g. emergency fix while away from the laptop), commit on the box, push back to GitHub as soon as you have laptop access.

### Pull latest + redeploy (run on the box)

```bash
cd /home/clipxd/clipxd
git pull github master       # fetch + merge latest from GitHub
./deploy/deploy.sh            # builds SPA + binaries, rsyncs to /opt/clipxd, restarts
sudo -n systemctl status clipxd-web
journalctl -u clipxd-web -n 20 --no-pager    # tail logs
```

### Make a code change from the box (when you don't have the laptop)

```bash
cd /home/clipxd/clipxd
# edit whatever file
git add -p
git commit -m "fix: ..."
# Push back to GitHub so the laptop can stay in sync:
git push github master
```

(Push from the box requires a GitHub PAT in `/home/clipxd/.github-token` — see setup.)

---

## Deploying

### One-time setup

1. **DNS** — A record for `clipxd.com` → `178.104.122.118` (set at Spaceship)
2. **Tailscale** — `tailscale up --authkey=$TS_AUTHKEY` (already done; box is `100.98.137.90`)
3. **GitHub OAuth App** — client ID + secret in `/etc/clipxd/clipxd.env`
4. **Storage** — `/etc/clipxd/clipxd.env` has `CLIPXD_STORAGE=local` (or `s3://...`)
5. **JWT secret** — `/etc/clipxd/clipxd.env` has `CLIPXD_JWT_SECRET=...` (64 hex chars)

### Deploy a new build (run on the box, or from laptop against the box)

```bash
# From laptop:
DOMAIN=clipxd.com SERVER=root@178.104.122.118 /home/rsx/Desktop/projx/clipxd/deploy/deploy.sh

# From the box (after `git pull`):
./deploy/deploy.sh
```

The script:
1. Builds the SPA (`vite build`)
2. Builds static musl binaries (`cargo build --target x86_64-unknown-linux-musl`)
3. Rsyncs `app/dist/` + binaries to `/opt/clipxd/`
4. Writes the Caddyfile with `$DOMAIN` baked in
5. Reloads Caddy + restarts `clipxd-web`

---

## The cloud stack

| Piece | Where | Why |
|---|---|---|
| **Hetzner CX23** | `178.104.122.118` | App server, only 4 GB RAM, runs clipxd-web + Caddy + PaddleOCR + (later) MinIO |
| **Tailscale** | your tailnet | Private network between laptop ↔ phone ↔ box; no public SSH exposure |
| **Hetzner Object Storage** | `nbg1` region | S3-compatible storage for video files (planned) |
| **Moondream Cloud API** | `api.moondream.ai/v1` | Scene captions for clips — no GPU needed on the box |
| **GitHub** | `rohansx/clipxd` | Source of truth for code |

---

## What's built ✅

- [x] **`clipxd-web`** — multi-tenant backend (JWT auth, email/pw + GitHub OAuth, per-user clip ownership)
- [x] **Username routing** — `https://clipxd.com/u/<slug>/clip/<id>` canonical share URLs
- [x] **Hetzner Caddy reverse proxy** — auto-TLS, route /api/* to backend, serve SPA
- [x] **`clipxd` CLI** (render, import)
- [x] **Two-phase ingest** — stub_clip fast, enrich_clip in background
- [x] **PaddleOCR venv** (CPU) — batched model-loads so the box's 4 GB RAM survives
- [x] **Moondream cloud captions** — `https://api.moondream.ai/v1/caption` with `X-Moondream-Auth` header; verified end-to-end (captioned a red test frame as "A red background fills the frame, uniformly colored and without any visible patterns, textures, or objects.")
- [x] **`yt-dlp` forwarder** (Python, on the home box or Tailscale-tunneled) — work-around for Loom/YouTube datacenter IP block
- [x] **GitHub is the source of truth** — laptop or box can `git push github master` / `git pull github master`
- [x] **Box pulls + deploys from the box itself** — `cd /home/clipxd/clipxd && ./deploy/deploy.sh` (no env vars needed; auto-detects local vs remote; sudo-nopasswd for the few ops it needs)
- [x] **PROJECT.md** on the box at `/home/clipxd/PROJECT.md` (and in `docs/PROJECT.md` in the repo)
- [x] **github-login.sh** — store a GitHub PAT on the box for push-back

## What's half-done ⚠️

- [ ] **Hetzner Object Storage** — bucket not created yet (need you to do that in the console)
- [ ] **S3 storage wiring** — code parses `CLIPXD_STORAGE=s3://...` but doesn't actually use S3 yet. Once bucket is up, this is ~150 LoC of real read/write plumbing.
- [ ] **`/clip/:id/claim` and `/clip/:id/re-enrich`** — endpoints live; SPA wiring pending (a "captions empty — re-enrich" banner was started in ClipPage.tsx but not deployed)
- [ ] **SPA "stale indexing pill" fix** — `visibilitychange` listener in `useClipData.ts` not deployed yet
- [ ] **Mobile UX polish** — sidebar user-chip showing username
- [ ] **YouTube ingest via residential proxy** — currently fails on the box's datacenter IP. Either use a residential proxy service, or use the home box's egress via the yt-dlp tunnel-forwarder

## What's not started 📋

- [ ] **Real GitHub PAT for the box** — `bash /home/clipxd/clipxd/deploy/github-login.sh` once you create one at github.com/settings/personal-access-tokens/new (fine-grained, contents: read+write, 90 days)
- [ ] **GitHub Actions CI** — runs `deploy/deploy.sh` on push to master, hits the box via SSH
- [ ] **Mobile UX polish** — sidebar user-chip showing username, "captions empty — re-enrich" banner in the SPA

---

## Common ops

### Check service health

```bash
sudo -n systemctl status clipxd-web caddy tailscaled    # these work without password
curl -i https://clipxd.com/auth/me                       # unauth: "not logged in"
curl -i https://clipxd.com/u/smoke/clip/clp_bc5859a9    # example clip
```

### Tail logs

```bash
sudo -n journalctl -u clipxd-web -n 50 --no-pager
sudo -n journalctl -u caddy -n 30 --no-pager
```

### Restart a service

```bash
sudo -n systemctl restart clipxd-web
sudo -n systemctl restart caddy
```

### Edit env vars (auth/secrets)

```bash
sudo nano /etc/clipxd/clipxd.env
sudo -n systemctl restart clipxd-web
```

### SSH into the laptop from the box (for bare-repo pulls)

```bash
ssh -i ~/.ssh/clipxd_deploy rsx@100.94.163.62
```

### SSH into the box from your phone

```bash
# In Termius / Termux / Blink Shell:
ssh clipxd@100.98.137.90
```

The `clipxd` user has passwordless sudo for: `systemctl {restart,reload,status,daemon-reload} clipxd-web|caddy`, `journalctl -u clipxd-web|caddy`, `systemctl reboot|poweroff`. Anything else still requires the sudo password.

---

## File map (on the box)

```
/etc/clipxd/clipxd.env             # env vars: JWT secret, OAuth creds, storage, captions
/opt/clipxd/
  clipxd-web                       # the backend binary (deployed here)
  clipxd                           # the CLI binary (deployed here)
  venv/                            # Python venv with paddlepaddle + paddleocr
/var/lib/clipxd/clips/
  clipxd.db                        # SQLite (users, clips ownership)
  clp_*/                           # per-clip dir (when storage=local)
/var/www/clipxd/                   # SPA static files (Vite build output)
/etc/caddy/Caddyfile               # reverse proxy config
/etc/systemd/system/clipxd-web.service
/home/clipxd/                      # <-- THIS is where the source lives
  .ssh/                            # SSH keys
  .github-token                    # (planned) PAT for pushing from box
  clipxd/                          # git working tree (cloned from github.com/rohansx/clipxd)
  veyo/                            # git working tree (sibling path dep, also from GitHub)
  PROJECT.md                       # (copy of docs/PROJECT.md) runbook
```

The split: `/opt/clipxd` holds the **deployed** artifacts (binaries + python venv). `/home/clipxd/clipxd` holds the **source**. Deploys copy source → `/opt/clipxd` then restart the service. This separation is intentional so a git pull + deploy doesn't pollute runtime state.

---

## Your home box (Arch laptop, `100.94.163.62`)

The "Moondream captioner" service used to run on this box over Tailscale. As of [date], we're switching to **Moondream's cloud API** (you pasted the key) — no home server needed. If you want the home server anyway for other VLMs (LLaVA, etc.), the script is at `tools/moondream-server/server.py` and the systemd unit pattern is at `tools/moondream-server/clipxd-caption.service` (TODO).

---

## Quick TODO list (runnable as a script)

```bash
# Weekly:
cd /home/clipxd/clipxd && git pull github master && ./deploy/deploy.sh
journalctl -u clipxd-web -n 30 --no-pager
# watch disk:
df -h / /var/lib/clipxd 2>/dev/null
# watch memory:
free -h
```

---

_Last updated by opencode during the 2026-06-30 session. Keep this file current whenever you ship._