# clipxd — the best agent-native recorder: feature catalog + clean-room build plan

> **Thesis.** Cap, Loom, Screenity, OpenScreen, Recordly, Screenize, BetterCapture, ShareX are all racing on the *same* axis: prettier pixels (auto-zoom, cursor physics, mockups, captions). clipxd matches that table-stakes layer, then wins on the axis none of them touch: **every recording is an agent-queryable index**. The cursor/click/keystroke track that everyone else throws away after rendering is, for us, the *primary product* — it becomes structured `events`, drives the cinematic camera, and is served over MCP so an agent reasons about a clip from text, without the video. That moat is already built (`clipxd-index`, `clipxd-mcp`, `clipxd-import`, `clipxd-browser`, `clipxd-cinematic`). The recorder is the last missing limb.

---

## 1. Master feature catalog (union of the best, with clean-room approach)

Legend — **Effort:** S ≤ 3d · M ≈ 1wk · L ≈ 2–4wk · XL > 1mo. **Ref** = the tool that does it best. **License of ref** drives whether we depend or clean-room.

### A. Capture modes

| Feature | One-line spec | Best ref | Clean-room / reuse approach | Effort |
|---|---|---|---|---|
| Full-screen capture | `capture(Display) → Frame stream` w/ timestamps, pause/resume | BetterCapture (SCK), OpenScreen (multi-native) | **Depend** on MIT `scap-screencapturekit` (mac) / `scap-direct3d` (win) / `scap-ffmpeg`. Linux = clean-room PipeWire (below). | M |
| Window capture | Pick a window, isolate from rest of desktop | BetterCapture, Screenize | `scap-targets` (MIT) `Window::list`; SCK `SCContentFilter` / D3D `GraphicsCaptureItem`. | S |
| Region capture | Drag a rect; clamp to even pixels; live preview of bounds | ShareX, BetterCapture | Overlay rect in React; pass `sourceRect` (top-left origin) to scap. Even-pixel clamp `round(d/2)*2` (H.264 req). | M |
| Webcam / PiP | Separate cam stream, composited bubble (circle/squircle, mirror, shadow) | Recordly, OpenScreen | **Depend** MIT `cap-camera*` (AVF/MF/DShow/v4l2) for *enumeration+frames only*. Compositing = our renderer (squircle = quarter-arc math). `cap-camera-effects` is AGPL → not used. | M |
| Microphone audio | Mic track w/ echo-cancel / noise-suppress / AGC profiles, gain | OpenScreen, Recordly | **Depend** MIT `scap-cpal`/`cpal`. Preprocessing profiles (`raw`/`no-agc`/`processed`) are our config enum. | S |
| System audio | Loopback capture, muxed as a 2nd track | BetterCapture, Recordly | mac: SCK `capturesAudio`; win: WASAPI loopback; Linux: PipeWire monitor node. Keep mic+system as **separate tracks** (BetterCapture pattern). | M |
| HDR / pro codecs | HEVC/ProRes 422-4444, alpha, per-frame BT.2020+PQ colorimetry | BetterCapture | Clean-room: `CVBufferSetAttachment` per-frame `colr`/`nclx` for ProRes (forbidden as container key). Enum-driven codec×container matrix. Mac-only, deferrable. | L |
| VFR capture | Write frames at real timestamps, no dup/drop | Screenize (SCRecordingOutput) | Honour native timestamps in our muxer instead of fixed-fps resample. | S |

### B. Cursor effects + cinematic auto-zoom (the "beautify" layer — **partly BUILT**)

