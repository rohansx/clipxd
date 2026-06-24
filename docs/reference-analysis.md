# clipxd — reference repo analysis (take / don't-take / implement)

## Governing rule

clipxd ships an **Apache-2.0 core** plus a **CLOSED commercial hosted tier**. That combination dictates exactly one licensing rule for everything below:

- **Permissive only for linking.** We may depend on, link, ship, and (if needed) relicense source that is **MIT or Apache-2.0**. Nothing else may enter our binary or our repo.
- **Copyleft + non-commercial source is off-limits to port or link.** Any code under **AGPL-3.0, GPL-3.0, or PolyForm Noncommercial 1.0.0** cannot be copied, ported, partially transcribed, or linked. AGPL Section 13 (network-service copyleft) is fatal to a closed hosted tier; GPL-3.0 is viral copyleft; PolyForm-NC forbids commercial use outright.
- **Everything else is observe-behavior-then-clean-room.** For forbidden-license repos we may study *observable behavior* (UX, output formats, parameter values, algorithm shape) and reimplement independently. We do **not** read-then-transcribe their source.

Net: the only thing we actually *depend on* is the MIT `scap-*` / `cap-camera*` family from Cap. Every cinematic/editor capability is clean-room.

---

## Cap — https://github.com/CapSoftware/Cap

### License boundary

Cap is a Rust + Tauri monorepo that is **mixed-license**:

- **MIT (safe to depend on, ship closed, relicense-compatible):** every crate matching `scap-*` (`scap-screencapturekit`, `scap-direct3d`, `scap-ffmpeg`, `scap-targets`, `scap-cpal`) and every crate matching `cap-camera*` (`cap-camera`, `cap-camera-avfoundation`, `cap-camera-directshow`, `cap-camera-mediafoundation`, `cap-camera-windows`, `cap-camera-effects`). The MIT boundary is declared in the root `LICENSE` file.
- **AGPL-3.0 (forbidden — port, link, or pattern-copy all off-limits):** every other crate (`recording`, `api`, `audio`, `automation`, `editor`, `rendering`, `encoding`, `project`, `output_pipeline`, the cursor crates) and both apps (`apps/desktop/src-tauri`, `apps/web`). Section 13 network-service copyleft applies to any networked deployment.

Note: `cap-cursor-info` and `cap-cursor-capture` are **not** in the MIT families (LICENSE line 5) and carry no separate MIT grant — treat as AGPL. The `project` crate (metadata schemas) is AGPL by default; only the *observable JSON schema* may be reimplemented, not the crate.

### ✅ TAKE

| Item | Why | How to consume |
|---|---|---|
| `scap-screencapturekit` | MIT macOS screen capture wrapper over ScreenCaptureKit (macOS 13+); simplified API without losing low-level access | `scap-screencapturekit = "0.1.0"` (verify crates.io publication) |
| `scap-direct3d` | MIT Windows screen capture via Direct3D11 / Windows.Graphics.Capture | `scap-direct3d = "0.1.0"` (Windows backend) |
| `scap-ffmpeg` | MIT platform-agnostic capture abstraction; orchestrates SCK on macOS, D3D on Windows; `AsFFmpeg` frame conversion | `scap-ffmpeg = "0.1.0"` (primary capture abstraction) |
| `scap-targets` | MIT display/window enumeration: `Display`, `Window`, `DisplayId`, list/find/query across platforms | `scap-targets = "0.1.0"` (target selection) |
| `scap-cpal` | MIT audio capture via CPAL; simple `Capturer` wrapping `cpal::Host` | `scap-cpal = "0.1.0"` (audio backend) |
| `cap-camera` | MIT cross-platform camera/webcam abstraction (AVFoundation / MediaFoundation / DirectShow / v4l2) | `cap-camera = "0.1.0"`, features `["serde", "specta"]` |
| `cap-camera-avfoundation` | MIT macOS camera backend (AVFoundation) | `cap-camera-avfoundation = "0.1.0"` |
| `cap-camera-mediafoundation` | MIT Windows camera backend (MediaFoundation) | `cap-camera-mediafoundation = "0.1.0"` |
| `cap-camera-directshow` | MIT Windows camera fallback (DirectShow) | `cap-camera-directshow = "0.1.0"` |

