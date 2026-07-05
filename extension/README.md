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
- `popup.html` / `popup.js` — record/stop UI + host/token settings.
