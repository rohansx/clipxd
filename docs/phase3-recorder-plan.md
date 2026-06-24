# Phase 3 recorder — what we can legally reuse, and the plan

## 1. The hard licensing truth, up front

**You cannot copy, port, or link Cap into clipxd, and you cannot use OpenVid at all.** This is not a gray area:

- **Cap is AGPL-3.0** (its application + cinematic + rendering + cursor crates). AGPL §13 is the network-copyleft clause: if you offer AGPL-derived code as a hosted/networked service, you must publish the **complete corresponding source** of that service to every user. clipxd's whole differentiator is a **closed commercial hosted tier** (durable links, managed enrichment, compliance). Porting Cap's app code forces that tier open. Forbidden under Apache + closed-tier, full stop. Only Cap's **MIT** capture crates (`scap-*`, `cap-camera*`) are touchable.
- **OpenVid is PolyForm Noncommercial 1.0.0.** NC **prohibits commercial use entirely** and is **incompatible with every OSI license** (Apache, MIT, GPL, AGPL alike). There is **no clipxd license** under which OpenVid code becomes usable — not Apache, not even AGPL. It is off-limits in all scenarios.
- **All GPL recorders (OBS, ScreenArc, Screenity, vokoscreen, SimpleScreenRecorder, Peek, ShareX) are equally forbidden.** GPL-3.0 is incompatible with Apache-2.0; linking or porting any of it contaminates the build.

**Consequence:** the recorder's *engine* can use permissive MIT/Apache capture crates directly, but every piece of *cinematic and product logic* from Cap/OpenVid/GPL tools must be **clean-room** — implemented from observable behavior and uncopyrightable math, never by reading their source.

## 2. The strategic fork you must decide — clipxd's license

**Option A — Stay Apache-2.0 + closed hosted tier (RECOMMENDED).**
clipxd remains Apache-2.0; the hosted tier (durable links, managed enrichment, compliance) stays **closed and monetizable** — your actual business. The cost: everything in Cap/OpenVid/GPL is either clean-room or off-limits; only MIT/Apache pieces (`scap-*`, `cpal`, `cap-camera`, dynamically-linked ffmpeg) are depend-directly. You write the cinematic layer and recorder UX yourself. This is more upfront engineering, but it's a one-time cost against a durable, defensible moat (the agent-queryable index) that nobody else can ship.

**Option B — Make clipxd AGPL-3.0, then fork Cap.**
You get a massive head start: fork Cap's recording pipeline, editor, cursor capture, and muxing directly (AGPL→AGPL derivatives are allowed). But AGPL §13 then forces clipxd's **hosted tier source to be published to every user** — the closed enrichment/compliance/durable-link layer that *is* the product must go open-source or disappear. And OpenVid is **still** off-limits (NC ≠ AGPL). You'd trade your business model for a code head start, and still have to clean-room the cinematic layer anyway.

**Recommendation: Option A.** The open-core + closed-hosted model is the entire commercial thesis of clipxd. AGPL detonates it. Take the engineering hit, stay Apache-2.0.

## 3. What we depend on directly (permissive)

| Concern | Crate | License | Version | Notes |
|---|---|---|---|---|
| Screen/window capture | `scap` (or split `scap-screencapturekit`, `scap-direct3d`, `scap-targets`) | **MIT** | `0.0.8` stable / `0.1.0-beta.1` latest | Cross-platform; MIT confirmed |
| Audio capture | `cpal` | **MIT/Apache-2.0** | `0.15.3` | Pulled in by scap on Win; usable standalone on Linux |
| Encode/mux | `ffmpeg` (system, **dynamic link**) | LGPL-2.1+ | system | **Dynamic link only**, never static; build **without** libx264/libx265/libfdk-aac to avoid GPL contamination |
| Camera | `cap-camera` | **MIT** | latest | Cap's MIT camera crate is fine; `cap-camera-effects` is AGPL — forbidden |