### 🚫 DON'T TAKE

| Item | Why |
|---|---|
| `cap-recording` (+ `api`, `audio`, `automation`, `editor`, `rendering`, `encoding`, `output_pipeline`) | AGPL-3.0. Application + networked-service logic; Section 13 source-release applies. Porting implementation forbidden for a closed product. |
| `apps/desktop/src-tauri` (Tauri desktop app) | AGPL-3.0. GUI/orchestration; cannot reuse or pattern-port. |
| `apps/web` (Next.js dashboard) | AGPL-3.0. Network-service code; closed hosted reuse violates AGPL §13. |
| `crates/project` (metadata crate) | AGPL-3.0 by default. Only the *observable JSON schema* may be reimplemented clean-room, not the Serde types/code. |
| `cap-cursor-info`, `cap-cursor-capture`, `cap-camera-effects` | Not in MIT families; cursor crates default to AGPL; camera *effects* are AGPL. Enumeration in `cap-camera` is MIT; post-processing is not. |
| Instant-mode + Studio-mode pipelines (`instant_recording.rs`, `studio_recording.rs`, `recorder-core/*.ts`) | AGPL-3.0. Multipart upload, segmentation, multi-track sync, muxer/task-pool scaffolding are copyrighted implementations. |

### 🔨 IMPLEMENT CLEAN-ROOM

| Component | Behavior | Algorithm approach | Notes |
|---|---|---|---|
| Screen-capture orchestration | `capture_start(target) -> Frame stream`; raw pixel buffers + timestamps; pause/resume; target via Display/Window query | macOS: SCK `ContentFilter` + `StreamCfg`, builder + `output_sample_buf` callback emitting `Frame`. Windows: `Direct3D11CaptureSession` + `GraphicsCaptureItem`, frame-arrived event → staging texture → read pixels. Linux: PipeWire/X11 (not in MIT crates). | Use the MIT `scap-*` `Capturer::builder()` APIs. Do **not** reuse `cap-recording`'s `OrchestrationSession`/`ActorState` (AGPL). |
| Target enumeration | `Display::list/primary/get_containing_cursor`; `id/name/physical_size/logical_size/refresh_rate`; `Window::list` | macOS `SCShareableContent`; Windows `EnumDisplayMonitors`/`EnumWindows`; X11 `RRGetScreenInfo`. Wrap platform impls behind a `Display` newtype. | `scap-targets` already gives this MIT — prefer depending over reimplementing. Keep platform impls internal. |
| Audio capture (CPAL) | `create_capturer(data_cb, error_cb) -> Capturer`; `.play()/.pause()`; `StreamConfig` metadata | `scap-cpal` wraps `cpal::Host` default device; `safe_buffer_size()` clamps to ~80 ms target. | Depend on MIT `scap-cpal`. Do **not** copy `cap-recording` mixing/resampling (AGPL). |
| Camera enumeration/capture | Platform-independent `Camera` enum; list devices, select by ID, stream raw frames | `cap-camera` re-exports platform backends behind a common trait. | MIT enumeration only — no effects/post-processing (`cap-camera-effects` is AGPL). |
| Cursor event capture | 16 ms poll loop for position/buttons/keyboard; normalize coords; batch and flush JSON every 5 s; capture cursor image, SHA256-dedupe to `cursors/*.png` | `device_query` per tick; elapsed ms from start; raw→[0,1] normalized→crop bounds; button down/up by prev-state diff; keycode→display string. Cursor image: macOS `NSCursor.TIFFRepresentation`, Windows `GetCursorInfo`+`GetIconInfo`+`DrawIconEx`, Linux x11rb `xfixes_get_cursor_image`. Hotspot fractional 0–1. | Behavioral spec only; `cap`'s `cursor.rs` is AGPL. Incremental flush bounds JSON size; final flush on stop. |
| Recording session data model | `RecordingMeta` (instant vs studio); studio = `segments[]` with display/camera/mic/system-audio/cursor/keyboard tracks (relative paths, fps, start offset, device id); `cursors` map; timeline; status InProgress→NeedsRemux→Complete | On start write `recording-meta.json` status=InProgress. Per closed segment append `{path, first_timestamp, video_frame_count, audio_gap_summary}`. On stop compute timeline, audio offsets (`CROSS_TRACK_SNAP_SECS ≈ 0.02`, snap leader camera>mic>display), write final JSON. Paths relative to recording dir. | Reimplement the **schema** (observable output), not Cap's Serde `cap_project` types (AGPL). |
| Instant-mode upload pipeline | On stop, chunk into ~5–16 MB parts; ≤3 concurrent presigned PUTs; multipart with exponential-backoff retry (≤8/part); `/complete` returns link in seconds | Buffer frames to `MIN_PART_SIZE` (5 MB) → enqueue; pool slot → presign → PUT with ETag tracking; final flush; completion includes duration/width/height/fps. | Cap uses XHR, 30 s stall timeout, 128 MB overflow guard, 500 ms–30 s backoff; S3 + Drive (Content-Range, serial). Behavior only — code is AGPL. |
| Studio-mode local recording | Record screen/camera/audio to segments (`init.mp4` + `fragment.m4s`, audio m4a/ogg); pause finalizes segment, resume starts new one; stop → remux + effects + cursor render | Independent per-track output pipeline; segment-boundary stop/freeze/resume with cursor continuity; compatibility mode caps screen to 1600×1000 with camera; quality tiers Compatibility(24fps)/Balanced/Ultra. | Build simpler output interfaces over FFmpeg/system APIs directly — do **not** copy Cap's muxer/`output_pipeline` framework (AGPL). |

