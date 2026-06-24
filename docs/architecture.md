# clipxd — architecture

> The system: a **thin owned recorder** in front of a **fat index pipeline**. Capture is small and fast; the value is everything downstream that turns a session into an agent-queryable object. For *what each stage produces*, see [index-schema.md](index-schema.md); for *how an agent reads it*, see [mcp-api.md](mcp-api.md).

---

## 1. The pipeline (end to end)

```
 SOURCE ─────────────┐
  ① recorder (screen) │   raw session = video + event-track + audio
  ② recorder (browser)│ ─────────────────────────────┐
  ③ import-from-URL    │   (video only, no events)     │
                       ┘                               ▼
                                          ┌──────────────────────────┐
                                          │  veyo-core (salience gate)│  CPU · on-device
                                          │  decide WHICH moments matter│
                                          └─────────────┬─────────────┘
                                       salient timestamps + deltas
                                                        ▼
                                          ┌──────────────────────────┐
                                          │  veyo-enrich              │
                                          │  · whisper.cpp transcript │
                                          │  · caption salient frames │
                                          │  · OCR on-screen text     │
                                          │  · structure event-track  │
                                          └─────────────┬─────────────┘
                                              raw index (sensitive)
                                                        ▼
                                          ┌──────────────────────────┐
                                          │  CloakPipe pass           │  redact PII/secrets
                                          └─────────────┬─────────────┘
                                               clean index
                                                        ▼
                                          ┌──────────────────────────┐
                                          │  store: video + index.json│  SQLite + FTS + blob
                                          └─────────────┬─────────────┘
                                                        ▼
                              ┌─────────────────────────┴─────────────────────────┐
                              ▼                                                     ▼
                   MCP server (agents)                              share page + JSON API (humans / tools)
            query_clip · get_frame_context ·                  watchable video + sidecar `.json`
            search_text · get_events                          behind the same URL
```

Three properties fall out of this shape:

1. **Async by construction.** The share link goes live the instant capture stops (upload-while-recording). Enrichment runs *behind* the live link — the user never waits on the agent index. The index "fills in" within seconds.
2. **No imagery leaves the device until CloakPipe has run.** veyo-core runs on CPU on-box; enrichment can run local or be routed; CloakPipe always runs *before* anything is shareable. See [privacy-and-redaction.md](privacy-and-redaction.md).
3. **One schema, many sources.** Screen capture, browser capture, and URL import all converge to the same `index.json`. The agent surface is identical regardless of origin.

---

## 2. Component responsibilities

| Component | Owns | Does **not** own | Status / source |
|---|---|---|---|
| **clipxd-recorder** | screen/browser capture, cinematic layer, share handoff | enrichment, codec, redaction | OWNED, THIN (Rust) |
| **clipxd-import** | fetch a remote video URL, normalize to a raw session | capture | OWNED (Rust) — ships Phase 1 |
| **veyo-core** | salience/habituation gate; delta schema; "which moments matter" | enrichment, capture | external engine ([veyo](../../veyo)) |
| **veyo-enrich** | transcript, frame captions, OCR, event structuring | gating decisions | external engine ([veyo](../../veyo)) |
| **CloakPipe** | detect + mask PII/secrets in frames + transcript | recording, indexing | sibling ([cloakpipe](../../cloakpipe)) |
| **clipxd-store** | persist video + `index.json`; FTS over the library | gating, capture | OWNED (Rust) |
| **clipxd-mcp** | expose the index to agents over MCP | storage internals | OWNED (Rust) |
| **clipxd-web** | share page (video player) + JSON API + sidecar | enrichment | OWNED (small Rust service) |

**The hard rule** (repeated because it is the whole discipline): the recorder owns *capture + cinematic + share handoff* and nothing else. Anything that improves only the human's viewing experience and does nothing for the index is out of scope (see [overview §8](overview.md#8-non-goals-holding-the-line)).

---

## 3. Crate / workspace layout

Single Rust toolchain, single workspace; clipxd consumes veyo as **path deps** (`workspace.dependencies` → `../veyo/crates/...`), so it shares veyo-core's types directly — no FFI boundary.