| Feature | One-line spec | Best ref | Clean-room approach | Effort |
|---|---|---|---|---|
| Cursor event capture | 16ms poll: pos/buttons/keys → normalized [0,1], incremental JSON flush | Cap (behavior), Screenize | `device_query` tick; raw→normalized; button-state diff; keycode→label. **This is also the agent event track** — dual-purpose. | M |
| Auto-zoom (click-driven) | Cluster clicks → 3-phase zoom region: ease-in / hold / ease-out | screenarc, OpenVid, Screenize | **BUILT** in `clipxd-cinematic::compute_zoom_track`: `easeOutQuart` entry/exit, `easeInOutQuart` focus, zoom→[1.2,4.0], pre/post offsets (1.0s/0.9s), EMA pan smoothing. Pure uncopyrightable math. | **done** |
| Spring cursor-follow | Velocity-continuous pan via per-axis spring (no wobble on reversal) | OpenScreen, Recordly, Screenize | Clean-room damped-harmonic solver (analytic, stable for ζ≷1) + **dead-zone targeting** (move only when cursor exits radius — kills jitter). | M |
| Intent-aware planning | Classify typing/click/drag/scroll → semantic zoom waypoints | Screenize, Recordly | Rule thresholds (typing 1.5s gap, nav 2.0s window, idle 5.0s) over the event timeline we already own. Layers onto BUILT zoom engine. | M |
| Temporal motion blur | Sample N frames across a shutter window, weighted composite | Recordly | Render shutter (18–300% frame-time, 3–61 odd samples), power-curve opacity weights. Render-time only. | M |
| Custom cursor render | Resolution-independent vector cursors, themes, click bounce/ripple | Screenize, Recordly | Draw cursors via paths (sharp at any zoom), LRU cache by (style,height); click bounce = spring scale `sqrt(zoom)` normalized. | M |
| Keystroke overlays | `⌘C` pills rendered + cached, opacity per-frame | Screenize | Rasterize pill once, cache by text, apply opacity via color-matrix. Feeds keystrokes we already capture. | S |
| Path smoothing | Catmull-Rom / EMA de-jitter of recorded cursor | OpenScreen | Spline interpolation post-capture; smoothing 0–100 = blend ratio. | S |

### C. Annotation / drawing

| Feature | Spec | Best ref | Clean-room approach | Effort |
|---|---|---|---|---|
| Live drawing | Pen/arrow/rect/ellipse/text/highlight during record, undo/redo | ShareX (18 shapes), Screenity | React overlay canvas; immutable `Shape` region model + memento undo. Region objects = same non-destructive model as edits. | L |
| Post overlays | Text/arrow/image annotations on a timeline w/ animations | OpenScreen, Recordly | `AnnotationRegion{type,t0,t1,pos%,style}`; text anims (fade/rise/pop/typewriter) = easing on transform/opacity over ~700ms. | M |
| Blur / redaction | Mosaic any region; toggle live or post | OpenScreen, Screenity | Average-color pixelation per block on `ImageData` (no shader). **Also wires to index `Redaction`/`RedactionItem` — already in schema.** | S |

### D. Trim / editor / timeline

| Feature | Spec | Best ref | Clean-room approach | Effort |
|---|---|---|---|---|
| Trim / cut | `TrimRegion{startMs,endMs}`; decoder skips dead ranges | OpenScreen, Recordly | Output-time→source-time map; decoder seeks past trims. | S |
| Speed regions | Per-segment slow-mo / fast-fwd, audio resampled | Recordly, OpenScreen | `SpeedRegion` stretches output frames; resample audio at adjusted rate. | M |
| Crop / resize | Normalized crop rect baked at compositing | OpenScreen | Canvas transform in frame renderer. | S |
| Multi-track timeline | Drag regions (zoom/trim/speed/annotation/audio), snap guides, waveform | Recordly, OpenScreen, Screenize | Immutable region data model → `.clipxd` JSON project. Waveform = worker RMS down-sample. Undo = immutable snapshots. | L |
| Project persistence | Save edit state + media refs, reopen | Screenize (`.screenize` pkg), Recordly | Package = `project.json` + `recording/` (video, `events.json`, audio); **relative paths** resolved at load. Our `Index` *is* the metadata core. | M |

### E. Backgrounds / padding / mockups

| Feature | Spec | Best ref | Clean-room approach | Effort |
|---|---|---|---|---|
| Backgrounds | Solid / linear-radial-conic gradient / wallpaper / image | OpenVid (100+), screenarc | Parse gradient CSS (angle, stops); draw bg-first then video. | M |
| Padding / corners / shadow | Scale to canvas baseline | OpenVid, Recordly | `scaledPad=(pad·0.5/100)·dim`, `radius=corners·(w/896)`, `shadowBlur=shadow·(w/896)·0.3`. | S |
| 2D device mockups | Safari/Chrome/Arc/macOS/iPhone frames via canvas primitives | OpenVid | `save→scale→arc/quadCurve→restore`, scale `(w/1280)·1.2`; `BOTTOM_ONLY_RADIUS` / `SELF_SHADOWING` sets. **Need our own permissive frame assets.** | M |
| 3D perspective / mockups | Plane w/ perspective, rotateX/Y eased across phases | OpenVid (Three.js) | `cameraZ=(2·perspPx)/1080`, `fov=2atan(1/cameraZ)`, `maxRot=32°·intensity/100`, supersample 4×. wgpu or WebGL. | L |