**scap Linux/Wayland reality (critical):** scap *does* support Linux via **PipeWire + xdg-desktop-portal** (D-Bus `ScreenCastPortal`, X11 also via the PipeWire abstraction) — and the portal permission prompt on Wayland is **architectural, not a bug**. But it is **beta and currently rough on Linux**:
- **Compilation fails on PipeWire 1.6** (`spa_pod_builder` mismatch, #185) — pin to a PipeWire 0.8.x toolchain or carry a patch.
- **Wayland format negotiation crashes** ("no more input formats", #158) on some compositors (e.g. Niri).
- **`fps` parameter ignored** (#150), **`output_type` broken** (#151), **audio device hardcoded** to cpal default (#184).

**Plan for it:** target `scap` ≥ `0.0.8`, expect to carry local patches / upstream PRs for #150/#151/#158/#185, and drive Linux frame rate from our own pacing rather than trusting scap's `fps`. ffmpeg stays dynamically linked and GPL-codec-free.

## 4. What is clean-room regardless of the license fork

None of this can come from Cap/OpenVid/ScreenArc source — but all of it is either **uncopyrightable math** or **observable behavior**, so we implement it fresh:

- **Cinematic auto-zoom.** Easing (`easeOutQuart(t) = 1 - (1-t)^4`, `easeInOutCubic`), the 3-phase zoom-in/hold/zoom-out fragment FSM, **EMA mouse smoothing** (`ema = α·x + (1-α)·prev`), **lerp** (`a·(1-t) + b·t`), and anti-jitter dwell (hold camera during fast cursor moves) are **decades-old signal-processing and animation formulas — uncopyrightable**. Re-derive from textbooks/Penner equations (concepts, not code).
- **Backgrounds, gradients, device mockups, cursor interpolation.** Canvas-2D gradient/padding rendering and device-frame geometry are observable behavior; reimplement independently. For 3D perspective/mockups depend on MIT **Three.js**; for animation orchestration MIT **Framer Motion** or **react-spring**; state via MIT **Zustand + Immer**.
- **Instant-share pipeline.** Chunked upload, durable links, session JSON schema — designed fresh (the durable-link/compliance layer is your *closed* differentiator anyway).

**Audit hygiene:** for each cinematic component, record a one-line clean-room note (which observable behavior / which formula, plus an explicit "no AGPL/GPL/NC source read" statement).

## 5. Concrete Phase-3 build plan for clipxd

**Crates (new, all Apache-2.0 in the clipxd tree):**

- **`clipxd-recorder`** — capture orchestration. Wraps `scap` for screen/window frames + `cpal` for audio; captures **cursor position and click events** on our own timeline (not Cap's AGPL cursor crates). Owns frame pacing (compensating for scap's broken `fps`). Output: raw frame stream + an event track (cursor/click/keystroke timestamps).
- **`clipxd-cinematic`** — clean-room zoom/pan engine. Consumes the event track, produces zoom fragments via the 3-phase FSM + EMA/lerp/dwell math. Renders backgrounds/gradients/mockups. No Cap/OpenVid code.
- **Encode** via dynamically-linked ffmpeg (LGPL, GPL-codec-free) → MP4/WebM.
- **Share** through the existing **clipxd-web** layer: chunked upload → durable link. This is where the **closed hosted tier** lives.

**The moat — feed the veyo gate so the recording is ALSO agent-legible:**
This is the part no competitor has. The recorder doesn't just emit a video — it emits, in lockstep, the **event track** (cursor/click/keystroke + zoom fragments + window/target metadata). That structured trace flows into clipxd's existing pipeline:

```
clipxd-recorder ──▶ raw frames + event track
                          │
                          ▼
                 veyo gate (Cells, not pixels)
                          │
                          ▼
                 enrich (managed, closed tier)
                          │
                          ▼
                 agent-queryable index  ◀── the differentiator
```

So a recorded session is queryable ("show the moment the user clicked Deploy", "what window was focused at 0:42") — the recording is a **first-class agent-legible artifact**, not an opaque MP4. That join (gesture → request → index) is the defensible product.

**Demoable on THIS Linux/Wayland box vs needs a Mac/Win:**

| Capability | This box (Linux/Wayland) | Needs Mac/Win |
|---|---|---|
| Screen capture via scap | **Yes**, via PipeWire+portal — but expect the beta caveats (#158/#185), carry patches | macOS ScreenCaptureKit, Win D3D11 paths untested here |
| Audio via cpal | **Yes** | — |
| Cursor/click event track | **Yes** (our own, X11/Wayland clean-room) | — |
| Cinematic zoom + backgrounds/mockups | **Yes** (pure compute + Canvas/Three.js, platform-agnostic) | — |
| ffmpeg encode → MP4 | **Yes** (system ffmpeg, dynamic) | — |
| veyo gate → enrich → index | **Yes** (existing pipeline) | — |
| macOS-native capture (`scap-screencapturekit`) | No | **Mac** |
| Windows-native capture (`scap-direct3d`) | No | **Win** |

**Bottom line for the demo:** the full vertical slice — capture → cinematic → encode → veyo → index → query — is demoable on **this Wayland box today** (modulo carrying scap Linux patches). Only the Mac/Windows native capture backends need their own hardware.

## 6. Bottom line — the single recommended path

**Keep clipxd Apache-2.0 with the closed hosted tier.** Depend directly on the **MIT** `scap-*` / `cap-camera` crate families for screen/audio/camera capture and dynamically-linked LGPL ffmpeg for encode. **Clean-room everything else** — the cinematic auto-zoom (uncopyrightable easing/EMA/lerp/dwell math), backgrounds/mockups, cursor track, and instant-share pipeline — from observable behavior, never from Cap/OpenVid/GPL source. Build `clipxd-recorder` + `clipxd-cinematic`, share via `clipxd-web`, and wire the event track through the **veyo gate → enrich → index** so the recording is agent-queryable. That join is the moat; AGPL would destroy the business model and OpenVid is unusable under any license, so neither is on the table.