```
clipxd/
  crates/
    # ── Phase 1 (BUILT, tested, demo-proven) ───────────────────────────────
    clipxd-index/       # the schema crate — index.json + the query surface; single source of truth
    clipxd-import/      # URL/file → demux → veyo gate → veyo-enrich → index.json
      downscale.rs      #   RGBA frame → veyo 8×8 luma Cell grid (CELLS, not pixels)
      media.rs          #   yt-dlp fetch · ffprobe · ffmpeg frame+audio demux
      gate.rs           #   run veyo-core Codec over frames; RETAIN salient frames
      map.rs            #   veyo-enrich Enrichment → clipxd Index
    clipxd-cli/         # `clipxd import | query | search | info`
    clipxd-mcp/         # MCP server (query_clip · search_text · get_frame_context · get_events · get_summary)
    # ── Phase 2/3/5 (PLANNED) ──────────────────────────────────────────────
    clipxd-recorder/    # screen + browser capture (scap MIT + CDP/rrweb)        — Phase 2/3
    clipxd-cinematic/   # auto-zoom FSM, backgrounds, mockups (clean-room)        — Phase 3
    clipxd-store/       # SQLite + FTS5 library across all clips                  — Phase 5
    clipxd-web/         # share page + JSON API sidecar (axum)                    — Phase 1.5
  (path deps, versioned independently)
    veyo-core      # the salience codec (../veyo) — consumed as-is
    veyo-enrich    # the meaning layer: transcript · OCR · caption (../veyo) — BUILT in this work
    cloakpipe      # redaction (../cloakpipe) — wired in Phase 4 (field stubbed in the schema now)
```

`clipxd-index` is the **schema crate** — it defines the `index.json` shape once and every other crate depends on it; changing the agent-facing contract means changing exactly one crate ([index-schema.md](index-schema.md)). **`veyo-enrich` did not exist** when these docs were first written (veyo deferred it to its own Phase 3); it was built upstream in veyo as part of standing clipxd up, so the codec stays a pure codec and the meaning layer is a separate, swappable crate.

---

## 4. Data flow detail — the three sources

### 4.1 Screen capture (the realtime path, hardest)
`capture-screen` (scap, MIT) pulls frames + cursor/click/key events. Frames stream **into veyo-core on the same box**, which emits salient timestamps + deltas. The cinematic layer renders the human-facing video in parallel. Only salient frames are handed to enrichment — captioning every frame is the ~2.6B-tokens/day non-starter veyo exists to avoid.

### 4.2 Browser capture (the high-volume path, easiest)
`capture-browser` records the **DOM mutation stream + accessibility tree + console + network**, with sparse screenshots only at salient points. For the browser, the DOM *is* the cheaper, exact, more legible primitive — captioning pixels is the wrong move. (rrweb-style DOM capture is well-trodden; its known weakness — animation-heavy pages and replay-time staleness — is mitigated because clipxd snapshots salient screenshots and stores them, rather than relying on later DOM replay.) See [capture-backends.md](capture-backends.md).

### 4.3 Import-from-URL (the traction path — BUILT, Phase 1)
`clipxd-import` fetches an existing Loom/Cap/arbitrary video (yt-dlp for URLs), probes it (ffprobe), and demuxes frames + audio (ffmpeg). Each frame is **downscaled to veyo's 8×8 luma `Cell` grid** (`downscale.rs`) and fed to `veyo-core`'s `Codec::observe` — veyo decides which moments are salient and emits deltas. **veyo-core deliberately discards pixels** (its `evidence` is `#[serde(skip)]`, local-only), so clipxd **retains the salient frames itself** (`gate.rs`) and hands them to `veyo-enrich` for OCR (tesseract) + captioning; audio goes to the transcriber (whisper.cpp, pluggable; null by default). The resulting `Enrichment` is mapped to `index.json` (`map.rs`). No event track exists for imported video, so that stream is empty — everything else is identical to a captured clip. This is why Phase 1 needs **zero capture code**, and the imported clips double as the real sessions veyo's gate needs for tuning.

> **Two facts the early drafts got wrong, now corrected:** (1) veyo eats *cells, not pixels* — clipxd does the RGBA→Cell downscale; (2) enrichment (transcript/OCR/caption) is **not** something veyo handed us — it didn't exist, so it was built as `veyo-enrich` and clipxd retains the salient frames it needs.

**Degrade mode.** While veyo's gate is formally unproven (target: recall ≥0.9 @ emission <1% on ≥3 real sessions), `clipxd import --salience-min <x>` lowers the codec's emission floor to caption more densely at higher cost — the *schema is unchanged*. The default uses veyo's tuned floor (0.4). This is the [plan §5](plan.md#5-codec-gated-branch-the-one-real-fork-in-the-plan) codec-gated branch made real.

---

## 5. Where the work runs (trust boundaries)

```
   ON-DEVICE (always)                 │  ROUTED (optional, user-controlled)      │  SHAREABLE (post-CloakPipe)
   ─────────────────────────────────  │  ──────────────────────────────────────  │  ───────────────────────────
   capture · veyo-core gate ·         │  heavy frame captioning / large-model     │  index.json · video file ·
   whisper.cpp (local) · OCR (local) ·│  enrichment may be routed to a backend    │  MCP responses · share page
   CloakPipe redaction                │  ONLY after CloakPipe has cleaned input    │
```

The boundary is non-negotiable: **raw imagery and un-redacted transcript never cross the "routed" line.** veyo's design ("no imagery leaves the box") and CloakPipe's redaction are what make a routed-enrichment or hosted tier safe to offer. Detail in [privacy-and-redaction.md](privacy-and-redaction.md).

