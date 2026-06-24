# clipxd — risks & open questions

> What could sink the product, and the decisions still genuinely open. Honest accounting beats optimistic plans. The seed open questions ([overview §12](overview.md#12-open-questions)) are folded in below with treatments.

---

## Risks (ranked by how much they threaten the thesis)

### R1 — veyo-core isn't at gate · **highest, and external**
The entire index leans on veyo deciding which moments are salient. If the codec can't hit **recall ≥ 0.9 at emission < 1% on ≥3 real sessions (CPU-only)** ([veyo status](../../veyo/README.md)), the visual timeline is either too sparse (misses the error) or too dense (cost explodes toward the ~2.6B-tokens/day problem).
- **Why it's survivable:** Phase 1 ships regardless, with the gate in **degrade mode** (denser/fixed-interval captioning, higher cost, *identical schema*). And **clipxd import generates the very sessions veyo needs to reach gate** — the dependency is two-way ([plan §5](plan.md#5-codec-gated-branch-the-one-real-fork-in-the-plan)).
- **Mitigation:** ship import first; bank real sessions; re-evaluate the gate before Phase 3 (screen capture, where a weak gate hurts most because it's realtime).

### R2 — the recorder becomes fat · **high, and self-inflicted**
Every recorder drifts toward editor → analytics → team admin. That drift is exactly what makes Loom and Screen Studio heavy, and it would dissolve clipxd's "thin + fast" advantage.
- **Mitigation:** the non-goals ([overview §8](overview.md#8-non-goals-holding-the-line)) and the governing filter — *does this improve the index, or only the human's viewing experience?* If only the latter, it's out ([features.md](features.md)). This is a *discipline* risk, defended by saying no.

### R3 — local-first sharing is too weak for humans · **medium, structural**
Tunnel/ngrok-class links work for *agent-on-my-machine* and *ephemeral peer view* but are flaky for durable async human sharing (recipient needs your box up; links die) ([overview §7](overview.md#7-sharing-model--the-honest-fork)).
- **Mitigation:** the hosted tier (Phase 4) exists precisely for durable human sharing; it's the commercial layer. The risk is really "does local-first alone satisfy enough users before hosting exists" — and the answer is yes for the *agent* use case, which is the wedge.

### R4 — redaction misses a secret · **medium, reputational**
No redactor catches everything; a leaked key in a shared index is a trust-killer.
- **Mitigation:** CloakPipe's 33+ entity types + **fail-safe (mask when unsure)** + **DOM-level redaction before screenshot** in browser mode + the **auditable manifest** so misses are diagnosable, not silent ([privacy-and-redaction.md](privacy-and-redaction.md)). Default policy is strict.

### R5 — import demand doesn't materialize · **medium, go-to-market**
Phase 1 bets that "paste a Loom URL → queryable index" pulls real demand. If it doesn't, the cheapest path to traction is also the first to fail.
- **Mitigation:** import is cheap to build (zero capture code) so the bet is small; even if demand is soft, it *still* delivers the veyo-tuning sessions (R1). Worst case it's a successful internal tool that de-risks the codec. (This is open question Q4 below.)

### R6 — realtime screen path blows the CPU budget · **medium, technical, Phase 3**
Running the veyo gate *alongside* live capture + cinematic render without dropping frames is the hardest engineering in the product.
- **Mitigation:** it's deliberately **last of the core** ([phases.md Phase 3](phases.md#phase-3--owned-recorder--the-hard-part-realtime-screen-capture--cinematic)); by then the gate is tuned (R1) and proven on non-realtime sources; veyo's own roadmap targets a damage/DMA-BUF optimized path.

### R7 — AGPL contamination · **low if disciplined, fatal if not**
Porting Cap's app source would force the hosted tier open and break the business model ([licensing.md](licensing.md)).
- **Mitigation:** clean-room the capture UX; reuse only MIT crates (`scap`); import-don't-fork; the pre-merge license checklist ([licensing §6](licensing.md#6-checklist-before-pulling-in-any-new-code)).

### R8 — index quality is "almost right" · **the silent killer**
If `query_clip` is *usually* right but sometimes confidently wrong, the headline demo loses trust faster than if it plainly failed.
- **Mitigation:** every answer is **cited with timestamps** ([mcp-api §1.2](mcp-api.md#12-the-headline-interaction-concretely)) so it's checkable; `status: partial/enriching` is honest about incompleteness ([index-schema §9](index-schema.md#9-invariants-what-consumers-can-rely-on)); the design principle *"an agent should never need the pixels — if it does, the index failed"* is a testable bar, not a slogan.

---

## Open questions (decisions not yet made)

### Q1 — Is veyo-core at gate yet?
Gates whether Phase 1 ships on the real codec or in degrade mode. **Treatment:** see R1; not blocking — import ships either way and feeds the answer. *(Seed Q: "recall ≥0.9 at emission <1% on ≥3 sessions yet?")*

### Q2 — MCP: one server per clip, or one indexing the whole library?
**Current lean:** *one server, two scopes* — per-clip addressing always works (a shared clip is queryable standalone), and library-wide search is the same server with the clip id omitted ([mcp-api §3](mcp-api.md#3-one-server-per-clip-or-one-for-the-library)). Not fully settled for the hosted multi-tenant case.

### Q3 — Recorder rendering: native cinematic layer, or webview?
**Treatment:** decided late, low-stakes — the render never feeds the index, so it can't affect the moat ([tech-stack §5](tech-stack.md#5-rendering--the-one-unsettled-choice)). Native = tighter/one-toolchain; webview = faster UI iteration.

### Q4 — Does import-from-URL alone pull enough demand to justify capture work before it ships?
**Treatment:** see R5. Import ships *first* specifically to answer this empirically and cheaply before committing to the harder capture phases.

### Q5 — Auto-zoom: inside the recorder, or a standalone reusable crate?
**Treatment:** build it inside the recorder first (Phase 3); extract to a standalone crate later *if* there's external pull ([phases.md Phase 6](phases.md#phase-6--optional-reusable-crates--leverage)). Don't pre-generalize.

---

## The one decision that reorders everything

**Q1 / R1 (veyo's gate).** Everything else is sequencing within a known plan. Whether veyo is at gate decides only the *cost* of Phase 1, never the *contract* — which is exactly why the plan front-loads the cheap, codec-feeding import phase to retire that risk while shipping value ([plan §5](plan.md#5-codec-gated-branch-the-one-real-fork-in-the-plan)).