### F. Export

| Feature | Spec | Best ref | Clean-room approach | Effort |
|---|---|---|---|---|
| MP4 (HW) | H.264/HEVC via HW encoder; bitrate by res×fps | OpenScreen, BetterCapture | **Depend** ffmpeg (LGPL dynamic) / VideoToolbox / MF. Bitrate = `px·fps·factor` clamp 4–24Mbps, or table (4K 45, QHD 28, 1080p 18). | M |
| WebM / alpha | VP8/VP9, yuva420p | OpenVid | ffmpeg `libvpx -vf format=yuva420p`. | S |
| GIF | Palette-gen + dither | OpenVid, OpenScreen, ShareX | `palettegen`→`paletteuse dither=bayer`; reduce fps to size target. | S |
| fMP4 streaming | 1s independently-playable fragments → crash recovery | Screenity | moov-first then moof+mdat; buffer ~1MB/1s writes. Resume = scan highest chunk key. | M |

### G. Sharing / upload / instant link

| Feature | Spec | Best ref | Clean-room approach | Effort |
|---|---|---|---|---|
| Instant share link | On-stop chunked multipart upload, link in seconds | Cap (instant mode) | 5–16MB parts, ≤3 concurrent presigned PUTs, backoff retry, `/complete` returns URL. Behavior-only; Cap code is AGPL. | L |
| Share page | Hosted player + the **agent index** beside the video | (none — our moat) | React share page renders clip + exposes `query_clip`/`search_text` panel and MCP endpoint. Closed hosted tier. | L |
| Cloud destinations | Drive/S3 w/ OAuth | Screenity, ShareX | Pluggable uploader trait (S3/Drive/custom HTTP), presigned + Content-Range. | M |

### H. Productivity surface

| Feature | Spec | Best ref | Clean-room approach | Effort |
|---|---|---|---|---|
| Global hotkeys | Start/stop/region without focus | ShareX | mac `CGEventTap`, win `RegisterHotKey`+hidden WndProc, Linux portal/global shortcut. | S |
| Scheduling | Trigger a capture at a time / on a cron | (none consumer) | Our own scheduler (host cron + in-app); ties to instant-upload for unattended captures. Differentiator. | M |
| Scrolling capture | Auto-scroll + stitch long pages | ShareX | Simulated scroll + Y-axis stitch w/ overlap detection. Niche; defer. | M |
| OCR (on-screen text) | Per-frame text → searchable | (ours via index) | **Already an index primitive** (`OnScreenText`/`TextKind`). Feed Tesseract/Apple Vision/Win OCR over salient frames. | M |
| Auto-captions | On-device Whisper, timestamped, lower-third | OpenScreen | Whisper (whisper.cpp Rust binding) on mono-16k; 12-min chunks; dedup/merge echoes. Output → index `TranscriptSegment` **and** caption overlay. | L |

### I. Agent-native index (the moat — **BUILT**)

| Feature | Spec | Best ref | Status |
|---|---|---|---|
| Structured clip index | One `Index` per clip: metadata, transcript, on-screen text, visual moments, **events**, summary, chapters, redactions | **none — clipxd-only** | **BUILT** `clipxd-index::{Index,Metadata,AppFocus,TranscriptSegment,VisualMoment,OnScreenText,Event,Summary,Chapter,Redaction}`. |
| Salience gate | Keep only meaningful frames (veyo Cells, not pixels) | none | **BUILT** `clipxd-import::gate` (veyo-core gate) + downscale/map/pipeline. |
| Browser-trace ingest | DOM/a11y/console/network/clicks → same index, no pixels | none | **BUILT** `clipxd-browser::{ingest,salience,trace}`. |
| Query surface | `query_clip` / `search_text` / `get_frame_context` / `get_events` | none | **BUILT** `clipxd-index::query` + `clipxd-mcp` MCP server (4 tools). |
| Recording→index bridge | A live recording's clicks/keys become index events; cursor drives auto-zoom | none | **PARTIAL** `clipxd-recorder::index_map` + `clipxd-cinematic` exist; live capture source pending. |