---

## 6. Storage model

- **Blob:** the video file (and salient screenshots) on local disk (or object storage in the hosted tier).
- **Index:** `index.json` per clip — the canonical artifact.
- **Library:** SQLite with FTS5 over transcript + OCR text + captions, one row per clip, so "find the clip where the 500 error showed up" is a single query across everything (see [features §4.7](features.md)).

A clip is therefore *self-describing*: `{ video, index.json }` is portable; copy it anywhere and the MCP server can serve it.

---

## 7. Deployment topologies

| Topology | Recorder | Store | Serve | Use |
|---|---|---|---|---|
| **Local-only (default)** | local | local disk | `clipxd serve` on localhost | agent-on-my-machine; ephemeral peer view via tunnel |
| **Self-host** | local | own server | own box + reverse proxy | team-internal durable links |
| **Hosted (commercial, later)** | local | object storage | managed | durable async human sharing, CDN video |

The recorder is identical across all three; only *where the artifact lives and who serves it* changes. The local-first default is the thesis-pure mode; hosting is the open-core commercial layer (see [licensing.md](licensing.md)).

---

## 8. Failure modes & degradation

- **Enrichment fails / is slow:** the share link still works as a plain video; the index is marked `partial` and back-fills. The human path never blocks on the agent path.
- **veyo-core not yet at target recall:** clipxd can over-emit (caption more frames) at higher cost, or fall back to fixed-interval sampling, until the codec's gate is proven. The schema is unchanged. (This is the Phase-1-vs-codec gate in [risks-and-open-questions.md](risks-and-open-questions.md).)
- **CloakPipe flags something:** the redaction is applied and recorded in the index's redaction manifest; the index is shareable. A clip is never shared with un-redacted content silently.
- **Import source unreachable / DRM'd:** import fails loudly with a clear error; nothing partial is stored.
- **OCR/transcription engine absent:** backends are pluggable with no-op defaults — `NullOcr`/`NullTranscriber` keep the pipeline valid (a transcript-less index is still useful). OCR auto-detects the system `tesseract`; transcription defaults to off until a whisper.cpp binary is wired. Missing language data (`eng.traineddata`) is the one external dependency for OCR.

---

## 9. Phase 1 — as built & verified

Phase 1 is implemented and **proven end to end**, through both the CLI and the MCP agent surface.

**What runs today:**

```
 clipxd import <url|file>            clipxd query <clip> "<question>"        clipxd-mcp <clip>
   yt-dlp · ffprobe · ffmpeg            search_text · get_frame_context          MCP/stdio: query_clip,
   → veyo-core gate (Cells)             → query_clip (grounded, cited)            search_text, get_frame_context,
   → veyo-enrich (tesseract OCR             ▲                                      get_events, get_summary
     + heuristic caption)                   └── reads index.json, never the video ──┘
   → index.json
```

- **Backends wired:** OCR = system `tesseract` (auto-detected, TSV → per-line bbox + confidence); captioner = `HeuristicCaptioner` (grounds the caption in nearby OCR text, offline, no VLM); transcriber = `NullTranscriber` by default (whisper.cpp is a one-box swap). All three are traits — see [veyo-enrich](../../veyo/crates/veyo-enrich).
- **The verified demo:** a synthetic checkout video (normal → "Processing…" → a 500 error banner) is imported; the veyo gate emits deltas only at the two real transitions; OCR reads the on-screen text; then —

  > `clipxd query <clip> "what error showed up and what was the user doing right before it"`
  > → *"At 9.0s, the on screen text shows: 'ERROR: Payment failed (500)'. Just before, at 7.8s: … 'Processing your payment…'"* — cited 9.0s, 7.8s.

  The same answer comes back over MCP (`tools/call query_clip`). The agent never touched the video — exactly the [headline demo](overview.md#6-the-headline-demo).
- **Tests:** `clipxd-index` (schema round-trip + the query/headline logic), `clipxd-import` (downscale + url/title parsing), and `veyo-enrich` (TSV + whisper-JSON parsers, caption grounding, the enrich orchestration) are unit-tested and green.
- **Also built — Phase 2 browser backend** (`clipxd-browser`): a browser-trace ingestor (DOM/console/network/a11y → the *same* `Index`, `source:"browser"`), with a browser-salience model (the gesture→request join, coalescing, novelty/habituation) and DOM-verbatim `on_screen_text`. Proven on a real Playwright capture; masked field values never reach the index. Spec: [phase2-browser-spec.md](phase2-browser-spec.md). Detail: [capture-backends §2](capture-backends.md#2-the-browser-backend).
- **Not yet (deferred by design):** CloakPipe is a stubbed manifest (Phase 4); there is no **screen** capture backend yet (Phase 3 — `scap` + clean-room cinematic); the searchable library and share-page/sidecar service are Phase 5 / 1.5. None are needed for the headline.
