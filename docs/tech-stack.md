# clipxd — tech stack

> Every technology choice and the reason for it. The through-line: **one Rust toolchain end to end**, so the recorder shares types with the codec directly (no FFI), matching the wider stack ([veyo](../../veyo), [CloakPipe](../../cloakpipe) are Rust too).

---

## 1. Language & toolchain — Rust, end to end

| Why | Detail |
|---|---|
| **Shares crates with veyo-core, no FFI** | capture feeds the codec in-process; one type system for the raw-session contract ([capture-backends §4](capture-backends.md#4-the-unified-raw-session-contract)) |
| **Low CPU, no Electron** | the realtime screen path must run the veyo gate *alongside* capture within budget ([phases.md Phase 3](phases.md#phase-3--owned-recorder--the-hard-part-realtime-screen-capture--cinematic)); this is also why Cap chose Rust+Tauri |
| **Single toolchain** | recorder, pipeline, store, MCP server, web service all one `cargo` workspace ([architecture §3](architecture.md#3-crate--workspace-layout)) |
| **Matches the ecosystem** | veyo and CloakPipe are Rust; vendoring them is native, not a bridge |

---

## 2. Component-by-component choices

| Concern | Choice | Why / rationale |
|---|---|---|
| **Screen capture** | `scap` (MIT) | the same MIT-licensed capture crate Cap uses — reusable without AGPL contamination ([licensing.md](licensing.md)); cross-platform pixel + cursor |
| **Browser capture** | CDP / Playwright-style driver + rrweb-style DOM stream | DOM/a11y/console/network is cheaper and exact vs captioning pixels; rrweb is the proven DOM-recording approach ([capture-backends §2](capture-backends.md#2-the-browser-backend)) |
| **Salience gate** | **veyo-core** (vendored) | the on-device codec that decides which moments to enrich — the whole cost/privacy thesis ([overview §2](overview.md#2-veyo-is-the-engine-clipxd-is-the-product)) |
| **Transcription** | **whisper.cpp** (local, CPU) | runs on-device, no audio leaves the box; well-trodden, fast on CPU |
| **OCR (screen mode)** | local OCR engine | on-device on-screen-text extraction; browser mode skips this (DOM is verbatim) |
| **Frame captioning** | small VLM, gated to salient frames only | only veyo-selected moments get captioned — the lever that avoids ~2.6B tokens/day |
| **Redaction** | **CloakPipe** (vendored) | Rust-native PII/secret masking, 33+ entity types, sub-5ms, local-first ([privacy-and-redaction.md](privacy-and-redaction.md)) |
| **Index store** | **SQLite + FTS5** + blob (video/frames on disk) | local-first, zero-ops, full-text over transcript+OCR+captions; the searchable-library substrate ([features §4.7](features.md), [architecture §6](architecture.md#6-storage-model)) |
| **Cinematic layer** | clean-room auto-zoom FSM; native render (or webview) | Screen-Studio-class beautify, owned; rendering surface is an [open question](risks-and-open-questions.md) |
| **MCP server** | `clipxd-mcp` (Rust) | exposes the index as MCP tools/resources for Claude/any agent ([mcp-api.md](mcp-api.md)) |
| **Web/share service** | `axum` (Rust) | small service for the share page + JSON API + sidecar; keeps the stack single-toolchain |
| **Import fetch** | yt-dlp-class fetcher + demux | URL → audio + sampled frames → raw session ([architecture §4.3](architecture.md#43-import-from-url-the-traction-path-ships-first)) |

---

## 3. The schema crate — the keystone

`clipxd-index` is plain Rust types that define `index.json` and the raw-session contract **once** ([index-schema.md](index-schema.md)). Everything depends on it; nothing else owns the contract. Serde for (de)serialization. This crate is built first ([plan §4](plan.md#4-phase-1-concretely--the-first-build)) and its schema-identity tests guard every later backend.

---

## 4. Vendored vs owned

| Owned (clipxd builds & maintains) | Vendored (consumed, versioned independently) |
|---|---|
| recorder (screen + browser capture glue) | **veyo-core / veyo-enrich** — the codec & enrichment |
| cinematic layer | **CloakPipe** — redaction |
| import | `scap` (MIT) — screen capture primitive |
| index schema crate | whisper.cpp — transcription |
| store / FTS | rrweb/CDP tooling — browser capture primitive |
| MCP server + web service | |

clipxd **owns the recorder, the pipeline wiring, the index contract, and the agent surface**; it **consumes** the codec, the redactor, and the low-level capture/transcription primitives. This split is what keeps the recorder *thin* ([overview §5](overview.md#5-what-makes-the-recorder-thin-fast-useful)) while standing on serious infrastructure.

---

## 5. Rendering — the one unsettled choice

The cinematic/preview layer can render **natively** (tighter, one toolchain) or in a **webview** (faster iteration on the canvas/compositing UI). This is tracked as an [open question](risks-and-open-questions.md#open-questions); it does **not** affect the index (the index is built from raw capture, never the beautified render — [features §4.2](features.md#42-beautiful-recording--the-cinematic-layer--adoption-table-stakes)), so it can be decided late without blocking the moat.

---

## 6. Non-choices (deliberately excluded)

- **No Electron.** CPU budget for the realtime path forbids it; Rust+native/Tauri-class is the point.
- **No heavyweight cloud DB.** Local-first means SQLite, not a managed Postgres, at the core. (A hosted tier may add object storage + CDN — but that's the commercial layer, not the product core: [licensing.md](licensing.md).)
- **No per-frame VLM streaming.** The entire architecture exists to *avoid* this (veyo gate). It's the anti-pattern the product is defined against.
