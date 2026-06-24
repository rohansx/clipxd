# clipxd — roadmap

> The high-level arc and the gates between stages. For the detailed scope of each phase see [phases.md](phases.md); for *why this order* and the week-by-week execution see [plan.md](plan.md).

---

## The arc in one line

**Prove the index can be queried (import) → make capture cheap and exact (browser) → own the recorder (screen) → harden privacy and offer hosting → make the corpus searchable → spin out reusable parts.**

Adoption comes from the recorder; the moat is the index. The roadmap front-loads the *moat* (you can query a clip in Phase 1) and back-loads the *table-stakes recorder* (Phase 3) — because the index is what no one else has, and the recorder is catch-up work we already know how to do.

---

## Milestones & success gates

| Phase | Milestone | Ships | **Gate to advance** |
|---|---|---|---|
| **1** | Import + enrich | URL → `index.json` + MCP query. No capture. | The [headline demo](overview.md#6-the-headline-demo) works against an imported clip, reliably, on real clips. |
| **2** | Browser backend | DOM/a11y/console/network trace → same schema. | A real web bug-report flow produces an index an agent answers correctly; schema-identity with Phase 1 proven by tests. |
| **3** | Owned recorder | Rust screen capture + cinematic layer → veyo pipeline → local link. | Record-to-shareable-link < 1s; cinematic auto-zoom is demo-grade; the index matches browser quality. |
| **4** | Privacy + hosting | CloakPipe pass in the pipe; optional hosted durable links. | A clip with secrets ships fully redacted with an auditable manifest; a hosted link survives the recorder's box being offline. |
| **5** | Searchable library | One FTS index across all clips/imports/dictation. | A single query returns ranked hits across the whole corpus, all sources. |
| **6** | *(optional)* Reusable crates | Cinematic auto-zoom (and/or veyo bits) as standalone crates. | The auto-zoom crate is usable by a third party without clipxd. |

Each gate is a *go/no-go*: don't start the next phase until the current gate holds. The discipline mirrors veyo's "prove the codec offline before building the daemon" gate ([veyo phases](../../veyo/docs/phases.md)).

---

## The one dependency that can reorder everything

**veyo-core's recall.** The whole index leans on veyo deciding *which moments are salient*. The codec's gate must hit **recall ≥ 0.9 at emission < 1% on ≥3 real sessions, CPU-only** ([veyo status](../../veyo/README.md)).

- If veyo is **at gate** → Phase 1 ships on the real codec immediately.
- If veyo is **not yet** → Phase 1 still ships, but the visual timeline degrades to denser/fixed-interval captioning (higher cost, same schema), and **clipxd import generates the very sessions veyo needs to tune on.** The dependency is two-way: clipxd feeds veyo data, veyo sharpens clipxd's index. See [risks-and-open-questions.md](risks-and-open-questions.md).

This is why **import ships first** even though it's not the flashiest feature — it de-risks the codec dependency by producing training/eval sessions while delivering value.

---

## What stays out of the roadmap (forever, by design)

- A full video editor (timelines, transitions, effects). → [overview §8](overview.md#8-non-goals-holding-the-line)
- Analytics / engagement dashboards (Loom's moat, irrelevant to the index).
- Team / workspace / SSO at the product core (single-user local-first is the thesis; org features are a hosted-tier concern at most).
- Anything that improves only the human's viewing experience and nothing in the index.

Holding this line is *the* strategic choice — it's how the recorder stays thin while still being better than the fat incumbents ([overview §5](overview.md#5-what-makes-the-recorder-thin-fast-useful)).

---

## Indicative sequencing

No hard dates (single-builder, codec-gated), but the intended *order of effort*:

```
  ── Phase 1 ──────► Phase 2 ──────► Phase 3 ──────► Phase 4 ──────► Phase 5 ──► Phase 6
   import+enrich     browser         owned           CloakPipe +     library     (optional
   (no capture)      backend         recorder        hosted tier     FTS         crates)
        │                                                                          
        └─ also: generate real sessions → feed veyo-core tuning (parallel, ongoing)
```

Phases 1–2 are mostly **plumbing + integration** (fast, low-risk). Phase 3 is the **hard realtime engineering**. Phases 4–6 are **hardening and leverage**. The riskiest work (realtime screen capture + cinematic layer) is deliberately *not first* — it waits behind two phases that prove the index on easier inputs.
