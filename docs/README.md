# clipxd — docs

> *Record once. Humans watch it. Agents read it.*
> A fast, Loom-style screen recorder with cinematic capture whose shared link is **fully legible to an LLM** — transcript, on-screen text, UI events, and salient visual moments, all queryable from the URL. Owned, local-first recorder; powered underneath by the **[veyo](../../veyo)** visual-event codec.

**Product name:** `clipxd` (clip + index — the *index* is the product).
**Engine:** [veyo](../../veyo) (separate tool, sits beneath).
**Sibling:** [CloakPipe](../../cloakpipe) (PII/secret redaction in the pipe).
**License intent:** open-core — Apache-2.0 core + optional commercial hosted/compliance layer.

---

## Read in this order

| # | Doc | What it answers |
|---|-----|-----------------|
| 1 | [overview.md](overview.md) | What clipxd is, the one-sentence thesis, the headline demo. **Start here.** |
| 2 | [competitive-analysis.md](competitive-analysis.md) | Why now — Loom, Cap, Screen Studio, Builder's Clips, and the gap clipxd fills. |
| 3 | [features.md](features.md) | Every feature, broken down, with acceptance criteria. |
| 4 | [architecture.md](architecture.md) | The pipeline, components, crate workspace, deployment topology. |
| 5 | [capture-backends.md](capture-backends.md) | Browser (DOM/a11y) vs screen (veyo) capture — two backends, one schema. |
| 6 | [index-schema.md](index-schema.md) | **The core artifact.** The clip-index JSON every agent queries. |
| 7 | [mcp-api.md](mcp-api.md) | MCP server tools + JSON API + per-clip sidecar. |
| 8 | [privacy-and-redaction.md](privacy-and-redaction.md) | CloakPipe in the pipe; the redaction model; local-first guarantees. |
| 9 | [tech-stack.md](tech-stack.md) | Technology choices and the rationale behind each. |
| 10 | [roadmap.md](roadmap.md) | High-level roadmap, milestones, and success gates. |
| 11 | [phases.md](phases.md) | Phase-by-phase build plan (1→6), scope in/out per phase. |
| 12 | [plan.md](plan.md) | Execution plan — why this order, workstreams, the MVP week-by-week. |
| 13 | [licensing.md](licensing.md) | Open-core split, AGPL avoidance, clean-room rules. |
| 14 | [risks-and-open-questions.md](risks-and-open-questions.md) | What could sink it, and the decisions still open. |
| 15 | [glossary.md](glossary.md) | Terms used across these docs. |

## The one-paragraph version

The screen-recorder market is solved for *humans*: Cap nailed instant + studio capture and cursor auto-zoom; Screen Studio nailed cinematic zoom, backgrounds, and device mockups. Nobody has solved it for *agents*. clipxd rebuilds that capture + beautification layer as one fast, owned Rust tool, and then does the part nobody does: turns every recording into a structured **index** an agent can query **from the URL, without downloading the video.** The recorder gets adoption. The index is the moat.

## The headline demo

> Paste a clip link into Claude. Ask *"what error showed up and what was the user doing right before it."* Get an accurate, timestamped answer grounded in what was on screen and said — **without Claude ever watching the video.**

If that one interaction is reliable, the product is made. Everything else is plumbing.

## How it relates to the rest of the stack

```
            clipxd  ──────────────  the product (recorder + share + index UX)
               │  uses
               ▼
            veyo    ──────────────  the engine (visual-event codec: salience gate, deltas)
               │  feeds clean text into
               ▼
            CloakPipe  ───────────  the guard (PII/secret redaction before any index is shared)
```

clipxd is veyo's **first user-facing surface**. veyo decides *which moments are worth enriching*; CloakPipe makes sure *nothing sensitive leaves the box*; clipxd owns the *recorder, the share layer, and the product*.
