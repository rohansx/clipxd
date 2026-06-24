# clipxd — the clip index schema

> **This is the product.** Everything else — the recorder, the cinematic layer, the share page — is plumbing that produces or serves *this object*. The `index.json` is the artifact an agent queries. It is the single source of truth that the [MCP server](mcp-api.md) and JSON API sit on top of, and it is defined once in the `clipxd-index` crate ([architecture §3](architecture.md#3-crate--workspace-layout)).

The governing principle: **an agent should never need the pixels. If it does, the index failed.** Every field below exists so that a question about the video can be answered from text.

---

## 1. Top-level shape

```jsonc
{
  "clipxd_version": "1",
  "id": "clp_8f2a…",
  "source": "screen" | "browser" | "import",
  "status": "complete" | "partial" | "enriching",   // partial = some enrichers failed; enriching = still filling in

  "metadata":        { /* §2 */ },
  "transcript":      [ /* §3 */ ],
  "visual_timeline": [ /* §4 */ ],
  "on_screen_text":  [ /* §5 */ ],
  "event_track":     [ /* §6 */ ],
  "summary":         { /* §7 */ },
  "redaction":       { /* §8 */ }
}
```

All timestamps (`t`, `start`, `end`) are **seconds from clip start**, floats. Every enriched item carries the timestamp(s) it applies to — the index is fundamentally a **time-indexed bundle of streams**, and queries are almost always "what was true at / between time(s) T."

---

## 2. `metadata`

```jsonc
"metadata": {
  "duration": 184.2,
  "resolution": [1920, 1080],
  "fps": 30,
  "created_at": "2026-06-23T18:30:00Z",
  "title": "Checkout flow — 500 on submit",     // AI-derived, human-editable
  "app_focus": [                                  // which app/window was foreground over time (screen mode)
    { "start": 0.0,  "end": 42.1, "app": "Arc",      "window": "app.example.com — Checkout" },
    { "start": 42.1, "end": 184.2,"app": "Terminal", "window": "zsh" }
  ],
  "url_context": "https://app.example.com/checkout",  // browser mode: the page(s) involved
  "has_video": true
}
```

---

## 3. `transcript`

Time-aligned, speaker-tagged where possible (whisper.cpp, local — see [tech-stack.md](tech-stack.md)).

```jsonc
"transcript": [
  { "start": 1.2, "end": 4.8, "speaker": "spk_0", "text": "Okay, so I click submit on the checkout…" },
  { "start": 5.0, "end": 7.1, "speaker": "spk_0", "text": "…and it just spins, then throws an error." }
]
```

- `speaker` is best-effort; absent or `null` when diarization isn't confident.
- Text is **post-CloakPipe** — a spoken credit-card number is already masked here (§8).

---

## 4. `visual_timeline` — the veyo-gated heart

Captions for **salient moments only**. This is where veyo's salience gate is load-bearing: captioning every frame is the ~2.6B-tokens/day non-starter ([overview §2](overview.md#2-veyo-is-the-engine-clipxd-is-the-product)). Each entry is a moment veyo judged worth enriching.

```jsonc
"visual_timeline": [
  {
    "t": 13.0,
    "salience": 0.91,                 // veyo-core's score for why this moment was emitted
    "caption": "A red error toast reads 'Payment failed (500)'. The submit button is disabled.",
    "delta": "error_toast_appeared",  // veyo's structured delta kind
    "frame_ref": "frames/0013.jpg"    // salient screenshot, if stored (post-redaction)
  },
  {
    "t": 12.4,
    "salience": 0.74,
    "caption": "Cursor clicks the 'Place order' button; a loading spinner replaces the label.",
    "delta": "state_settle",
    "frame_ref": null
  }
]
```

- Ordered by salience or time depending on query; stored time-ascending.
- `frame_ref` points at a redacted screenshot — the *only* pixels in the index, and only at salient points.
- If veyo-core is not yet at target recall, this stream degrades gracefully to denser/fixed-interval captions at higher cost — the *shape* is unchanged ([architecture §8](architecture.md#8-failure-modes--degradation)).

---

## 5. `on_screen_text`

Searchable, timestamped text that appeared on screen — errors, code, labels, URLs. **OCR** in screen mode; **DOM/a11y-verbatim** in browser mode (exact, no OCR guessing).

```jsonc
"on_screen_text": [
  { "start": 13.0, "end": 18.4, "text": "Uncaught Error: Payment failed (status 500)", "source": "ocr",  "bbox": [320,210,980,240] },
  { "start": 42.1, "end": 60.0, "text": "POST /api/checkout 500 (Internal Server Error)", "source": "dom", "selector": ".console-row.error" }
]
```

- `source` distinguishes OCR (with `bbox`) from DOM (with `selector`) so consumers know the confidence.
- This stream is what makes `search_text` ([mcp-api.md](mcp-api.md)) possible and what feeds the library FTS ([features §4.7](features.md)).

---

## 6. `event_track`

The interaction stream. Input events in every mode; **rich web events in browser mode**.

```jsonc
"event_track": [
  { "t": 12.4, "kind": "click",          "x": 840, "y": 220, "target": "button#place-order" },
  { "t": 12.5, "kind": "key",            "keys": "Enter" },
  { "t": 13.0, "kind": "console_error",  "text": "Uncaught TypeError: cannot read 'id' of undefined", "stack": "at checkout.js:88" },
  { "t": 13.1, "kind": "network",        "method": "POST", "url": "/api/checkout", "status": 500, "ms": 1840 },
  { "t": 13.2, "kind": "dom_mutation",   "summary": "error toast node inserted into #notifications" }
]
```

Event `kind`s:

| kind | modes | notes |
|---|---|---|
| `click` / `key` / `scroll` / `move` | all | input events; `target` present in browser mode |
| `focus_change` | screen | app/window focus switched |
| `console_log` / `console_error` | browser | with `stack` when available — **the bug-report gold** |
| `network` | browser | method, url, status, timing |
| `dom_mutation` | browser | summarized; the spine of browser-mode time |

The event track is what lets an agent answer *"what was the user **doing** right before the error"* — the second half of the headline demo.

---

## 7. `summary`

Derived convenience, explicitly **not the source of truth** (an agent that needs ground truth reads the streams above).

```jsonc
"summary": {
  "tldr": "User attempts checkout; the POST /api/checkout returns 500 and a 'Payment failed' toast appears.",
  "chapters": [
    { "start": 0.0,  "title": "Setup — opening the checkout page" },
    { "start": 12.0, "title": "Submitting the order" },
    { "start": 13.0, "title": "The 500 error" }
  ]
}
```

---

## 8. `redaction` — the manifest

CloakPipe ran; this records *what it did*, so redaction is auditable rather than silent ([privacy-and-redaction.md](privacy-and-redaction.md)).

```jsonc
"redaction": {
  "ran": true,
  "engine": "cloakpipe",
  "version": "…",
  "items": [
    { "stream": "transcript",     "t": 30.2, "entity": "CREDIT_CARD", "action": "masked" },
    { "stream": "on_screen_text", "t": 55.0, "entity": "API_KEY",     "action": "masked" },
    { "stream": "visual_timeline","t": 55.0, "entity": "API_KEY",     "action": "frame_blurred" }
  ],
  "policy": "default-strict"
}
```

- Every other stream in the index is **already post-redaction** — the manifest is the receipt, not a to-do list.
- `frame_blurred` records that a stored screenshot had a region obscured before storage.

---

## 9. Invariants (what consumers can rely on)

1. **Schema-identity across sources.** Screen, browser, and import produce the *same shape*. The only difference is which streams are populated (import has an empty `event_track`; browser has rich web events; screen has OCR + input events). Tests assert this.
2. **Everything served is post-redaction.** If `redaction.ran` is true, every stream is clean. The manifest says what changed.
3. **Time is the universal key.** Every enriched fact carries its timestamp(s). The MCP/JSON surface is "query by time or by text," nothing more exotic.
4. **The index is self-contained and portable.** `{ video, frames/, index.json }` can be copied anywhere; the MCP server can serve a clip from just those files ([architecture §6](architecture.md#6-storage-model)).
5. **`status` is honest.** `enriching`/`partial` mean some streams are still filling in or an enricher failed; consumers should not treat a partial index as complete.

This contract is deliberately small. A small, stable schema is what keeps the agent surface ([mcp-api.md](mcp-api.md)) simple and what lets the recorder, backends, and enrichers all evolve behind it.
