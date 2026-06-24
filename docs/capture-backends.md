# clipxd — capture backends

> Two ways in, one schema out. The agent that queries a clip cannot tell — and does not care — whether the index came from a screen recording, a browser trace, or an imported URL. This doc explains the two *capture* backends (the third source, import, has no capture and is covered in [architecture §4.3](architecture.md#43-import-from-url-the-traction-path-ships-first)).

---

## 1. Why two backends

Most "bug report I can hand an agent" traffic is **browser-based**, and for the browser, **video is the wrong primitive.** The DOM + accessibility tree + console + network trace is cheaper to capture, exact (no OCR guessing at text that the DOM already knows verbatim), and more legible to an agent than captioning pixels. Desktop demos and arbitrary native apps, by contrast, have no DOM — there pixels are all you have, and the veyo codec earns its keep.

So clipxd ships two capture backends with **different internals and an identical output contract.**

| | **browser backend** | **screen backend** |
|---|---|---|
| Primitive | DOM mutations · a11y tree · console · network · sparse screenshots | pixels (veyo delta codec) · cursor/click/key events |
| On-screen text | read verbatim from DOM / a11y | OCR over salient frames |
| Cost | very low (JSON deltas) | low (veyo gate keeps captioning sparse) |
| Best for | web bug reports, feedback, QA flows, SaaS demos | desktop app demos, native tools, anything without a DOM |
| Build effort | **easy — ship first (Phase 2)** | harder — realtime path (Phase 3) |
| Known weakness | animation-heavy pages mutate a lot; replay-time staleness | OCR/caption accuracy; realtime CPU budget |

---

## 2. The browser backend

> **Status: BUILT (Phase 2).** Implemented as the `clipxd-browser` crate — a clean-room browser-trace ingestor (no pixel codec; salience derived from the event stream). Full design + the trace JSON format in [phase2-browser-spec.md](phase2-browser-spec.md). Proven on a *real* Playwright capture: the headline demo answers *"Clicked 'Place order' → POST /api/checkout (500)"* via CLI and MCP.

### What it captures
- **DOM mutation stream** — every meaningful change to the page, as structured JSON (rrweb-class). This is the spine of the index's event track in browser mode.
- **Accessibility tree snapshots** — roles, labels, names. This is *exact* on-screen text and structure with no OCR.
- **Console** — logs, warnings, errors, stack traces (gold for bug reports).
- **Network trace** — requests, statuses, timings, failures (the 500 that broke the flow).
- **Sparse screenshots** — only at salient points (veyo-gated), stored so the human share page and any frame-context query have real pixels without relying on later DOM replay.

### Why DOM over video here
rrweb-style DOM recording is well-established: JSON events, far lighter than video, and the captured text is the *real* text — searchable and inspectable, not OCR'd. Its documented weaknesses are (a) animation-heavy / rapidly-mutating pages and (b) replay-time staleness (replay days later can pull *today's* data). clipxd sidesteps both: it does not depend on faithful later replay — it **snapshots salient screenshots at capture time** and treats the DOM stream as the event track, not as the canonical pixels.

### Redaction advantage
In browser mode, CloakPipe redacts **at the DOM level before screenshotting** — strip the SSN node, *then* snapshot. This is cleaner and cheaper than post-hoc pixel redaction, and it is something Cap/Loom cannot do because they only ever have pixels. See [privacy-and-redaction.md](privacy-and-redaction.md).

### Mechanism
A CDP (Chrome DevTools Protocol) / Playwright-style driver attaches to the page (extension, headless driver, or embedded webview), streams DOM/a11y/console/network, and triggers screenshots on salient events. The output is normalized into the raw-session format the pipeline expects.

---

## 3. The screen backend

### What it captures
- **Pixels**, fed live into **veyo-core**, which keeps a text world-state and emits a delta only when the scene meaningfully changes. Only salient frames are captioned/OCR'd downstream.
- **Cursor / click / key / scroll events** — the input event track, time-aligned to the video.
- **Audio** — for transcript (whisper.cpp).
- **App / window focus** — which application/window is foreground over time.

### The cinematic layer (parallel, human-facing)
Same capture, two consumers: veyo-core consumes pixels for the *index*; the cinematic layer consumes them for the *human video* — auto-zoom following cursor/clicks, easing, dwell, anti-jitter, backgrounds, padding, rounded corners, shadows, device mockups. These run in parallel; the cinematic output never feeds the index (the index is built from the raw capture, not the beautified render). See [features §4.2](features.md).

### Mechanism
`scap` (the MIT-licensed capture crate, same family Cap uses — safe to reuse, see [licensing.md](licensing.md)) provides cross-platform pixel + cursor capture. Frames go two ways: to veyo-core (CPU, on-device) and to the cinematic renderer. The realtime budget — keeping the veyo gate cheap enough to run alongside capture without dropping frames — is the hard engineering of this backend, which is why it ships *after* the browser backend.

---

## 4. The unified raw-session contract

Both backends (and import) emit a **raw session** with this shape, which is what veyo-core consumes:

```jsonc
{
  "source": "screen" | "browser" | "import",
  "media": { "video": "<path|null>", "audio": "<path|null>", "fps": 30, "resolution": [1920,1080] },
  "events": [            // empty for import; input events for screen; DOM/console/network for browser
    { "t": 12.4, "kind": "click", "x": 840, "y": 220 },
    { "t": 12.4, "kind": "dom_mutation", "summary": "…" },
    { "t": 13.0, "kind": "console_error", "text": "Uncaught TypeError…" },
    { "t": 13.1, "kind": "network", "status": 500, "url": "/api/checkout" }
  ],
  "frames_hint": "stream" | "screenshots" | "decode-from-video"
}
```

From here the pipeline is backend-agnostic: veyo-core gates → veyo-enrich enriches → CloakPipe redacts → store. The resulting `index.json` (see [index-schema.md](index-schema.md)) is **byte-for-byte the same shape** whether it came from a desktop demo, a browser QA flow, or a pasted Loom link. That invariant is the entire reason the agent surface ([mcp-api.md](mcp-api.md)) is simple.

---

## 5. Build order (and why)

1. **Import first** (Phase 1) — no capture at all; proves the headline and generates real sessions for veyo to tune on.
2. **Browser next** (Phase 2) — easy, exact, covers the highest-volume use case (web bug reports). DOM is cheap and the redaction story is best here.
3. **Screen last** (Phase 3) — the realtime path is the hardest; it waits until the codec gate and enrichment are proven on the easier sources.

See [phases.md](phases.md) for the full sequencing and gates.