---

## 2. What we depend on directly — permissive only

**One legal line: scap is the only third-party capture/camera source that enters our binary; ffmpeg links dynamically under LGPL; everything cinematic, editorial, and agentic is clean-room.**

| Depend-directly (MIT/Apache/LGPL-dynamic) | Use |
|---|---|
| `scap-screencapturekit`, `scap-direct3d`, `scap-ffmpeg`, `scap-targets` (MIT) | Screen capture mac/win + target enumeration |
| `scap-cpal` + `cpal` (MIT/Apache) | Audio capture |
| `cap-camera`, `cap-camera-avfoundation/-mediafoundation/-directshow` (MIT) | Camera **enumeration + raw frames only** |
| `ffmpeg` (LGPL) | Encode/mux/transcode — **dynamic link only**, no GPL `--enable-gpl` build, no static link |
| `whisper.cpp` binding (MIT), `tesseract`/platform OCR | On-device captions + OCR |

**Hard-forbidden — study behavior, never port or link (closed hosted tier + AGPL §13 are fatal):**
- **AGPL-3.0:** all Cap crates except `scap-*`/`cap-camera*` (`cap-recording/editor/rendering/encoding/output_pipeline/audio/api`, `cap-cursor-*`, `cap-camera-effects`); **Recordly** (entire app). §13 network copyleft kills any hosted reuse.
- **GPL-3.0:** **Screenity** (all `src/`), **ShareX** (capture/shape/effects libs), **screenarc** (`src/`). Viral, Apache-incompatible.
- **PolyForm-NC 1.0.0:** **OpenVid** — forbids commercial use outright.

Everything from these is consumed as **behavioral spec** (parameter values, easing names, bitrate tables, phase ordering — uncopyrightable facts), never transcribed source. Reference repos live under `_reference/` (inspection-only, never vendored). Provenance note required per clean-room component; clean-room discipline checklist in `docs/reference-analysis.md` is binding.

> Note: `cap-cursor-info`/`cap-cursor-capture` carry **no MIT grant** (Cap LICENSE line 5) → treat as AGPL. The OpenScreen/Screenize MIT/Apache repos *could* be depended on, but they're Electron/Swift app code, not crates — we take their **techniques** (Whisper chunking, spring solver, dead-zone, temporal blur) clean-room rather than vendoring.

---

## 3. Architecture — Tauri-style app, event track → veyo → agent index

```
┌──────────────────────── Vite + React frontend ────────────────────────┐
│  Recorder controls · live preview · editor timeline · share page      │
│  (region overlay, hotkeys, cam bubble, drawing canvas, region model)  │
└───────────────▲──────────────────────────────────────▲───────────────┘
        Tauri IPC │ (commands / event stream)            │ MCP / HTTP
┌───────────────┴──────────────────────────────────────┴───────────────┐
│                          Rust core (crates)                            │
│                                                                        │
│  clipxd-recorder ──────────────┐         clipxd-cinematic [BUILT]      │
│   CaptureSource trait          │          compute_zoom_track()         │
│   ├─ scap backend (feat flag)  │          easing · zoom · render       │
│   ├─ InMemorySource (test)     │                ▲ cursor track          │
│   └─ Linux PipeWire (clean-rm) │                │                       │
│   interaction event track ─────┼────────────────┘                       │
│            │ clicks/keys/cursor                                          │
│            ▼  index_map                                                  │
│  ┌─────────────────────────────────────────────────────────────────┐  │
│  │ clipxd-import [BUILT]  ── veyo-core salience gate ── veyo-enrich  │  │
│  │   (video file path)        (Cells not pixels)      (OCR/captions) │  │
│  │ clipxd-browser [BUILT] ── DOM/a11y/net trace ──────► same index   │  │
│  └─────────────────────────────────┬───────────────────────────────┘  │
│                                     ▼                                    │
│            clipxd-index [BUILT]  ── THE Index (index.json) ──            │
│   Metadata·AppFocus·Transcript·VisualMoment·OnScreenText·Event·         │
│   Summary·Chapter·Redaction        + query.rs (search/frame/clip)       │
│                                     │                                    │
│            clipxd-mcp [BUILT] ── query_clip · search_text ──            │
│                                  get_frame_context · get_events         │
│            clipxd-web (TODO)  ── hosted share page + upload ──           │
└────────────────────────────────────────────────────────────────────────┘
```

