# clipxd-capture (Playwright)

A small, **optional** capture emitter for the Phase-2 browser backend. It is *not* the
tested core — the `clipxd-browser` crate + its fixture are the contract. Any tool that
emits a valid trace ([phase2-browser-spec.md](../../docs/phase2-browser-spec.md) §1) works:
an rrweb post-processor, a hand-written CDP script, or this Playwright driver.

`capture.mjs` drives a page with headless Chromium, records console / network /
navigation / clicks / DOM mutations / a11y text / sparse screenshots, and writes a
`session.trace.json` you feed to `clipxd ingest-browser`.

## Use

```bash
npm install playwright          # browsers may already be cached in ~/.cache/ms-playwright
node capture.mjs ./clip         # drives the bundled checkout.html demo (a 500 on submit)
clipxd ingest-browser ./clip/session.trace.json --out clips
clipxd query clips/clp_*  "what error showed up and what was the user doing right before it"
```

`checkout.html` is a self-contained demo page (clicking **Place order** POSTs
`/api/checkout`, which the script's local origin returns as **500**, producing a console
error + an error toast). Point `capture.mjs` at any URL to capture a real flow instead.

## What the capture script normalizes (per spec §5.2)

- **One timebase:** every event's `t_ms` is wall-clock ms-since-epoch.
- **Network correlation:** one `network` event per response (`page.on('response')`).
- **In-page recorder:** a `MutationObserver` + click listener injected via `addInitScript`
  post normalized `dom_mutation` / `click` / `a11y_text` records back through an
  `exposeBinding`.
- **Redaction (Phase 4 will enforce):** mark password/payment fields `masked` at the DOM
  level *before* screenshots and before values enter the trace — the ingestor records a
  `redaction.items` marker and never copies the value into the index.
