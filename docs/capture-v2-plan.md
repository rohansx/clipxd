# Capture v2 — semantic capture, faster indexing, and where CRDTs actually belong

Written 2026-08-02, after measuring the current pipeline in production.

## The thesis

We run OCR over pixels to recover text the browser would hand us exactly, for free. That trade
costs a 3.4 GB memory floor, minutes of enrichment per clip, and a whole class of hallucination —
for a *worse* answer than `document` already has. For browser recordings, deriving semantics from
pixels is a strictly dominated strategy.

"Capture from paint" is close but not the lever. Paint gives better *pixels* (compositor frames
with scroll offsets and timestamps via `Page.startScreencast`); it carries no meaning. The meaning
is in the DOM and accessibility trees. What we want is **DOM/AX capture keyed to the video's
timeline**, with pixels kept for humans.

## Where we actually are

The Rust trace schema (`crates/clipxd-browser/src/trace.rs`) already defines `DomSnapshot`,
`DomMutation`, `A11yText` and `Screenshot`. **The capture client emits none of them.**
`extension/content.js` is 117 lines hooking `click`, `change`, `keydown`, `popstate`,
`hashchange`, plus a console relay. A click yields:

```
target: "button.btn-primary"      label: first 80 chars of innerText
x: 412  y: 233                    (clientX/Y, viewport pixels)
```

Which is why every share page renders `CLICK AT (51%, 36%)`. The schema is ahead of the client;
most of this plan is closing that gap rather than designing anything new.

## What Chrome will give us

**Content script — no permission banner, works today**

| API | What it yields |
|---|---|
| `TreeWalker` over text nodes + `getBoundingClientRect` | every visible string *with its rect* — exactly what OCR tries to reconstruct |
| `MutationObserver` (subtree, attributes, characterData) | what changed and when — free salience |
| `IntersectionObserver` | what is actually on screen vs scrolled past |
| pointer events + element chain | role, accessible name, text, href, sibling index |
| `scrollingElement`, `visualViewport` | scroll and zoom state per moment |

**`chrome.debugger` + CDP — richer, but shows the debugging banner**

`DOMSnapshot.captureSnapshot` (whole layout tree, text + rects, one call, crosses iframes),
`Accessibility.getFullAXTree` (roles/names/values), `Page.startScreencast` (frames carrying
`scrollOffsetX/Y`, `pageScaleFactor`, `timestamp`).

**Decision: content script first.** The debugger banner would undermine the product for a marginal
gain. Revisit only if cross-origin iframe content turns out to matter.

## What to take from PostHog / rrweb

Session replay there records no video at all, and its architecture maps onto ours almost 1:1:

1. **One full snapshot, then incremental mutations** keyed by node id. Payloads are tiny beside video.
2. **Element chains, not coordinates.** This is what turns "click at (51%, 36%)" into
   "clicked *Generate credentials*".
3. **Periodic full re-snapshots ("checkouts")** so replay can start mid-session. This maps exactly
   onto our 5-second chunks: one checkout per chunk boundary makes every chunk independently
   indexable — which is what "indexing keeps up with recording" actually requires.
4. **Masking at capture**, by selector and input type, rather than blurring rectangles afterwards.
   Strictly better than pixel redaction and it makes CloakPipe precise instead of probabilistic.

## Phases

Ordered by payoff per unit of work. Each phase ships and is verifiable on its own.

### Phase 0 — element chains (small, immediate)
Replace `describe()` in `content.js`: role + accessible name + visible text + rect + href + a
stable-ish selector, instead of `tag.class` + x/y. Keep x/y as a fallback.
*Payoff:* every event row on every share page becomes a sentence. Also feeds better titles and
chapter labels, since the deep pass finally has verbs and nouns to work with.
*Risk:* none to existing clips — additive fields, `#[serde(default)]` already on the Rust side.

### Phase 1 — visible-text snapshot per chunk (the big one)
TreeWalker over visible text nodes at each chunk boundary → `A11yText` / `DomSnapshot` events with
rects. Server-side: when a clip carries DOM text, **skip OCR entirely** and build `on_screen_text`
from it.
*Payoff:* removes the 3.4 GB OCR floor and most of the 2–4 minutes of enrichment for browser
recordings; text becomes exact rather than approximate; the `minmax.mozilla.mx` hallucination class
disappears from on-screen text.
*Watch:* snapshot size on text-heavy pages — diff against the previous snapshot, don't resend.