**The flow that is the moat:** capture and the **interaction event track** are siblings, not master/slave. The same cursor/click/keystroke stream (a) drives `clipxd-cinematic` to *beautify* the video, and (b) flows through `clipxd-recorder::index_map` into `clipxd-index` `Event`s. veyo's salience gate keeps only meaningful frames; enrich adds OCR/transcript/visual-moments. The result — `index.json` — is queried by `clipxd-mcp`. **A recording is a render target for humans and a queryable document for agents, from one capture.** No competitor emits the second artifact.

`CaptureSource` is the seam: `InMemorySource`/video-file source exists today (testable everywhere); the live `scap` backend sits behind a feature flag for mac/win; Linux capture is clean-room PipeWire.

---

## 4. Phased build plan

**Demoable on THIS box** (Linux · Wayland · PipeWire 1.6.7) **today, no live capture needed:** the entire index/cinematic/editor/agent stack runs on file-source input.

| Phase | What | Why now | Box |
|---|---|---|---|
| **P3.0 — Recorder over file source** ✅ mostly | Wire `clipxd-recorder` `InMemorySource` + event track → `index_map` → `clipxd-index`; cursor track → `clipxd-cinematic`. Prove "video-in → beautified + queryable index-out" end to end. | Closes the loop with **zero capture deps**; fully testable on Linux. | ✅ Linux |
| **P3.1 — React recorder/editor UI** | Vite+React: region overlay, live preview, timeline with trim/zoom/speed/annotation **regions** (immutable model), waveform, undo. Driven by file source + computed zoom track. | Highest-leverage UX; renders the BUILT cinematic + index without a capture backend. | ✅ Linux |
| **P3.2 — Cinematic depth** | Spring/dead-zone follow, intent classifier, temporal motion blur, custom cursor render, keystroke pills, backgrounds/padding/2D mockups. | All pure render math; verifiable frame-by-frame on file input. | ✅ Linux |
| **P3.3 — Linux live capture** | Clean-room **PipeWire** screen capture (ashpd portal + `pipewire`/`wayland-client`, permissive) — Cap's Linux path is AGPL, so we write our own. | Unblocks live recording on this exact box (PW 1.6.7). Portal screencast is the only sane Wayland route. | ⚠️ needs PW glue (have PW 1.6.7) |
| **P3.4 — Native capture (mac/win)** | Enable `scap-*` feature flag: SCK (mac), D3D11 (win); `cap-camera*` webcam; `scap-cpal` audio; system audio. | The premium-quality path; **depends on hardware we don't have here.** | ❌ needs Mac/Win |
| **P3.5 — Enrich: OCR + captions** | Tesseract/platform OCR + whisper.cpp → populate `OnScreenText` + `TranscriptSegment`. | Makes `search_text` and `query_clip` rich; runs on file input. | ✅ Linux |
| **P4 — Hosted: share + instant upload** | `clipxd-web` (Rust) + React share page; chunked multipart instant-link; **agent index served beside the video over MCP/HTTP**. | The commercial moat made visible: a shared clip an agent can query. | ✅ Linux (backend) |
| **P4+ — Export polish** | fMP4 fragments + crash recovery, GIF/WebM-alpha, 3D mockups, HDR/ProRes (mac). | Quality + recovery; HDR is Mac-gated. | mixed |

---

## 5. Bottom line

**Build P3.1 — the React recorder/editor UI — next, fed by the existing file-source + the BUILT cinematic engine and index.** The whole differentiating stack (`clipxd-index`, `clipxd-mcp`, `clipxd-cinematic`, `clipxd-import`, `clipxd-browser`) is already done and runs end-to-end on this Linux box with zero capture dependencies. The single highest-leverage move is to make that stack *visible and editable* — a timeline where you scrub a video, watch the auto-zoom apply, and open the agent query panel beside it — because it proves the only claim that matters ("a recording you can query") without waiting on a capture backend. Live capture (clean-room PipeWire on Linux, `scap` on Mac/Win) is the limb to grow *after* the body is demoable, not before.
