# clipxd — features

> The complete feature surface, each with what it does, why it earns its place, and how we know it works. The governing filter: **does this improve the index, or only the human's viewing experience?** If only the latter, it is a non-goal (see [overview §8](overview.md#8-non-goals-holding-the-line)).

Legend: **★ headline** (the moat) · **adoption** (gets users in) · **moat** (hard to copy) · **table-stakes** (must match Cap/Loom).

---

## 4.1 Quick, Loom-style sharing — *adoption, table-stakes*

Hit record, stop, get a link immediately.

- **Upload-while-recording** so the link is live the moment you stop — no "processing…" wait.
- The link resolves to a **normal watchable video page** *and* the agent-readable index behind the same URL.
- **Local/self-host link by default** (zero egress); hosted durable links are the later commercial option ([overview §7](overview.md#7-sharing-model--the-honest-fork)).

**Acceptance:** stop → shareable URL in < 1s; the video is playable while enrichment is still running; the same URL serves both the human page and the JSON sidecar.

---

## 4.2 Beautiful recording — the cinematic layer — *adoption, table-stakes*

Make a raw screen capture look produced with zero effort. This is the Screen Studio / Cap-Studio-class experience, rebuilt and owned.

- **Cinematic auto-zoom** that follows the cursor and clicks: smooth easing, dwell on the target, anti-jitter so it doesn't lurch on every tiny move.
- **Backgrounds:** gradients, images, blur. **Padding, rounded corners, shadows.**
- **Device mockups:** Safari / Chrome / Arc window frames.
- **Aspect-ratio presets** (16:9, square, vertical for social).

**Boundary:** this layer renders the *human* video only. It never feeds the index — the index is built from raw capture, so beautification can never corrupt what the agent reads. **No editor** (timelines, transitions, effects) — that is how the recorder gets fat ([overview §8](overview.md#8-non-goals-holding-the-line)).

**Acceptance:** auto-zoom tracks a cursor through a real demo without visible jitter or motion sickness; backgrounds/mockups apply in the preview without a render wait; output is a standard MP4.

---

## 4.3 Agent-readable & processable recordings — ★ **the headline** — *moat*

Every clip resolves, from one URL, to a structured object an agent explores **without downloading the video.** Full shape in [index-schema.md](index-schema.md); query surface in [mcp-api.md](mcp-api.md).

The index contains:

| Field | What | Source |
|---|---|---|
| **transcript** | time-aligned, speaker-tagged where possible | whisper.cpp (local) |
| **visual timeline** | captions for *salient moments only* (veyo-gated), each timestamped | veyo-core + caption |
| **on-screen text** | OCR'd (screen) or DOM/a11y-verbatim (browser), searchable, timestamped — errors, code, labels, URLs | OCR / DOM |
| **event track** | cursor / clicks / keystrokes / scroll; in browser mode, DOM mutations + a11y tree + console + network | capture |
| **summary + chapters** | derived, *not* the source of truth | enrich |
| **metadata** | duration, resolution, app/window focus, redaction manifest | pipeline |

Exposed via:
- **MCP server** — `query_clip`, `get_frame_context`, `search_text`, `get_events` so Claude / any agent reasons over it natively.
- **JSON API + per-clip `.json` sidecar** — for non-MCP consumers.

> **Design principle:** *an agent should never need the pixels. If it does, the index failed.*

**Acceptance:** the [headline demo](overview.md#6-the-headline-demo) — paste a link, ask "what error showed up and what was the user doing right before it," get a correct timestamped answer with the agent never fetching the video.

---

## 4.4 Import-from-URL — process recordings that already exist — *adoption, ships first*

Paste a Loom / Cap / arbitrary video URL → run it through enrich → get the same index + MCP query, **no recording required.**

- Fastest path to traction: needs **zero capture code**.
- Solves an immediate, visceral pain: *"stop sending me Looms, I can't pass them to an agent — now I can."*
- Doubles as the **session generator** that gives veyo-core its first real tuning data.

**Acceptance:** a public Loom URL becomes a queryable index; the headline demo works against an imported clip identical to a natively-recorded one.

---

## 4.5 Two capture backends, one schema — *moat*

The agent doesn't care which backend produced the index; the schema is identical. Detail in [capture-backends.md](capture-backends.md).

- **browser** — DOM / a11y / console / network + sparse screenshots. Wins web bug reports; ships first of the two.
- **screen** — veyo delta codec over pixels + cursor/key events + cinematic layer. Wins desktop demos; the realtime path.

**Acceptance:** the same MCP query returns the same *shape* of answer against a browser-captured clip and a screen-captured clip; tests assert schema-identity across backends.

---

## 4.6 CloakPipe in the pipe — *moat, structural differentiator*

Strip PII / secrets from frames + transcript **before any index is shared.**

- For browser mode, redact at the **DOM level before screenshotting** — cleaner and cheaper than post-hoc pixel redaction.
- Produces a **redaction manifest** in the index (what was masked, where) so the redaction is auditable, not silent.
- Wires two owned products together; Cap / Loom / Builder's Clips cannot match it.

Detail in [privacy-and-redaction.md](privacy-and-redaction.md).

**Acceptance:** a clip containing an API key in the terminal and an email in the transcript ships with both masked and both recorded in the manifest; nothing un-redacted is ever served.

---

## 4.7 Searchable library — *moat (compounds over time)*

One index across **every** clip, dictation, and imported video.

- Full-text over transcripts + OCR'd on-screen text + captions.
- Local **SQLite + FTS5** (same pattern as the sibling tools).
- Turns a pile of recordings into a queryable corpus: *"find the clip where the checkout 500 happened."*

**Acceptance:** a single query returns matching clips ranked across the whole library, spanning all three sources, with timestamped hit locations.

---

## Feature → phase → differentiation map

| Feature | Phase | Category | Copyable by Loom/Cap? |
|---|---|---|---|
| 4.4 Import-from-URL | 1 | adoption | hard (needs the index) |
| 4.3 Agent-readable index | 1 | ★ headline / moat | no (needs veyo) |
| 4.5 Browser backend | 2 | moat | partially |
| 4.2 Cinematic layer | 3 | table-stakes | yes (it's their turf) |
| 4.1 Quick sharing | 3 | table-stakes | yes |
| 4.5 Screen backend | 3 | moat | partially |
| 4.6 CloakPipe redaction | 4 | moat | no (needs CloakPipe) |
| 4.7 Searchable library | 5 | moat (compounding) | partially |

The pattern is deliberate: the **table-stakes** features (the recorder experience) are what Loom/Cap already do well, so clipxd matches them but doesn't lead with them. The **moat** features (index, redaction, library) are what no competitor can ship without owning a codec and a redaction layer — which is exactly the [veyo](../../veyo) + [CloakPipe](../../cloakpipe) stack clipxd sits on.
