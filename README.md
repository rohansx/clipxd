<div align="center">

# clipxd

**Record once. Humans watch it. Agents read it.**

A fast, local-first screen recorder whose every recording is an **agent-queryable index** —
transcript, on-screen text, UI events, and salient moments, all answerable from the URL,
*without an agent ever watching the video*. Built in Rust on the [veyo](https://crates.io/crates/veyo-core)
visual-event codec, with a Vite + React editor.

Apache-2.0 core · open-core (closed hosted tier later) · clean-room

</div>

---

## The one thing

Cap, Loom, and Screen Studio all race on prettier pixels. clipxd matches that — auto-zoom,
backgrounds, instant share — and then does the part **nobody** does: turns every recording
into a structured index an agent queries from text.

> Paste a clip link into Claude. Ask *"what error showed up and what was the user doing right
> before it?"* Get a correct, timestamped, cited answer — without the video being watched.

That index is the moat. The recorder is table-stakes we own.

## Status

| Phase | What | State |
|---|---|---|
| **1 — Import** | any video/URL → agent-queryable `index.json` (no capture) | ✅ built |
| **2 — Browser** | DOM/console/network trace → the *same* index | ✅ built |
| **3 — Recorder** | cinematic auto-zoom · `record`/`beautify` · React editor · share layer | ✅ built (live screen capture is platform-gated) |
| 4 — Hosted | CloakPipe redaction + durable links | stubbed |

## Architecture

```
        Vite + React editor (app/)  ── plays a recording with live auto-zoom,
                  │                     asks the clip live (query_clip over HTTP)
          HTTP / MCP │
        ┌───────────┴──────────────── Rust core (crates/) ──────────────────────┐
        │  clipxd-recorder  capture → event track ┐                              │
        │  clipxd-cinematic clean-room auto-zoom   │ (cursor → zoom, math only)  │
        │  clipxd-import    video → frames ────────┤                              │
        │  clipxd-browser   DOM trace ─────────────┤                              │
        │                                          ▼                              │
        │            veyo-core salience gate ── veyo-enrich (OCR/caption)         │
        │                                          ▼                              │
        │            clipxd-index   THE index.json (the product)                  │
        │            clipxd-mcp     query_clip · search_text · get_frame_context  │
        │            clipxd-web     index.json sidecar + share page + query API   │
        └────────────────────────────────────────────────────────────────────────┘
```

The cursor/click/keystroke track is a **sibling** of the video, not a byproduct: it drives
the cinematic camera **and** becomes index `events` — so a recording is a render target for
humans and a queryable document for agents, from one capture.

## Quickstart

```bash
cargo build --release                      # Rust core (pulls veyo from crates.io)

# 1. turn a video into an agent-queryable clip
clipxd import some-screen-recording.mp4 --out clips
# …or a browser trace, or a real recording:
clipxd ingest-browser session.trace.json --out clips
clipxd record screen.mp4 --events events.json --out clips    # source: screen + event track

# 2. ask it — no video needed
clipxd query clips/clp_*  "what error showed up and what was the user doing right before it"

# 3. make it look produced (auto-zoom + background + padding)
clipxd beautify screen.mp4 --events events.json --out beautified.mp4

# 4. serve clips + open the editor
clipxd-web clips --port 8787 &
cd app && npm install && npm run dev        # → http://localhost:5174/?clip=<id>&api=http://localhost:8787
```

OCR (`clipxd import`) needs `eng.traineddata` on `TESSDATA_PREFIX`; transcripts need a
whisper.cpp binary (off by default). Live *screen* capture is feature-gated to scap
(macOS/Windows) or clean-room PipeWire (Linux) — the file/import path runs everywhere.

## Licensing & IP

Apache-2.0 with an intended **closed hosted tier**, so every reference recorder
(Cap = AGPL, Screenity/ShareX/screenarc = GPL, OpenVid = Non-Commercial, Recordly = AGPL)
is **study-then-clean-room** — implemented from observable behavior + uncopyrightable math,
never copied. Only permissive pieces are depended on directly (scap MIT, cpal, dynamic
ffmpeg). See [`docs/reference-analysis.md`](docs/reference-analysis.md) and
[`docs/recorder-feature-catalog.md`](docs/recorder-feature-catalog.md).

## Docs

[overview](docs/overview.md) · [architecture](docs/architecture.md) ·
[the index schema](docs/index-schema.md) · [MCP API](docs/mcp-api.md) ·
[phases](docs/phases.md) · [Phase-3 recorder plan](docs/phase3-recorder-plan.md) ·
[browser-backend spec](docs/phase2-browser-spec.md) · [competitive analysis](docs/competitive-analysis.md)
