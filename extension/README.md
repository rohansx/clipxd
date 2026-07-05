# clipxd — Browser capture (MV3 extension)

Records a browser session into a clipxd **BrowserTrace** — clicks, text input,
console output, network requests, and navigation — and POSTs it to
`POST /ingest/browser-trace`, producing an agent-queryable clip with a real
interaction event track.

This is the capture client for **Browser mode**. A screen recording
(`getDisplayMedia`) can only observe input while the pointer is over the clipxd
tab, so a recording of *another* page/app gets no interaction track. This
extension runs *inside* the page, so it captures the real one.

## Load it (unpacked)

1. Open `chrome://extensions`, enable **Developer mode**.
2. **Load unpacked** → select this `extension/` directory.
3. Click the clipxd icon → **Settings** and set:
   - **host** — `https://clipxd.com` (or your local `http://localhost:8787`).
   - **Bearer token** — from clipxd.com (needed on the hosted service; leave
     blank for a local no-auth backend).
4. On any page, click the icon → **Record this tab**, do your thing, then
   **Stop & save clip** → the popup links straight to the new clip.

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

- `manifest.json` — MV3 manifest (content scripts + service worker + popup).
- `content.js` — isolated-world capture (clicks/input/scroll/nav/DOM); dormant
  until the worker arms the tab.
- `inject.js` — main-world console hook (relays to `content.js` via
  `postMessage`).
- `background.js` — recording state, network capture, trace assembly + POST.
- `popup.html` / `popup.js` — record/stop UI + host/token settings.
