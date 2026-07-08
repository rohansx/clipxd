# Teleprompter · voice-only · styled captions · camera filters — design

> Adds the "produced-presentation" layer to the recorder: a teleprompter you read while
> recording but that never lands in the capture; a voice-only mode; stylistic subtitles whose
> emphasis is decided by the Ollama-Cloud LLM **during indexing**; a clean/blur/replace
> background for the camera; and live camera filters. Written 2026-07-08, grounded in
> `app/src/Recording.tsx`, `app/src/useScreenRecorder.ts`, `crates/clipxd-recorder/src/pipeline.rs`,
> `crates/clipxd-web/src/lib.rs` (`spawn_phase2`), `crates/clipxd-web/src/llm.rs`,
> `docs/next-features.md` §3.3.

The governing filter from `features.md` applies to each item: anything that **improves the
index** is core; anything that only pleases a human viewer is accepted only when it is
**generated from the index** (the captions + emphasis work) or is a pure capture aid that
changes nothing about queryability (the teleprompter, voice-only, camera filters).

---

## 1. The five features, in one sentence each

1. **Transparent teleprompter** — a floating, semi-transparent script overlay the presenter
   reads while recording. It is a DOM element on the clipxd tab, **never composited into the
   recorded canvas**, so it does not appear in the capture (the canvas only draws screen +
   camera bubble — see `useScreenRecorder.ts`). Adjustable opacity, font size, scroll speed,
   mirror, and a reading-line highlight.
2. **Voice-only recording** — a capture mode that records the microphone only (no
   `getDisplayMedia`), producing a clip with `has_video: false` whose value is the transcript
   + styled captions. The same two-phase ingest/stage path; Phase 2 indexes the audio.
3. **Ollama-Cloud emphasis analysis (indexing-time)** — after enrichment, the transcript is
   sent to `llm.rs` (Ollama Cloud first, NVIDIA/Gemini fallback) to mark which words to
   **focus** on per segment. Output lands in a new optional `index.subtitle_emphasis` field.
   Runs inside `spawn_phase2` — exactly when the rest of the index is being built — never on
   the request path. Failure is logged-and-swallowed; the clip is already complete without it.
4. **Subtitle design selection (post-indexing)** — the user picks a caption *design*
   (Classic / Bold / Karaoke / Minimal / Boxed / Glow) on the clip page, with a live preview
   over the transcript. The choice is saved to a new optional `index.subtitle_style` field via
   `POST /clip/:id/subtitle-style`. The *designs themselves* are static presets; the
   *emphasis* that drives the Karaoke highlight is what the LLM produced at indexing time —
   so selection is meaningful only after indexing, matching the user's "after recording done
   and when indexing" requirement.
5. **Clean / blur / replace background + live camera filters** — for the camera bubble
   (and, in voice-only, the whole frame): Google-Meet-style **scene presets** (aurora, dusk,
   ocean, violet, noir, mint, warm, cool — curated gradients drawn behind a sharp camera
   inset), **custom image upload** (your own photo as the bubble backdrop), a blur halo, a
   solid, or a two-stop gradient — all WYSIWYG (drawn live on the preview and baked into the
   recorded canvas). Plus CSS-level live filters (brightness / contrast / saturate /
   grayscale / sepia / hue) applied to both the on-screen preview and the composited canvas.
   True portrait segmentation (person over scene) remains out of scope (AGPL references are
   forbidden) — these are "clean produced look" backgrounds behind a sharp camera inset.

---

## 2. Data model additions (schema v2 → additive, no breaking change)

Added to `clipxd-index::Index` (Rust) and mirrored in `app/src/types.ts`. Both are
`#[serde(default, skip_serializing_if = "Option::is_none")]` — old consumers and old clips
keep working unchanged.

```text
index.subtitle_emphasis: {
  generated_by: string,        // "ollama" | "nvidia" | "gemini"
  generated_at: string,        // RFC3339
  segments: [
    { start: f64, end: f64,
      words: [ { text: string, emphasis: "primary" | "secondary" | "none" } ] }
  ]
}

index.subtitle_style: {
  design: "classic" | "bold" | "karaoke" | "minimal" | "boxed" | "glow",
  font_scale: f32,              // 0.8 .. 1.6, default 1.0
  position: "bottom" | "center" | "top",
  emphasis: bool                // honor subtitle_emphasis in the render (karaoke/bold)
}
```

`subtitle_emphasis` is **server-produced** (Phase 2). `subtitle_style` is **user-produced**
(post-index selection) and is the only one of the two a write endpoint mutates.

---

## 3. Indexing-time integration — exactly where

`spawn_phase2` (`crates/clipxd-web/src/lib.rs`) already runs this sequence after the
Phase-1 commit:

```
enrich_clip / indexer.finalize   ← transcript + OCR + captions land
merge_browser_trace_into_clip    ← (browser mode)
auto-title + description         ← llm.rs, Ollama-first
optional deep pass (tl;dr/chapters)
mirror to S3
```

The new emphasis pass slots in **right after auto-title**, before the optional deep pass and
the final mirror — so the emphasis lands in the same index.json the mirror ships to S3. It
reuses `deeppass::build_context`-style transcript flattening and `llm::complete_with_keys`
(Ollama Cloud first), gated on `llm::any_backend_configured()` exactly like auto-title: off
locally with no key, on with a server- or owner-supplied key. Same BYOK path
(`owner_llm_keys`) so a user's emphasis analysis bills to their own Ollama/NVIDIA/Gemini
account.