### Phase 2 — mutation-driven salience
`MutationObserver` volume × affected viewport area replaces pixel diffing as the moment detector
for browser clips.
*Payoff:* exact and nearly free; no frame decode to decide what mattered. The VLM captioner becomes
optional polish ("what does this look like") rather than the source of truth.

### Phase 3 — masking at capture
Selector/input-type masking before anything leaves the tab; password and payment fields already
have a crude version in `isSecret()`. Extend to rrweb-style rules plus a user-visible allow/deny.
**This gates broad rollout** — DOM capture captures strictly more than pixels, including PII that
OCR would have mangled into uselessness.

### Phase 4 — hybrid logical clocks for multi-source merge
Extension trace, page cursor track, video chunks, and later a desktop agent each have their own
clock. Stamp every event with an HLC per source and merge on ingest.
*Not a CRDT* — a few lines for causal ordering across sources.

### Phase 5 — CRDT layer for co-authored artifacts
See below.

## CRDTs: where they belong and where they don't

**Not the capture stream.** A recording is a single-writer, append-only log with one clock and a
total order. There is no conflict to resolve. Chunk sequence numbers plus idempotent writes already
handle out-of-order arrival — and the bug we actually hit there was a *missing completeness check*,
not a merge conflict. A CRDT would be machinery bought for a problem we don't have.

**Not `index.json` as a whole.** It is machine-generated and regenerated wholesale by enrichment.
Last-writer-wins is correct and simpler.

**Yes for artifacts humans and agents co-author**, which is a real and growing set:

- comments and annotations
- the cinematic edit project (zoom regions, cuts, speed ramps)
- transcript corrections
- agent-written annotations landing while a human edits chapters

These are concurrently edited, often offline, and merging them by hand is exactly the problem CRDTs
solve. Yjs (with `y-websocket`) or Automerge over *those documents only*.

The line to hold: **CRDTs for what humans and agents co-author; append-only logs for what machines
observe.**

## Hosting: what belongs on Dokploy

Three candidates, in order of value:

1. **The enrichment worker.** The outstanding audit item is "enrichment off the request box" — the
   4 GB Hetzner VM has been OOM-killed nine times, and enrichment still shares a process with
   request serving. A worker container with its own memory limit, pulling jobs, is a natural
   Dokploy service and finally decouples the two.
2. **The CRDT sync server** (Phase 5) — a small stateful websocket service; textbook Dokploy.
3. **A trace-ingest endpoint**, if browser traces outgrow the current `/ingest/browser-trace` path.

The clipxd web service itself can stay where it is; moving it buys nothing today.

## Postgres consolidation on Dokploy

Dokploy provisions a Postgres *service per project* by default, so N projects means N clusters —
each with its own shared_buffers, WAL, autovacuum and background workers. On a small host that is
real memory spent on duplication rather than data.

**One cluster, one database per project, is the right default.** Recipe:

```sql
CREATE ROLE app_myproj LOGIN PASSWORD '…';
CREATE DATABASE myproj OWNER app_myproj;
REVOKE CONNECT ON DATABASE myproj FROM PUBLIC;
```

Then point each project at `postgres://app_myproj:…@postgres-shared:5432/myproj`. In Dokploy this
means creating one Postgres service and setting each app's `DATABASE_URL` by hand instead of
letting it provision a per-project database. Check that the services share a docker network
(Dokploy's `dokploy-network`) so the service name resolves across projects.

**Database per project, not schema per project.** Postgres can't query across databases, which here
is a feature: it's a hard isolation boundary, and `pg_dump`/restore works per project.

Add PgBouncer once total connections get into the dozens — one cluster means one connection budget,
which is the main thing that bites.

**When not to share:** a project needing a different Postgres major version, one with compliance
isolation requirements, or one whose write volume would starve the others. Also note the blast
radius: one cluster means one upgrade window and one bad query can affect every project.

Not relevant to clipxd itself, which uses SQLite (`rusqlite`, bundled) — a single file next to the
clips. No plan to change that; the workload is one box and a handful of writers.
