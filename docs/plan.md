# clipxd — build plan

> The execution rationale: *why* the phases are ordered as they are, how the work splits into streams, and what the first concrete weeks of Phase 1 actually look like. The phase scopes/gates live in [phases.md](phases.md); the milestone arc in [roadmap.md](roadmap.md).

---

## 1. The ordering principle

Three forces decide the order:

1. **Prove the moat before building the table-stakes.** The index is the differentiator; the recorder is catch-up. So the *first* thing built is the thing that proves the index can be queried (import), and the *last* of the core is the recorder (screen). Building the recorder first would be months of well-understood work before validating the one risky bet.
2. **Cheapest-and-most-valuable first.** Import needs zero capture code and solves a felt pain ("I can't hand this Loom to an agent"). Browser is the highest-*volume* real use case (bug reports) and the *easiest* capture. Screen is the hardest (realtime) and waits.
3. **De-risk the external dependency early.** The whole index leans on veyo-core's salience gate. Phase 1 import doubles as a **session generator** for veyo's eval/tuning ([roadmap](roadmap.md#the-one-dependency-that-can-reorder-everything)). We retire codec risk while shipping value, instead of waiting on the codec or building blind.

Result: **import → browser → screen → privacy/host → library → crates.** Risk decreases as effort increases — the opposite of the naive "build the recorder, then bolt on AI."

---

## 2. Workstreams

The work isn't a single line; it's a few parallel streams that meet at integration points.

| Stream | Spans | Owner concern |
|---|---|---|
| **A — pipeline** | Phase 1→ | raw-session → veyo → enrich → CloakPipe → store. The spine; everything plugs into it. |
| **B — sources** | 1 (import) → 2 (browser) → 3 (screen) | the three capture/import frontends, each emitting the raw-session contract. |
| **C — agent surface** | Phase 1→ | `clipxd-mcp` + JSON API + sidecar; grows as streams in the index grow. |
| **D — engine integration** | ongoing | tracking [veyo](../../veyo) + [CloakPipe](../../cloakpipe) versions; feeding veyo real sessions. |
| **E — recorder UX** | Phase 3→ | cinematic layer, share page polish. The thin human-facing shell. |

Streams A and C are stable early and evolve additively (new index streams → new tools). Stream B is where each phase's new frontend lands. Keeping the **raw-session contract** ([capture-backends §4](capture-backends.md#4-the-unified-raw-session-contract)) and the **index schema** ([index-schema.md](index-schema.md)) frozen-but-additive is what lets B grow without churning A/C.

---

## 3. The single source of truth that makes this tractable

`clipxd-index` (the schema crate) defines `index.json` **once** ([architecture §3](architecture.md#3-crate--workspace-layout)). Every frontend targets it; every consumer reads it. This is the lever:

- A new capture backend is "done" when it emits a valid raw session that flows to a valid index — no agent-surface changes needed.
- A new agent tool is a *view* over existing streams — no capture changes needed.
- Adding a stream (future audio-events, emotion, etc.) is additive: old tools keep working ([index-schema §9](index-schema.md#9-invariants-what-consumers-can-rely-on)).

Design the contract first, defend it, and the phases become mostly independent.

---

## 4. Phase 1, concretely — the first build

A pragmatic decomposition of the smallest thing that proves the headline:

1. **Schema crate (`clipxd-index`).** Define `index.json` + the raw-session contract in Rust types. Write the schema-identity tests *now* (they'll guard Phases 2–3). This is the keystone — build it first.
2. **Import (`clipxd-import`).** URL → download → demux audio + sample frames → raw session. Start with public Loom/Cap/MP4 URLs.
3. **Pipeline wiring (stream A).** Raw session → veyo-core (gate) → veyo-enrich (whisper.cpp transcript + caption salient frames + OCR) → `index.json`. CloakPipe is a stubbed no-op manifest this phase.
4. **MCP server (`clipxd-mcp`).** Implement `query_clip` + the four primitives over one clip ([mcp-api §1](mcp-api.md#1-mcp-server)).
5. **Share page + sidecar (`clipxd-web`).** Minimal: a video player and `/index.json`.
6. **Prove it.** Run the [headline demo](overview.md#6-the-headline-demo) end to end on ≥3 real clips. Capture those sessions for veyo tuning.

Sequence within the phase: **1 → (2 ∥ 3) → 4 → 5 → 6.** The schema gates everything; import and pipeline can progress in parallel once it exists; the agent surface and share page follow; proof closes the phase.

**Definition of done for Phase 1:** a stranger pastes a Loom link into Claude (pointed at `clipxd-mcp`) and gets a correct, timestamped answer about the clip without the video being downloaded — and we have ≥3 real sessions banked for veyo.

---

## 5. Codec-gated branch (the one real fork in the plan)

At the start of Phase 1 there is exactly one decision that changes execution, driven by veyo's status:

- **veyo at gate** (recall ≥ 0.9 @ emission < 1% on ≥3 sessions): wire veyo-core's real salience gate; the visual timeline is sparse and cheap as designed.
- **veyo not yet:** ship Phase 1 with the gate in **degrade mode** — denser/fixed-interval captioning (higher token cost, identical schema). Import is *still worth shipping* because it produces the sessions that move veyo to gate. Re-evaluate before Phase 3 (screen capture is where a weak gate hurts most — realtime cost).

Either way Phase 1 ships. The branch only changes *cost*, never the *contract*. This is the de-risking move in action. Full risk treatment in [risks-and-open-questions.md](risks-and-open-questions.md).

---

## 6. Integration cadence with the sibling engines

- **veyo** ships and versions independently ([overview §2](overview.md#2-veyo-is-the-engine-clipxd-is-the-product)). clipxd pins a veyo version and upgrades deliberately; feeds sessions back upstream.
- **CloakPipe** is a stubbed interface until Phase 4, then wired as the redaction pass ([privacy-and-redaction.md](privacy-and-redaction.md)). Stubbing it early keeps the manifest field in the schema from day one, so turning it on is a swap, not a schema change.

Both are vendored/pinned, not forked — clipxd consumes them as the engine and the guard, and owns only the recorder, the pipeline wiring, the index contract, and the agent surface.

---

## 7. What "done" looks like for the whole product

When Phases 1–5 hold their gates:

- Any recording — captured (screen/browser) or imported — becomes a queryable `index.json` behind its share URL.
- An agent answers questions about it from text, with citations, never touching pixels.
- Nothing sensitive leaves the box; redaction is auditable.
- The recorder is fast, beautiful, and *thin* — no editor, no analytics, no admin.
- The whole library is one searchable corpus.

That is the product: **record once; humans watch it; agents read it.** Phase 6 is pure leverage on top.