This is the literal answer to "check how and when indexing is done, and at that time only
it should be doing its part": Phase 2 = `spawn_phase2`; the emphasis pass is one more
log-and-swallow step inside it, never on the request path, never blocking the clip becoming
watchable/shareable.

---

## 4. Teleprompter — not visible in the recording

`useScreenRecorder.ts` composites exactly two sources onto the recorded canvas: the
`getDisplayMedia` screen track and (optionally) the camera bubble. The teleprompter is a
React DOM node on the clipxd tab; it is **not** drawn to that canvas. Therefore:

- Recording **another window/tab**: the prompter is never captured (it lives in the clipxd tab).
- Recording **the clipxd tab itself**: the prompter could be visible. Mitigation: the
  prompter is a transparent overlay with a max opacity the user sets (default 55%), and a
  one-line note in its bar: *"not recorded unless you capture this tab"*. A future native
  (scap/PipeWire) recorder records a *display*, not a tab, so the prompter is invisible there
  by construction.

No capture-side change is needed for the common case; this is a UI feature only.

---

## 5. Voice-only mode

`useScreenRecorder.start()` gains an `audioOnly` path:

- `navigator.mediaDevices.getUserMedia({ audio: { echoCancellation, noiseSuppression, autoGainControl }, video: false })` — no `getDisplayMedia` prompt.
- `MediaRecorder` with `audio/webm;codecs=opus` (fall back to default audio mime).
- No canvas, no camera bubble, no cursor track (nothing to track).
- Same streaming stage upload (`/ingest/stage`, chunk PUTs, `/commit`) — the server treats
  the opus webm as the "video" file. `media::probe` + `extract_audio` handle audio-only
  containers; `extract_frames` yields no frames, so the visual gate is skipped and the clip
  is `has_video: false` with a transcript-only index. `stub_clip`/`promote_recording_stub`
  already set `has_video` from the probe; an audio-only probe sets it false.

Server-side tolerance: `enrich_clip` already `ensure!`s frames are non-empty and bails — for
audio-only we skip the gate/visual enrich and run only the transcript thread, then write the
index with `has_video: false`. (Small pipeline branch; documented as the one server change.)

---

## 6. Camera: clean / blur / replace background + live filters

Two independent knobs, both applied live to the preview **and** baked into the composited
canvas so WYSIWYG:

- **Background**: `none` (raw camera) | `blur` (gaussian blur of the camera frame, drawn behind
  a rounded rect — a "clean" look without ML segmentation) | `solid:<#hex>` | `gradient:<a>~<b>`.
  Blur is implemented on the canvas (downsample + box-blur the camera frame, draw it blurred
  as the bubble backdrop, then draw the sharp camera on top inset) — no shader, matches the
  `recorder-feature-catalog.md` §C "Average-color pixelation per block on ImageData (no
  shader)" clean-room approach.
- **Filters**: a CSS `filter` string (`brightness() contrast() saturate() grayscale() sepia()
  hue-rotate()`) applied to the preview `<video>` and to the canvas via `ctx.filter` (supported
  in Chromium/Firefox) before `drawImage` — so the baked-in bubble matches the preview.

True ML background *replacement* (portrait segmentation) is explicitly out of scope here
(AGPL references forbid it); the "clean" replacement is a solid/gradient fill behind the
bubble, which already reads as a produced look.

---

## 7. Subtitle design selection

A `SubtitleStyle` panel on `ClipPage` (below the editor controls when a transcript exists):

- Design chips: Classic · Bold · Karaoke · Minimal · Boxed · Glow.
- font-scale slider, position (bottom/center/top), emphasis toggle.
- Live preview: renders the current transcript segment over the video using the chosen
  design; for Karaoke/Bold+emphasis, words tagged `primary`/`secondary` in
  `subtitle_emphasis` are highlighted as they're spoken.
- Save → `POST /clip/:id/subtitle-style` → `index.subtitle_style` (owner-gated, same
  `require_clip_access` as `set_cursor`/`local-captions`).

The render path (`clipxd beautify --captions`) **now consumes** `subtitle_style` +
`subtitle_emphasis` to burn styled captions into the MP4 — 6 designs, per-word emphasis colouring,
Karaoke word-by-word lighting, boxed/glow/shadow, position + font-scale, all wired through
`POST /clip/:id/render?captions=true` and the `RenderOpts.captions` flag the SPA sets.

---

## 8. Acceptance

- A recording made with the teleprompter open and recording **another window** contains no
  teleprompter text; the prompter's opacity slider visibly dims it for the presenter.
- A voice-only recording produces a clip with `has_video: false`, a transcript, and no video
  element on the clip page; the share page shows the styled captions.
- With an Ollama Cloud key set, a freshly indexed clip's `index.json` contains
  `subtitle_emphasis` with per-word emphasis within ~10s of enrichment completing; with no
  key, the field is absent and the clip is otherwise identical.
- Selecting "Karaoke" + emphasis on a clip with emphasis data highlights the spoken focus
  words in the live preview; saving persists `subtitle_style` and survives a refresh.
- A camera bubble with `blur` background + a warm filter renders identically in the recorded
  MP4 and the on-screen preview.