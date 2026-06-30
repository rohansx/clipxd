# Deploying clipxd (hosted, multi-tenant) to Hetzner

A single CX23 (2 vCPU / 4 GB) running: **Caddy** (HTTPS) → **clipxd-web** (auth + API) → static **React SPA**, with **PaddleOCR** on CPU and scene-**captions tunneled to your home GPU** over Tailscale.

```
Internet ──HTTPS──> Caddy :443 ──┬─ /clip/* /clips /ingest /import /net /auth/*  → clipxd-web :8787
                                 └─ everything else                              → /var/www/clipxd (SPA)
clipxd-web ── PaddleOCR (venv, CPU) ── ffmpeg ── clipxd CLI (render/import)
clipxd-web ── CLIPXD_CAPTION_URL ──Tailscale──> home GPU Moondream :8799
```

## One-time setup

1. **DNS** — point an `A` record for your domain at the server IP (`178.104.122.118`).

2. **Authorize the deploy key** (already generated locally at `~/.ssh/clipxd_deploy`). On the server (Hetzner web console as root):
   ```
   mkdir -p ~/.ssh && echo '<contents of ~/.ssh/clipxd_deploy.pub>' >> ~/.ssh/authorized_keys
   ```

3. **GitHub OAuth App** — github.com → Settings → Developer settings → OAuth Apps → New:
   - Homepage URL: `https://YOURDOMAIN`
   - Authorization callback URL: `https://YOURDOMAIN/auth/github/callback`
   - Copy the **Client ID** and generate a **Client Secret**.

4. **Provision the server** (installs ffmpeg, PaddleOCR venv, Caddy, Tailscale, swap, the systemd unit):
   ```
   ssh -i ~/.ssh/clipxd_deploy root@178.104.122.118 'bash -s' < deploy/provision.sh
   ```

5. **Join Tailscale** on the server so captions can reach your home GPU:
   ```
   ssh -i ~/.ssh/clipxd_deploy root@178.104.122.118 'tailscale up'
   # ...and make sure your home box runs the Moondream server bound on 0.0.0.0:8799
   ```

6. **Fill the env** on the server — `/etc/clipxd/clipxd.env`:
   ```
   CLIPXD_AUTH=1
   CLIPXD_JWT_SECRET=<openssl rand -hex 32>
   CLIPXD_PUBLIC_BASE=https://YOURDOMAIN
   CLIPXD_CAPTION_URL=http://100.94.163.62:8799   # your home box's tailscale IP
   GITHUB_CLIENT_ID=...
   GITHUB_CLIENT_SECRET=...
   ```

## Deploy (repeatable)

From your laptop:
```
DOMAIN=clips.example.com SERVER=root@178.104.122.118 deploy/deploy.sh
```
Builds the SPA + static musl binaries, rsyncs them up, installs the Caddyfile with your domain, and restarts the services. Re-run any time to ship updates.

Logs: `ssh root@178.104.122.118 'journalctl -u clipxd-web -f'`

## Notes / tradeoffs
- **Binaries are static musl** so the dev box's newer glibc doesn't matter — they run on Ubuntu as-is.
- **Captions** require the home GPU + Tailscale online. If it's unreachable, captions degrade to heuristic (the recording is never lost). Swap to OpenRouter or a CPU GGUF VLM later by changing `CLIPXD_CAPTION_URL` / the caption backend.
- **Public share links** stay open by unguessable id (that's the sharing model); the **library/editor** is private per-account.
- **Backups:** `/var/lib/clipxd` holds the SQLite user/clip DB + clip files — enable Hetzner backups or rsync it.
- **Storage backend** is local disk by default. The env knob `CLIPXD_STORAGE=s3://bucket/prefix?endpoint=...&region=auto` is parsed at boot (so the env-file contract is stable) but the S3 read/write path isn't wired yet — clipxd-web logs `WARN ... falling back to local` and reads from disk. Set `CLIPXD_STORAGE=local` to silence the warning. Cloudflare R2 is the obvious target (S3-compatible, $0 egress), wiring it up is a focused ~150 LoC change.
- **yt-dlp tunnel for Loom/YouTube/Cap:** the Hetzner box's datacenter IP is blocked by Loom/YouTube's anti-bot, so `yt-dlp` on the box fails with "No video formats found". The fix is `tools/clipxd-yt-forwarder.py` running on your home box (over Tailscale) — set `CLIPXD_YT_TUNNEL_URL=http://<tailscale-ip>:8911/<token>` in `/etc/clipxd/clipxd.env` to enable. See `tools/clipxd-yt-forwarder.service` for the systemd unit; token + port live in `/etc/clipxd-yt-forwarder.env` on the home box.
