# clipxd — phases

> The detailed build plan: each phase's scope (in / out), what it delivers, and the gate that lets the next phase start. The arc and gate table are in [roadmap.md](roadmap.md); the rationale for the ordering is in [plan.md](plan.md).

The discipline: **each phase ships a usable thing and proves one risk.** Nothing tempting-but-heavy is pulled forward; everything deferred sits behind an interface that's stubbed now.

---

## Phase 1 — import + enrich (no capture) · *prove the index*

**Thesis it proves:** a recording can be turned into an object an agent queries from the URL — *without any capture code.*

**In**
- `clipxd-import`: fetch a remote video URL (Loom/Cap/arbitrary), demux audio + frames into the raw-session contract ([capture-backends §4](capture-backends.md#4-the-unified-raw-session-contract)).
- Pipeline wiring: raw session → veyo-core → veyo-enrich (whisper.cpp transcript, salient-frame captions, OCR) → `index.json`.
- `clipxd-mcp`: `query_clip`, `get_frame_context`, `search_text`, `get_events`, `get_transcript` over a single clip ([mcp-api.md](mcp-api.md)).
- `clipxd-web`: minimal share page + `/index.json` sidecar.
- Per-clip `.json` sidecar.

**Out (deferred behind stubs)**
- Any capture (screen or browser).
- CloakPipe pass (stub the interface; redaction is a no-op manifest for now).
- Cinematic layer, library FTS, hosting.

**Deliverable:** paste a Loom URL → get a queryable index + working MCP.

**Gate:** the [headline demo](overview.md#6-the-headline-demo) works reliably against imported clips on ≥3 real videos. Also: this phase **generates the first real sessions** veyo-core needs for tuning — a deliverable in its own right ([roadmap: the one dependency](roadmap.md#the-one-dependency-that-can-reorder-everything)).

---

## Phase 2 — browser backend · *cheap, exact capture for the common case*

**Thesis it proves:** for web flows, the DOM beats video, and it produces the *same* index shape as import.

**In**
- `capture-browser`: CDP/Playwright/rrweb-style driver capturing **DOM mutations + a11y tree + console + network**, with sparse salient screenshots ([capture-backends §2](capture-backends.md#2-the-browser-backend)).
- Normalizer: browser trace → raw-session contract (rich `event_track`: `console_error`, `network`, `dom_mutation`).
- DOM-verbatim `on_screen_text` (no OCR needed in this mode).
- Schema-identity tests: a browser clip and an import clip answer the same query shape.

**Out**
- Screen capture, cinematic layer (no pixels to beautify in pure browser mode).
- DOM-level CloakPipe redaction *enforcement* (interface present; full enforcement in Phase 4).

**Deliverable:** record a web bug-report flow → an index where an agent can read the console error, the failing request, and what was clicked right before.

**Gate:** a real QA/bug-report flow yields a correct agent answer; tests prove schema-identity with Phase 1.

---

## Phase 3 — owned recorder · *the hard part: realtime screen capture + cinematic*

**Thesis it proves:** we can own the Cap/Screen-Studio-class recorder experience *and* keep the realtime veyo gate cheap enough to run alongside it.

**In**
- `capture-screen`: `scap` (MIT) pixel + cursor/click/key/scroll capture, audio ([capture-backends §3](capture-backends.md#3-the-screen-backend)).
- Live feed into veyo-core on-device (the realtime path — the core engineering challenge).
- `clipxd-cinematic`: clean-room auto-zoom FSM (cursor/click follow, easing, dwell, anti-jitter), backgrounds, padding, corners, shadows, device mockups ([features §4.2](features.md#42-beautiful-recording--the-cinematic-layer--adoption-table-stakes)).
- Upload-while-recording → shareable link < 1s ([features §4.1](features.md)).
- OCR-based `on_screen_text` for the no-DOM case.

**Out**
- Any editor surface (timelines/transitions/effects) — permanent non-goal.
- Hosted tier.

**Deliverable:** hit record on a desktop demo → beautiful video + queryable index + instant local link.

**Gate:** record-to-link < 1s; auto-zoom is demo-grade (no jitter/motion-sickness); the screen-mode index matches browser-mode quality; the realtime gate runs within CPU budget without dropping frames.

---

## Phase 4 — CloakPipe pass + hosted optional tier · *make it safe to share*

**Thesis it proves:** clipxd can share an index that's provably free of PII/secrets — and can offer durable hosting without breaking the privacy thesis.

**In**
- CloakPipe in the pipe: redact transcript + frames + on-screen text before any index is shareable; emit the **redaction manifest** ([privacy-and-redaction.md](privacy-and-redaction.md)).
- Browser-mode **DOM-level redaction before screenshotting**.
- Optional **hosted tier**: durable links, object storage, CDN video — the open-core commercial layer ([overview §7](overview.md#7-sharing-model--the-honest-fork)).

**Out**
- Team admin / SSO beyond what a hosted tier minimally needs.

**Deliverable:** a clip containing an API key and a spoken card number ships fully masked + audited; a hosted link works while the recorder's machine is offline.

**Gate:** nothing un-redacted is ever served; the manifest is complete and accurate; a hosted link survives the origin box going down.

---

## Phase 5 — searchable library · *the corpus compounds*

**Thesis it proves:** many clips become more valuable than the sum — a queryable knowledge base of recordings.

**In**
- `clipxd-store` FTS5 over transcript + on-screen text + captions across **all** clips/imports/dictation ([features §4.7](features.md)).
- Library-wide `search_text` (clip id omitted → whole corpus) ([mcp-api §3](mcp-api.md#3-one-server-per-clip-or-one-for-the-library)).

**Out**
- Cloud-wide cross-user search (privacy thesis: local-first).

**Deliverable:** "find the clip where the checkout 500 happened" → ranked hits across everything.

**Gate:** a single query returns correct ranked results spanning all three sources.

---

## Phase 6 — (optional) reusable crates · *leverage*

**Thesis it proves:** parts of clipxd are valuable on their own.

**In (candidates)**
- Cinematic **auto-zoom as a standalone crate** usable without clipxd (open question — [risks](risks-and-open-questions.md)).
- Possibly: the import demux, the index schema crate as a public contract.

**Out**
- Anything that distracts from the core product before Phases 1–5 are solid.

**Gate:** a third party uses the auto-zoom crate without pulling in clipxd.

---

## Phase dependency at a glance

```
 Phase 1 (import) ──┬─► Phase 2 (browser) ──┐
   proves index     │     same schema        ├─► Phase 3 (screen) ─► Phase 4 (privacy+host) ─► Phase 5 (library) ─► Phase 6 (crates)
   feeds veyo  ◄─────┘                        │      hard realtime       safe to share          corpus            leverage
                                              (cinematic clean-room)
```

Every arrow is also a **gate**. The codec dependency (veyo recall) runs *underneath* all of them and is fed by Phase 1 onward ([roadmap](roadmap.md#the-one-dependency-that-can-reorder-everything)).
