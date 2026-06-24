# clipxd — licensing & IP

> Open-core, like the rest of the stack: **Apache-2.0 core + an optional commercial hosted/compliance layer.** The one hard constraint is staying clear of AGPL contamination so the closed hosted tier remains possible. This is a *clean-room recorder*, not a port.

---

## 1. The open-core split

| Layer | License | Contains |
|---|---|---|
| **Core** | **Apache-2.0** | the recorder, the index schema, the pipeline wiring, the MCP server + JSON API, the local store, the cinematic layer |
| **Commercial** | proprietary | hosted durable links (storage + CDN), managed enrichment, compliance/audit tooling around the redaction manifest |

clipxd inherits this split from **veyo** (Apache-2.0 core + commercial managed/compliance layer — [veyo licensing](../../veyo/README.md)). The local-first product is fully open and free; the money is in *hosting* and *compliance*, which are exactly the things that cost ongoing money to run ([overview §7](overview.md#7-sharing-model--the-honest-fork)).

---

## 2. The AGPL constraint — the rule that shapes the build

**Cap is AGPL-3.0.** A Go/Rust translation of Cap's app source would be an **AGPL derivative**, which would force clipxd's hosted service to release its source — foreclosing the commercial tier. Therefore:

- **Do NOT port Cap's application source.** Not the editor, not the share platform, not the app glue. Reimplement the *capture experience* **clean-room** — from the observable behavior, not the code ([overview §11](overview.md#11-licensing--ip-notes)).
- **DO reuse Cap's MIT crates.** The `scap` and `cap-camera` crate families are **MIT-licensed** — these are safe to depend on, and `scap` is in fact the planned screen-capture primitive ([tech-stack §2](tech-stack.md#2-component-by-component-choices)).
- **Interoperate, don't fork.** clipxd *imports* Cap recordings (and Loom, and arbitrary URLs) rather than building on Cap's AGPL codebase ([features §4.4](features.md#44-import-from-url--process-recordings-that-already-exist--adoption-ships-first)). Interop is a feature; forking is a license trap.

---

## 3. The cinematic-layer reference

The cinematic auto-zoom / backgrounds / mockups layer is modeled on the **Screen Studio-class** experience (what the seed overview calls the "openvid" reference).

- **Verify the license of any specific code reference before pulling it in.** If it's permissive (MIT-class), reuse is fine. If not, **reimplement the zoom/background logic clean-room** — the behavior (cursor-follow easing, dwell, anti-jitter, gradient backgrounds, device frames) is not patent-encumbered; only specific source is license-bound.
- The cinematic layer is a strong **clean-room candidate** regardless, and a possible standalone open crate later ([phases.md Phase 6](phases.md#phase-6--optional-reusable-crates--leverage)).

---

## 4. Vendored engines & their licenses

| Dependency | License | Treatment |
|---|---|---|
| **veyo-core / veyo-enrich** | Apache-2.0 (core) | vendored, versioned independently; same open-core owner |
| **CloakPipe** | Apache-2.0 (core) | vendored; commercial compliance layer aligns with clipxd's |
| **scap / cap-camera** | MIT | safe dependency |
| **whisper.cpp** | MIT | safe dependency |
| **rrweb** | MIT | safe dependency (browser capture) |
| **axum / serde / rusqlite etc.** | MIT/Apache-2.0 | standard permissive Rust ecosystem |

Every planned dependency is **permissive (MIT/Apache-2.0)**. The *only* AGPL thing in the neighborhood is Cap's app source — which is precisely what the clean-room rule keeps out.

---

## 5. Why the closed tier must stay possible

The whole commercial model is "free local-first product, paid hosting + compliance." That model **dies** if any core dependency is copyleft-viral (AGPL), because the hosted service would have to open its source. So the licensing discipline isn't bureaucratic — it's what keeps the business model intact:

```
   Apache/MIT deps only  ──►  closed hosted tier stays legal  ──►  open-core model works
        │
        └─ one AGPL port would break the whole chain → hence: clean-room, import-don't-fork
```

---

## 6. Checklist before pulling in any new code

- [ ] Is the license permissive (MIT / Apache-2.0 / BSD)? If yes → fine.
- [ ] Is it AGPL/GPL? If yes → **do not link/port**; reimplement clean-room or interop via import.
- [ ] Is it a *crate dependency* (allowed if permissive) or a *source port* (clean-room only)?
- [ ] Does using it keep the hosted tier closeable? If not → reject.
