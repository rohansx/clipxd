# clipxd — Browser capture (MV3 extension)

Records a browser tab's **video + audio** (via `chrome.tabCapture`) together
with a clipxd **BrowserTrace** — clicks, text input, console output, network
requests, and navigation — producing one clip that's both watchable *and*
agent-queryable, which screen recording alone can't do for other pages
(`getDisplayMedia` can only see input while the pointer is over the clipxd
tab itself).

If tab capture isn't available (permission denied, another capture already
active, etc.), it degrades gracefully to a trace-only clip — no video, but
still the full interaction/DOM/console/network track.

## Load it (unpacked)

1. Open `chrome://extensions`, enable **Developer mode**.
2. **Load unpacked** → select this `extension/` directory.
3. Click the clipxd icon → **Settings** and set:
   - **host** — `https://clipxd.com` (or your local `http://localhost:8787`).
   - **Bearer token** — from clipxd.com (needed on the hosted service; leave
     blank for a local no-auth backend).
4. On the tab you want to record, click the clipxd icon → **Record this
   tab**, do your thing, then **Stop & save clip** → the popup links straight
   to the new clip.

## How the video path works

`chrome.tabCapture` requires a genuine user gesture (the toolbar-icon click
that opens the popup) — it's a real browser security boundary, not something
this code can work around. On Record:

1. The clip id is minted up front via `POST /ingest/stage` (same instant-link
   architecture the web recorder uses — the share URL exists immediately).
2. `chrome.tabCapture.getMediaStreamId()` gets a stream id for the active tab.
3. An **offscreen document** (`offscreen.html`/`offscreen.js`) — the only MV3
   context with a real DOM, since the service worker has none — turns that
   into a `MediaStream`, records it with `MediaRecorder`, and `PUT`s each
   ~4s chunk straight to `/ingest/stage/:id?seq=N`.
4. On Stop, the trace assembled from `content.js`'s captured events is POSTed
   to `/ingest/stage/:id/commit` as the body — the server fuses it into the
   same clip's index after enrichment (transcript/frames from the video,
   interaction/DOM/console/network from the trace).

## What it captures

| Event | Source |
|-------|--------|
| `navigate` | load + SPA route changes (`pushState`/`popstate`/`hashchange`) |
| `click` | capture-phase click listener (target selector + label + x/y) |
| `input` | `change` + Enter-to-submit (passwords / card fields masked) |
| `scroll` | throttled scroll position |
| `console` | main-world hook over `console.*` + `window.onerror` |
| `network` | `chrome.webRequest` (method, url, status, type, duration) |
| `dom_snapshot` | coarse snapshot (node count + visible text) on record start |

## Local (WebGPU/wasm) captioning

If the recording account's `caption_mode` setting (`GET /settings/keys`) is
`"local"`, `background.js` passes `includeLocalCaptioning: true` into the
offscreen `start` message alongside `includeCamera`. `local-captioner.js`
then samples the tab video every ~6s and runs each frame through
[Xenova/moondream2](https://huggingface.co/Xenova/moondream2) — a small
(~1.6-1.8B param) VLM, via [Transformers.js](https://github.com/huggingface/transformers.js)
— entirely client-side (WebGPU, falling back to wasm, falling back to
skipping captioning silently if neither works). No frame or caption text
ever leaves the browser except the final short caption strings, POSTed to
`/clip/:id/local-captions` on Stop.

Transformers.js isn't used from a CDN — MV3's CSP blocks that, and this
extension has no build step besides the one-time vendoring below.

- `vendor/build.sh` — one-time (re-run only when bumping pinned versions)
  script that bundles `@huggingface/transformers` + `onnxruntime-web/webgpu`
  into `vendor/transformers.min.js` (esbuild, zero remaining bare-specifier
  imports — see the script's own comments for why a plain copy of the
  published dist file doesn't work) plus the two ONNX Runtime Web wasm assets
  it needs at runtime. Model *weights* are not vendored — those are fetched
  from huggingface.co on first use and cached by the browser.
- `manifest.json`'s `content_security_policy` — restates the documented MV3
  default (`script-src 'self' 'wasm-unsafe-eval'`) explicitly. This was
  empirically necessary: leaving it unspecified produced `WebAssembly.compile`
  CSP violations in testing, even though it's supposed to be the default.

## Files

- `manifest.json` — MV3 manifest (content scripts + service worker + popup +
  offscreen document).
- `content.js` — isolated-world capture (clicks/input/scroll/nav/DOM); dormant
  until the worker arms the tab.
- `inject.js` — main-world console hook (relays to `content.js` via
  `postMessage`).
- `background.js` — recording state, tab-capture + offscreen orchestration,
  network capture, trace assembly, and the stage/commit calls.
- `offscreen.js` / `offscreen.html` — the DOM context that runs
  `getUserMedia`/`MediaRecorder` on the tab-capture stream and PUTs chunks.
- `local-captioner.js` — fully local WebGPU/wasm Moondream2 captioning (see
  above).
- `vendor/` — the vendored Transformers.js bundle + ONNX Runtime Web wasm
  assets `local-captioner.js` dynamically imports (see `vendor/build.sh`).
- `popup.html` / `popup.js` — record/stop UI + host/token settings.