**Linux gap:** screen capture is *not* in the MIT `scap-*` crates (Cap's Linux path lives in AGPL `cap-recording` via `ashpd`/`pipewire`/`x11rb`). For Linux desktop capture we must write clean-room X11/Wayland using permissive crates (`x11rb` MIT/Apache, `wayland-client`). Audio (`scap-cpal`) and camera (`cap-camera` v4l2) are covered on Linux.

---

## openvid — https://github.com/CristianOlivera1/openvid

### License boundary

**PolyForm Noncommercial 1.0.0** — strictly forbids commercial use and sublicensing (viral on commercial reuse). clipxd is a commercial Apache-2.0 product with a paid hosted tier, so **no openvid code may be used, linked, or ported**. Personal/research/educational/charitable use only — none of which we qualify for. Everything is observe-behavior-then-clean-room.

### ✅ TAKE

— (nothing; license forbids reuse)

### 🚫 DON'T TAKE

| Item | Why |
|---|---|
| All openvid code (TypeScript canvas utils, mockups, zoom logic, export pipelines, Three.js helpers) | PolyForm-NC 1.0.0 prohibits use in commercial products. clipxd's closed hosted tier violates the license. |

### 🔨 IMPLEMENT CLEAN-ROOM

| Component | Behavior | Algorithm approach | Notes |
|---|---|---|---|
| Auto/cursor zoom (cinematic camera) | 3-phase fragment: entry (ease-in scale), hold (dwell, focus point may move), exit (ease-out). Speed 1–10 → duration; zoom 1–10 → target scale; independent 3D rotation across phases | `easeOutQuart(t)=1-(1-t)^4` entry/exit; `easeInOutQuart` for focus movement; zoom→[1.2,4.0] linear; speed→[150ms,2000ms]; fragment dur = total − 2·transition; focus dwell within hold window; 3D intensity 0–100 modulates `32deg·intensity`; perspective 500px; cameraZ `=(2·perspectivePx)/1080`. | Default fragments at 33%/67% marks, 2 s each. Simple zoom exits after fragment; advanced (3D/movement) stays inside. Cursor focus smoothing via `easeInOutCubic` blend. |
| Backgrounds / padding / corners / shadows / device mockups | 100+ gradient + solid presets; Safari/Chrome/Arc/Brave/VS Code/macOS/iPhone/Samsung mockups via 2D Canvas; padding/radius/shadow scale to canvas | Parse gradient CSS (angle for linear; originX/Y for conic; color stops); `createLinear/Radial/ConicGradient`. `scaledPaddingX/Y=(pad·0.5/100)·canvasDim`; `scaledRadius=corners·(w/896)`; `scaledShadowBlur=shadow·(w/896)·0.3`. Mockups: `save→scale→primitives(arc/quadraticCurve)→restore`, scale `=(w/1280)·1.2`; return `{contentX,Y,W,H}`. | Baselines: canvas 896px, mockups 1280×720. `BOTTOM_ONLY_RADIUS_MOCKUPS` (header mockups), `SELF_SHADOWING_MOCKUPS` (glass). All standard Canvas primitives — generalizable, not proprietary. |
| Export pipeline (4K / WebM / GIF) | Frame-by-frame canvas render at target res with zoom/effects baked in; 4K/2K/1080/720/480, WebM-alpha (VP8), GIF; FFmpeg audio mixing; AR preserved | Pause→`currentTime=0`→even-pixel dims `round(d/2)·2`. MP4: `CanvasSource→Output` (mediabunny). WebM: PNG frames → `ffmpeg -f image2 -framerate fps -i frame%05d.png -c:v libvpx -vf format=yuva420p`. GIF: `palettegen`→`paletteuse dither=bayer`. Audio: `amix`/`adelay`/`asetpts`/`atrim`. | Quality table: 4K 40Mbps, 2K 16Mbps, 1080p 8Mbps, 720p 5Mbps, WebM/GIF 2–2.5Mbps. Letterbox when AR differs. Even-pixel clamp is an H.264 requirement. |
| 3D perspective (zoom rotation) | `enable3D` with rotateX/Y (0–45 → intensity); Three.js WebGL applies perspective to a flat plane; rotation eased across phases with opacity ramp | Plane geometry `(2·aspect,2)` + `CanvasTexture`; `fovDeg=2·atan(1/cameraZ)·180/π`, `cameraZ=(2·perspectivePx)/1080`; `plane.rotation.x=-(rotXDeg·π/180)`, `.y=(rotYDeg·π/180)`; `maxRotation=32deg·intensity/100`; opacity entry `min(1,p·1.2)`, exit `max(0,1−p·1.8)`. | SRGBColorSpace, NoToneMapping, alpha on; LinearFilter no mipmaps; transparent material, depth off; cache renderer/scene/camera/plane/texture, dispose on unmount. |
| Cursor position interpolation | Keyframes (time, x%, y%, state, clicking); playback interpolates with `easeInOutCubic` blended with linear; smoothing 0–100 sets blend ratio; state/clicking from nearest keyframe | Find prev/next around `currentTime`; `progress=(t−prev.t)/(next.t−prev.t)`; `eased=easeInOutCubic(p)·(s/100)+p·(1−s/100)`; `lerp` x/y; state from `progress<0.5?prev:next`. | Defaults: smoothing 50, size 32px, click effect 'ripple'. Capture ~60 Hz; interpolate to arbitrary playback time. |
| 3D device mockups (iPhone/Samsung/laptop) | Three.js loads GLTF, renders video/image onto UV-mapped screen; FOV 20°, camera [0,0,6]; rotation via interaction/animation | `createCoverScreenCanvas`: fit source via `Math.max` scale, center offset, clip to corner radius, `imageSmoothingQuality='high'`; optional mask gradients with `destination-out`; `CanvasTexture` → screen mesh, `needsUpdate=true` per frame; rotation `x=-rotXDeg·π/180`, `y=rotYDeg·π/180`. | `RENDER_MULTIPLIER=4` supersampling; per-device dimension/offset/cornerRadius configs; mask angle CSS convention (0=up). Models would need our own permissively-licensed assets. |

---

## screenarc — https://github.com/tamnguyenvan/screenarc

### License boundary

**GPL-3.0** — viral copyleft across all `src/` TypeScript/React. Cannot reuse, link, or derive in an Apache-2.0 product. Observe behavior and reimplement clean-room only.

### ✅ TAKE

— (nothing; license forbids reuse)

### 🚫 DON'T TAKE

| Item | Why |
|---|---|
| All `src/` TypeScript/React | GPL-3.0 viral copyleft; incompatible with Apache-2.0. |
| `transform.ts` zoom calculation | Direct copyleft implementation. |
| `easing.ts` spring-physics functions | GPL-3.0 code. |
| `renderer.ts` canvas pipeline | GPL-3.0 implementation. |

### 🔨 IMPLEMENT CLEAN-ROOM

| Component | Behavior | Algorithm approach | Notes |
|---|---|---|---|
| Click-driven zoom FSM | Active region → smooth zoom-in, pan to follow smoothed mouse, then zoom-out. Phases: zoom-in (scale 1→level, pan 0→initialPan), hold (scale=level, pan=livePan), zoom-out (scale→1, pan→0) | Per frame: find region where `t∈[start,start+duration]`; `zoomInEnd=start+transition`, `zoomOutStart=start+duration−transition`. Phase 1 `t=(now−start)/transition`, ease, lerp scale + initialPan. Phase 2 livePan from metadata + bound calc. Phase 3 ease lerp scale level→1, finalPan→0. | Three separate pan targets: initialPan/finalPan are **stationary** (fixed-time), livePan updates every frame — prevents hold-phase jitter. |
| Mouse smoothing & pan calc | EMA-smooth raw click coords; map to camera pan via normalized coords + zoom-aware bounds | `getSmoothedMousePosition`: events in `[t−0.5s, t]`, EMA `smoothingFactor=0.1`, `smoothedX=lerp(prev,next.x,0.1)`, interpolate sub-frame. `calculateBoundedPan`: `nsmx,nsmy=(x/recW, y/recH)`; `targetPan=(0.5−((nsmy−originY)·zoom+originY))·frameHeight`; clamp to bounds via origin. | 0.5 s lookback builds EMA context; bounds keep zoomed view inside frame edges. |
| Transform origin / zoom anchor | Anchor from first click, normalized to [-0.5,0.5], converted to CSS transform-origin % | `targetX=firstClick.x/w−0.5`, `targetY=…`; `originX=targetX+0.5` (→[0,1]); `transform-origin = originX·100% originY·100%`; canvas decompose `originPxX=originXMul·frameContentWidth`; `translate(frameXY)→translate(originPx)→scale(s)→translate(-originPx)`. | Origin stays fixed across all 3 phases → stable anchor. |
| Canvas 2D rendering with zoom | Background first; video under zoom transform; cursor, click ripples, webcam overlay composited on top | `drawBackground`; compute `frameContent` size from AR + padding; `save→translate(frameXY)→translate(originPx)→scale→translate(transXY)→translate(-originPx)`; draw video; click ripples (events in `[now−dur,now]`, eased expanding circle, fading); cursor (nearest event, bitmap, drop-shadow, scale anim); restore; webcam via smart-position FSM. | Webcam has its own FSM transitioning across 8 corners with 0.5 s easing to dodge the cursor. |
| Background / gradient compositing | Solid, 8-direction linear, radial, and image/wallpaper; images center-cropped to canvas AR | Color: `fillStyle`+`fillRect`. Gradient: coords from direction (`'to right'→[0,0,w,0]`), add stops. Image: compute crop `(sx,sy,sW,sH)`; if wider than canvas `sH=img.h, sW=sH·canvasAR, sx=(img.w−sW)/2`; `drawImage(...)`. | Async image load; fall back to dark blue until ready. |
| Webcam smart positioning | Moves to avoid cursor using 0.1 s lookahead; 8-position grid; transitions eased over 0.5 s | `currentPos=getTargetPosAtTime(now)`; check `futureCursorIndex` at `now+0.1s`; if overlap return adjacent from `ADJACENT_POSITIONS`. History: scan back in 0.05 s steps for last change → `timeOfChange`; if changed and `<0.5s`, `progress=(now−change)/0.5`, ease, lerp rects. Size `=base·(size%/100)`; if `scaleOnZoom`, lerp to `SCALE_ON_ZOOM_AMOUNT=0.8` during zoom. | Smart position keeps click ripples + cursor scale visible. |

**Auto-zoom generation (from `projectSlice.ts`, behavior only):** group clicks within ~3 s windows; create regions with `AUTO_ZOOM_PRE_CLICK_OFFSET=1.0s` and `AUTO_ZOOM_POST_CLICK_PADDING=0.9s`; anchor from first click. Speed presets `{Slow:1.5, Mellow:1.0, Quick:0.7, Rapid:0.4}`.

---

## Decision summary

**The one thing we actually depend on:** Cap's **MIT `scap-*` family** (`scap-screencapturekit`, `scap-direct3d`, `scap-ffmpeg`, `scap-targets`, `scap-cpal`) and the **MIT `cap-camera*` family**. These are the *only* third-party source that enters our build. Everything else is clean-room.

**Everything cinematic is clean-room from behavior:** zoom FSM, pan smoothing, transform-origin anchoring, backgrounds/gradients, padding/corners/shadows, 2D + 3D device mockups, Three.js perspective, cursor interpolation, click ripples, smart webcam positioning, and both the instant-upload and studio-recording pipelines. The sources are AGPL (Cap pipelines), GPL-3.0 (screenarc), or PolyForm-NC (openvid) — none may be ported or linked.

| clipxd phase | What it pulls from this analysis |
|---|---|
| **Phase 1 — Import** | **Nothing here.** Import ingests existing media into the agent-queryable index; no capture backend or cinematic engine is required. This entire document is irrelevant to Phase 1. |
| **Phase 3 — Screen backend** | Depend on MIT `scap-*` (macOS SCK, Windows D3D11, targets, CPAL audio) + `cap-camera*` (camera). Write Linux X11/Wayland capture clean-room with permissive `x11rb`/`wayland-client`. |
| **Phase 3 — Cinematic** | Clean-room the zoom FSM, pan/EMA smoothing, transform-origin anchoring, backgrounds, mockups (2D + 3D), perspective, export pipeline, cursor interpolation — derived from openvid/screenarc *behavior*. |
| **Phase 3 — Recorder UX** | Clean-room cursor event capture, recording session data model/JSON schema, instant-upload pipeline, studio multi-track segment recording — derived from Cap's *observable behavior*, never its AGPL source. |

Single dependency line that matters: **scap is the only port-in; all cinematic and recorder UX is clean-room.**

---

## Clean-room discipline checklist

To keep the closed hosted tier legally clean while reimplementing AGPL/GPL/PolyForm-NC behavior, every clean-room component must satisfy:

- [ ] **Observe behavior, not source.** Derive the spec from running the tool, reading its docs/UI, and inspecting *output formats* (JSON schemas, file layouts, frame formats). Do **not** open the forbidden source and transcribe it line-by-line.
- [ ] **Separate the eyes from the hands where feasible.** Behavior is captured as a written, license-free spec (parameter values, easing names, phase ordering, default constants); a second pass implements *from the spec*, not from the original repo.
- [ ] **Algorithms and constants are facts, not code.** Easing formulas (`easeOutQuart`, `easeInOutCubic`), bitrate tables, snap thresholds (`CROSS_TRACK_SNAP_SECS≈0.02`), offsets (`1.0s`/`0.9s`), and zoom ranges (`[1.2,4.0]`) are uncopyrightable values — record them. Their *expression* in the original source is copyrighted — do not copy it.
- [ ] **No structural copying.** Don't mirror their file layout, function decomposition, type names, or comment structure. Choose our own module boundaries (per house style: many small files, 200–400 lines).
- [ ] **Verify license at link time.** Before adding any crate, confirm MIT/Apache (check the crate's own `LICENSE`, not just the workspace root). Re-verify `scap-*`/`cap-camera*` publication and license on crates.io. Treat undeclared-license crates (e.g. cursor crates) as forbidden.
- [ ] **Document independent derivation.** For each clean-room component, keep a short provenance note in-repo: which observable behavior it matches, what spec it was built from, and an explicit statement that no forbidden source was copied. This is the audit trail if provenance is ever questioned.
- [ ] **Quarantine forbidden source.** Reference repos under `_reference/` (AGPL/GPL/NC) are inspection-only and must never be imported, vendored, or copied into the build tree.
