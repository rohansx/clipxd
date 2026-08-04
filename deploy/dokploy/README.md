# clipxd on Dokploy

Containerised deployment, replacing the systemd + Caddy + `cargo build`-over-SSH setup on the
Hetzner VM.

## Shape

| service | image | what it is |
|---|---|---|
| `edge` | `ghcr.io/rohansx/clipxd-edge` | Caddy + the built SPA — routing, cache headers, the CSP |
| `web` | `ghcr.io/rohansx/clipxd-web` | the Rust service, plus ffmpeg and whisper |

Traefik (Dokploy's ingress) terminates TLS in front of `edge`, which listens plain `:80`.

**Caddy is kept rather than translated into Traefik middleware.** The Caddyfile carries the
nonce-based CSP, the security headers, `/fonts/*` → backend, `/assets/*` immutable caching and the
SPA fallback. Rewriting that as Traefik labels is exactly where subtle breakage lives — we already
shipped one production bug where `/fonts/*` fell through to the static handler and answered font
requests with `index.html`, silently, with no error anywhere. `deploy/dokploy/Caddyfile` is the VM
Caddyfile with three changes: bind `:80`, `auto_https off`, and `reverse_proxy web:8787`.

## Why CI builds the images

The Rust workspace path-depends on the sibling **veyo** repo (`veyo-core`, `veyo-enrich`). A build
context of this repo alone cannot resolve those, so Dokploy cannot build from git.
`.github/workflows/images.yml` checks out both repos side by side and builds with the parent
directory as context. Dokploy pulls the result.

If veyo is ever published to crates.io, this can go back to a plain git-source build.

## Deploying

1. Create a **Compose** service in Dokploy pointing at `deploy/dokploy/compose.yml`.
2. Set the environment (below).
3. Attach the domain to the **`edge`** service, port **80**.
4. Deploy.

### Environment

| variable | notes |
|---|---|
| `CLIPXD_JWT_SECRET` | **required.** Stable across restarts — a per-boot value logs every user out. |
| `CLIPXD_PUBLIC_BASE` | `https://clipxd.com` — what share links resolve to. |
| `CLIPXD_STORAGE` | R2: `s3://<bucket>?endpoint=https://<account>.r2.cloudflarestorage.com&region=auto`. Leave **unset** to run entirely off the data volume. |
| `R2_ACCESS_KEY_ID` / `R2_SECRET_ACCESS_KEY` | R2 credentials, mapped to the AWS SDK's env names. |
| `CLIPXD_OCR_MODE` | `server` (default) or `off`. See below. |
| `OLLAMA_API_KEY`, `NVIDIA_API_KEY`, `GEMINI_API_KEY`, `MOONDREAM_API_KEY` | remote inference. |
| `CLIPXD_WEB_MEMORY` | container memory cap, default `2G`. |

### On `CLIPXD_OCR_MODE`

OCR is the only heavy local inference left: PP-OCRv6 through ONNX Runtime peaks around **1.4 GB**,
where the rest of the pipeline is remote API calls costing nothing. Measured on one production
clip: **1.71 GB with server OCR, 0.35 GB without.**

- `server` — as before. Needs the 2 GB cap.
- `off` — text comes from the capture client instead: the browser extension already ships the
  page's DOM text (exact, no model anywhere), and `POST /clip/:id/local-text` accepts text a
  recording tab extracted itself. **A plain screen recording made without the extension gets no
  on-screen text in this mode** until the WebGPU extractor ships.

### Object storage: deliberately none

`CLIPXD_STORAGE` is unset in this deployment, so clips live on the data volume and nowhere else.

Two reasons, in order of weight. Hetzner Object Storage was billing roughly $1/month to hold under
a gigabyte, which is poor value for a bucket that only ever mirrors what is already on disk. And
every "the disk says X but HTTP says Y" bug this project has hit came from the S3-first read path
racing local writes — a stopped recording still reporting "recording", a 0:00 duration beside a
player showing 1:57. Removing the mirror removes that whole class.

The cost is that there is no offsite copy: **back up the `clipxd-data` volume**. If an offsite
copy becomes worth it, R2's free tier covers this size at no charge and needs only
`CLIPXD_STORAGE` plus credentials — the code path still exists and is unchanged.

## State

One volume, `clipxd-data`, holding clips, the SQLite auth DB, and the OCR model cache. It is the
only thing not reconstructible from the images. **Back up the volume, not the containers.**

## Migrating from the VM

Clip IDs are the product — share links must survive — so this is a file copy, not a re-import.
Re-ingesting the videos would mint new ids and lose ownership, comments and view counts.

Run from the Dokploy host (Dokploy exposes no exec or upload API, so this needs shell access
there):

```sh
# 1. pull the clips + SQLite auth DB off the VM (908 MB at time of writing)
rsync -avz --progress clipxd@<vm-ip>:/var/lib/clipxd/clips/ /tmp/clipxd-clips/

# 2. copy into the running container's volume
docker cp /tmp/clipxd-clips/. \
  "$(docker ps --filter name=clipxd-stack.*web --format '{{.Names}}' | head -1)":/data/clips/

# 3. the container runs as root, but make ownership explicit anyway
docker exec "$(docker ps --filter name=clipxd-stack.*web --format '{{.Names}}' | head -1)" \
  sh -c 'ls /data/clips | wc -l'   # expect ~26 clip directories + clipxd.db
```

Then cut DNS over, and keep the VM running until the new deployment has served real traffic.
