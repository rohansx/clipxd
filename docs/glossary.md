# clipxd — glossary

> Terms used across these docs, in one place.

---

**agent-legible / agent-readable** — a recording whose meaning an LLM/agent can extract by querying text (transcript, on-screen text, events, captions) instead of watching pixels. The thing clipxd delivers that Loom/Cap/Screen Studio don't. → [overview §1](overview.md#1-the-one-thing-this-product-is)

**caption (salient frame caption)** — a short text description of what a *salient* moment shows, written into the `visual_timeline`. Only veyo-selected moments get captioned — that's the cost lever. → [index-schema §4](index-schema.md#4-visual_timeline--the-veyo-gated-heart)

**cinematic layer** — the beautify pipeline: auto-zoom, easing, dwell, anti-jitter, backgrounds, padding, corners, shadows, device mockups. Screen-Studio-class, owned, clean-room. Renders the *human* video only; never feeds the index. → [features §4.2](features.md#42-beautiful-recording--the-cinematic-layer--adoption-table-stakes)

**clip** — a recorded (or imported) session run through the veyo pipeline, with the video file attached. Concretely: `{ video, frames/, index.json }`. → [overview §2](overview.md#2-veyo-is-the-engine-clipxd-is-the-product)

**clipxd** — the product: a thin, fast, owned screen recorder whose output is an agent-queryable index. *clip + index* — the index is the product. → [overview](overview.md)

**CloakPipe** — the sibling product; a Rust-native PII/secret redaction proxy. Runs as clipxd's redaction pass before any index is shareable. → [privacy-and-redaction.md](privacy-and-redaction.md) · [../../cloakpipe](../../cloakpipe)

**degrade mode** — running the index pipeline when veyo-core isn't yet at its recall gate: denser/fixed-interval captioning, higher token cost, *identical schema*. → [plan §5](plan.md#5-codec-gated-branch-the-one-real-fork-in-the-plan)

**delta** — veyo's compact, structured change emitted only when the scene meaningfully changes (vs streaming every frame). The codec's output unit. → [overview §2](overview.md#2-veyo-is-the-engine-clipxd-is-the-product)

**emission (rate)** — the fraction of frames veyo actually emits a delta for. The codec's gate targets emission < 1% at recall ≥ 0.9 — i.e. catch what matters, stay silent otherwise. → [roadmap](roadmap.md#the-one-dependency-that-can-reorder-everything)

**enrich / veyo-enrich** — the stage that turns salient moments into index content: whisper.cpp transcript, frame captions, OCR, structured event track. → [architecture §1](architecture.md#1-the-pipeline-end-to-end)

**event track** — the interaction stream in the index: clicks/keys/scroll always; in browser mode also DOM mutations, a11y, console, network. Answers "what was the user *doing*." → [index-schema §6](index-schema.md#6-event_track)

**headline demo** — paste a clip link into Claude, ask "what error showed up and what was the user doing right before it," get a correct timestamped answer with the video never downloaded. The product's proof. → [overview §6](overview.md#6-the-headline-demo)

**import-from-URL** — paste an existing Loom/Cap/arbitrary video URL → run through enrich → get the same index. Zero capture code; ships first. → [features §4.4](features.md#44-import-from-url--process-recordings-that-already-exist--adoption-ships-first)

**index / index.json** — the structured, agent-queryable object a clip resolves to: transcript, visual timeline, on-screen text, event track, summary, metadata, redaction manifest. **The product.** → [index-schema.md](index-schema.md)

**local-first** — capture, gating, transcription, OCR, and redaction all run on-device; in default mode nothing leaves the box. → [privacy-and-redaction §1](privacy-and-redaction.md#1-the-two-guarantees)

**MCP (Model Context Protocol)** — the open standard for exposing tools/resources to an agent. clipxd's `clipxd-mcp` exposes a clip's index as MCP tools (`query_clip`, `get_frame_context`, `search_text`, `get_events`). → [mcp-api.md](mcp-api.md)

**moat vs table-stakes** — *table-stakes* = the recorder experience Loom/Cap already do well (clipxd must match). *moat* = the index, redaction, and library that need an owned codec + redactor (no one can quickly copy). → [features: differentiation map](features.md#feature--phase--differentiation-map)

**on-screen text** — searchable, timestamped text that appeared on screen. OCR in screen mode; DOM/a11y-verbatim (exact) in browser mode. → [index-schema §5](index-schema.md#5-on_screen_text)

**raw session** — the backend-agnostic intermediate (`source`, media, events) that all three sources emit and veyo-core consumes. Keeps the frontends decoupled from the pipeline. → [capture-backends §4](capture-backends.md#4-the-unified-raw-session-contract)

**redaction manifest** — the receipt CloakPipe writes into the index recording *what* was masked, *where*, *what kind* — making redaction auditable, not silent. → [index-schema §8](index-schema.md#8-redaction--the-manifest)

**salience / salience gate** — veyo-core's mechanism for deciding which moments are worth enriching (scoring + habituation so repetitive/idle stretches stay silent). What makes agent-legible video affordable. → [overview §2](overview.md#2-veyo-is-the-engine-clipxd-is-the-product)

**`scap`** — the MIT-licensed screen-capture crate (same family Cap uses). clipxd's planned screen-capture primitive — reusable without AGPL contamination. → [licensing §2](licensing.md#2-the-agpl-constraint--the-rule-that-shapes-the-build)

**schema-identity** — the invariant that screen, browser, and import all produce the *same index shape*; only which streams are populated differs. Enforced by tests. → [index-schema §9](index-schema.md#9-invariants-what-consumers-can-rely-on)

**sidecar (`.json`)** — every clip URL has a sibling `…/index.json`; append it (or content-negotiate) to get the structured object behind the same URL. → [mcp-api §2](mcp-api.md#2-json-api--sidecar-non-mcp-consumers)

**thin recorder** — the discipline that the recorder owns *only* capture + cinematic + share handoff; no editor, analytics, or team admin. → [overview §5](overview.md#5-what-makes-the-recorder-thin-fast-useful)

**veyo** — the engine: a local-first visual-event codec that keeps a text world-state on-device and emits deltas only on meaningful change. clipxd is its first user-facing surface. → [overview §2](overview.md#2-veyo-is-the-engine-clipxd-is-the-product) · [../../veyo](../../veyo)

**visual timeline** — the veyo-gated stream of timestamped captions for salient moments — the heart of the index. → [index-schema §4](index-schema.md#4-visual_timeline--the-veyo-gated-heart)

**whisper.cpp** — local, CPU-side speech-to-text; produces the transcript without audio leaving the device. → [tech-stack §2](tech-stack.md#2-component-by-component-choices)
