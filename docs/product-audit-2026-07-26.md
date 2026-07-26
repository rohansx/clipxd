# clipxd product audit — 2026-07-26

Method: live browse of clipxd.com (landing + a real public share page) with agent-browser,
full read of the SPA + recorder + web crate, and a direct inspection of **production data and
logs** on the box (33 clip dirs, `journalctl -u clipxd-web`, `ffprobe` on every stored video).

Everything marked ✗ below is verified against production, not inferred.

---

## 1. What's actually right

- **Instant-link architecture is real and correct.** `POST /ingest/stage` mints the `clp_` id at
  record *start* and writes a `status: recording` stub, so the share URL resolves before a single
  byte of video lands (`lib.rs:1514`). Commit returns in ~1 ms and does assemble+enrich in a
  background task. This is the right shape and it beats Loom's own flow on paper.
- **Chunked upload during recording works.** MediaRecorder 5 s timeslice → `PUT /ingest/stage/:id?seq=N`
  (`useScreenRecorder.ts:306`). At Stop only ≤5 s of tail remains.
- **The index/MCP angle is a genuine differentiator** and nobody else ships it.
- Share page has real depth: chapters, moments, GIF, embed, agent link, QR, @-timestamp discussion.
- Security work is solid: strict CSP + Trusted Types on the SPA, XSS-safe comment rendering,
  HTML-escaped embed attributes.
- Sidebar copy was already corrected to stop claiming "local" when signed into the cloud.

## 2. Verified broken in production

### P0-1 — Every screen recording is **silent**. No mic is ever captured.
`getDisplayMedia({audio:true})` captures *system/tab* audio only; the camera stream is requested
with `audio:false` (`Recording.tsx:124`); `getUserMedia` for a mic exists **only** in voice-only mode.

Evidence: `ffprobe -select_streams a` over all 15 stored screen recordings → **zero audio streams**,
every one. The probe of every `index.json` → `transcript: 0` on **100%** of native screen clips
(only imported videos have transcripts).

Consequences: no narration (the core Loom use case), an always-empty transcript track, weak
LLM titles, no captions, and the "Ask the clip" answers have nothing to ground on. The UI makes
this worse by rendering a **fake mic meter** (`MIC_BARS` is a static sine curve, `Recording.tsx:41`)
that animates while nothing is being recorded.

Fix: `getUserMedia({audio:true})` + mix with display audio through one `AudioContext` →
`MediaStreamDestination`, add a mic device picker + real level meter from an `AnalyserNode`,
default mic **on** for screen mode.

### P0-2 — Auto-zoom dies on exactly the clips whose duration is missing. *(corrected)*

**An earlier draft of this doc said auto-zoom "never fires." That was wrong**, and the correction
matters because it changes the fix. I sampled one clip's `zoom.json` (`clp_445ee98d`: a single flat
keyframe at scale 1.0) and generalised. Measuring all of them says otherwise:

| clip | duration | zoom keyframes | max scale | % of timeline zoomed |
|---|---|---|---|---|
| clp_18bfa626 | 46.6 s | 1396 | **2.00×** | 7% |
| clp_18c05b42 | 18.7 s | 559 | **2.00×** | 64% |
| clp_bc219f7a | 102.0 s | 3051 | **2.00×** | 3% |
| clp_4e203602 | 41.7 s | 1248 | **2.00×** | 4% |
| clp_445ee98d | **0.0** | **1** | 1.00× | 0% |
| clp_18be6ed0 | **0.0** | **1** | 1.00× | 0% |

The engine works, and the cursor-free path already exists: `autofocus::focus_track_from_deltas`
turns veyo's salience regions into a synthetic cursor path + zoom triggers, which is why clips with
no `cursor.json` (i.e. all of them) still push in to 2.0×. What kills it is
`compute_zoom_track` emitting `duration × fps + 1` keyframes — at duration 0.0 that is exactly one
flat keyframe. **Same root cause as P0-4**, so the remux fixes auto-zoom too, and `--repair`
rebuilds the track for clips already on disk.

What's still true from the original finding: no recording has a real cursor track, because `window`
pointer events only fire over the clipxd tab. So the camera follows *content salience*, not the
pointer — good enough that it reaches 2.0× on the action, but a real OS cursor (extension or
desktop app) would still aim it better.

### P0-3 — "Indexing while uploading" is built but **fails on every session in prod**.
```
incremental add_increment failed for session clp_…: ffprobe failed for
/tmp/clipxd-stage-clp_…/video-so-far.webm (continuing)
```
This line appears for *every* recording session in the journal. So each chunk's incremental pass
no-ops and all indexing collapses back onto post-commit enrichment — the exact "indexing is slow"
symptom.

Root cause: a concatenated, unfinalized MediaRecorder WebM has no duration and a truncated final
cluster; `ffprobe` exits non-zero and the pass bails. Fix: stop requiring a clean probe — extract
frames with `ffmpeg -fflags +genpts -i <partial> -vf fps=… -f image2` and ignore the missing
duration, or index the newest *chunk* instead of the whole re-concatenated file. Also: this failure
is `eprintln!`-only, so it stayed invisible for a month — it needs to reach the clip status.

### P0-4 — Stored videos have **no duration and no seek index**.
`ffprobe` on `clp_445ee98d/video.webm`: `duration=N/A`, `nb_frames=N/A`, VP8, 30 MB for ~45 s
(~5–8 Mbps). Nine `complete` clips carry `metadata.duration: 0.0`.

Consequences: viewers can't scrub reliably, chapter/moment seeking is guesswork, the
`VideoObject` JSON-LD ships a wrong duration, thumbnails/GIFs are unreliable, and raw VP8/VP9
WebM is a compatibility risk on Safari/iOS — for a share-first product that's the recipient's
device half the time.

Fix: at commit, remux once — `ffmpeg -i in.webm -c copy -movflags +faststart out.mp4` when the
codec allows, else remux WebM so cues+duration are written. Better still: request
`video/mp4;codecs=avc1` from MediaRecorder when supported (Chrome does) → H.264 out of the box,
Safari-safe, and a `-c copy` faststart remux costs nothing. Drop the bitrate to ~3 Mbps; 8 Mbps
is 3× Loom for no visible gain.

### P0-5 — Chunk uploads are fire-and-forget; a dropped chunk is unrecoverable data loss.
`fetch(...).catch(() => {})` (`useScreenRecorder.ts:316`) — no retry, no ack, no server-side
completeness check at commit. Only chunk 0 carries the WebM header, so losing it destroys the
whole recording.

Evidence: `clp_18c0982c1f38076c8606210e/video.webm` → `EBML header parsing failed / Invalid data
found`. That clip is dead on the box right now.

Fix: retry with backoff, have the server ack received seqs, resend gaps before commit, and make
commit validate `chunk-000000` + contiguity before promoting the stub.

### P0-6 — Broken clips accumulate and never resolve.
Six clip dirs have **no `index.json`** (`clp_1b345028`, `clp_5ee8108c`, `clp_8a05d803`,
`clp_8a7c07dd`, `clp_9b381a10`, `clp_f8424a7b`, `clp_f851ece6`); `clp_1a8b8220` has been stuck at
`status: enriching` since well before today, so its card spins forever. Two clips sit at `partial`
with nothing in them.

Fix: a reaper that fails out stage sessions past a deadline, a hard timeout that moves
`recording`/`enriching` → `failed` with a reason, and a "this recording didn't survive — here's why"
card instead of an eternal spinner.

## 3. Infra / enterprise-grade gaps

- **The service was OOM-killed on Jul 10** (`clipxd-web.service: Failed with result 'oom-kill'`) on a
  single 4 GB Hetzner VM. `concat_chunks` also reads *every* chunk into a `Vec<u8>` and rewrites the
  whole video-so-far on **every** chunk PUT — O(n²) I/O and O(n) RAM in recording length. A 30-minute
  recording is 360 rewrites of a growing multi-GB buffer. Append to a single file instead, and cap
  concurrent enrichment by memory, not just a 2-slot semaphore.
- **Single box, no queue, no autoscale, no health-gated deploy.** Enrichment competes with request
  serving on the same 4 GB.
- **The LLM cascade is broken.** Journal: Ollama 429/timeout, NVIDIA `z-ai/glm4.7` **410 EOL**,
  `moonshotai/kimi-k2.6` 404, Gemini key invalid. Titles/descriptions are therefore best-effort.
  Make the whole cascade env-configurable (only Ollama is today), add a startup health probe, and
  fall back to a deterministic title from OCR/filename rather than "Screen recording".
- **`transcriber=null`** in every recent `enrich backends:` line.
- **Any clip id is publicly readable without auth** — I loaded a real clip's share page while
  signed out. There is no private/unlisted/expiring/password/domain-restricted option, no viewer
  identity, no audit log, no retention policy, no SSO. All of those are table stakes for the word
  "enterprise".
- Share pages have **no CSP** (known follow-up) and pull **fonts from gstatic** — a third-party
  request on every viewer's page load, which enterprise reviews flag.

## 4. Product / UX findings

**The share page — the surface that actually sells the product — is the weakest one.**

- It uses a raw `<video controls>`: default browser chrome, no speed control, no captions, no
  keyboard shortcuts, while the *internal* app has a far better liquid-glass player. That's
  backwards — the public page should get the good one. *(Correction: it does set `poster` and
  `preload="metadata"`, and `/clip/:id/thumbnail` returns a valid 169 KB JPEG — the black
  rectangle in my screenshot was that clip's genuinely dark first frame, not a missing poster.)*
- The top bar carries an `agent-queryable` pill (this is the navbar copy you flagged). Dev-facing
  affordances — index.json, MCP url, agent link, Copy index — should collapse into one
  "For agents ⌄" disclosure. Recipients want: title, play, chapters, comment, copy link.
- **"Chapters" aren't chapters.** They're per-frame captions: 8 entries in 46 s, all beginning
  "A screenshot shows a dark desktop with…". Real chapters = merge adjacent near-identical frames,
  then one ≤6-word title each, ~1 per 30–60 s. Ban screenshot-speak in the caption prompt.
- **ON-SCREEN TEXT is a public panel of raw OCR noise** — `< e & clipxd.com Qo`, `(@ nec) 00:00 en +
  1086`, 500+ rows, most stamped `0:00`. OCR belongs in the *search index*, not on the viewer's page.
  (The 0:00 stamping is downstream of the duration bug.)
- The empty-transcript state is handled well in-app ("No transcript — this clip has no audio
  track.") — it just never explains *why* or offers the fix, because there is no mic control to
  point at.

**Copy that isn't true (each one erodes the trust the index story depends on):**

| Where | Says | Reality |
|---|---|---|
| `Recording.tsx:606` | "on device · 0 px egress" | hosted; pixels go to the box |
| `Recording.tsx:604` | "index forms on stop" | incremental indexing is the whole design (and is currently failing) |
| `Recording.tsx:41` | animated mic meter | no mic is recorded at all |
| `Sidebar.tsx:129` | "MCP server · connected" | hardcoded string, never checked |
| `Library.tsx:227` | "every one queryable from its link" | true only where enrichment produced something |

**Flow gaps vs Loom/Cap:**

- No desktop app → no full-screen cursor, no system-wide audio, no global hotkey to start/stop.
  The browser recorder can't be the whole product.
- No trim/crop on the *share* page (the editor is in-app only) and no "replace/re-record" loop.
- No post-record "who should see this" step, no notify-by-email, no viewer analytics beyond a count.
- The library has no search over transcript/OCR content (only title filter), no folders, no bulk ops.
- No mobile view for the share page.

## 5. What I'd do, in order

**Week 1 — make one recording actually good.**
1. Mic capture + real level meter (P0-1).
2. Remux + duration + faststart, drop to ~3 Mbps, prefer H.264 when available (P0-4).
3. Fix `add_increment`'s probe so indexing-during-upload really runs (P0-3).
4. Chunk retry + commit-time completeness validation (P0-5).
5. Delete the false copy in §4's table.

**Week 2 — make the zoom real (this is the "wow").**
6. Salience/OCR-derived attention track feeding `compute_zoom_track` (P0-2b) — works on every clip,
   including imports, with zero new capture.
7. Real cursor track via the extension for tab recordings (P0-2a).

**Week 3 — make the share page the product.**
8. Ship the in-app player on `/clip/:id`: poster, big play, speed, captions, keyboard.
9. Real chapters; move OCR to search-only; collapse agent/dev affordances behind one disclosure;
   drop `agent-queryable` from the nav.
10. Reaper + terminal statuses so nothing spins forever.

**Then — the enterprise words.**
11. Link privacy (unlisted / workspace-only / password / expiry) + viewer identity + audit log.
12. Move enrichment onto a queue off the request box; memory caps; deploy health gate.
13. CSP on share pages; self-host fonts.
14. Desktop app (the only way to match Loom on capture) — or position explicitly as browser-first.

---

## 6. Signed-in walkthrough — clicked, not inferred

Every view exercised in a real browser with a session: Library → clip page (Watch / Split / Read
seams, all six index tabs, moment-seek, playback, cinematic editor, share modal) → Recording →
Import → Ask agent (real question, timed) → Docs → Settings (all cards).

**Correction to §4:** the empty-transcript state *does* exist and reads well — "No transcript —
this clip has no audio track." An earlier draft of this doc said otherwise.

New findings from clicking:

- **`/docs` is unreachable when signed out.** The one view deliberately given a real, crawlable
  URL (`SEO_VIEWS.docs`, `noindex` deliberately off) routes a signed-out visitor to the login card
  and rewrites the URL back to `/`. `curl https://clipxd.com/docs` returns 200 with the landing
  page's `<title>` — crawlers index nothing and prospects can't read the docs before signing up.
- **The entire footer is dead.** Recorder, Agent index, Import from URL, MCP server, Documentation,
  GitHub, Changelog, About, Privacy, Security are plain `<li>` elements with `cursor:pointer`, no
  href, no handler. Clicked Privacy, Changelog and MCP server — nothing happens. There are **no
  Privacy or Security pages at all**, which is a hard blocker in enterprise procurement.
- **Ask across the library took ~75–90 s**, showing only "searching the library…" — no streaming,
  no per-clip progress, no partial results. The answer was genuinely good (it surfaced the
  `NO_FCP` Lighthouse error from a *different* clip than the one asked about), which makes latency
  the only thing standing between this feature and its own demo.
- **The share modal has no privacy controls** — link + Copy + Open watch page and nothing else.
  No visibility scope, expiry, password, domain restriction, or comment toggle.
- **Settings → "This server" prints a public IP** (`LAN address 178.104.122.118`) under the label
  "Anyone on this network can open your unlisted share links" — copy written for local mode, wrong
  and mildly leaky when the server is the shared production box. Same card repeats
  "0 px video egress by default".
- **The Recording view contradicts the product's own Docs.** It says "screen · 1080p · auto-zoom
  on", "cursor-follow auto-zoom — zoom tracks your pointer + clicks", "index forms on stop",
  "on device · 0 px egress" — while `/docs` correctly states Screen mode only sees cursor/clicks
  while the pointer is over the clipxd tab. The honest text is already written; it just isn't the
  text on the recorder.
- **No mic control exists anywhere in the recorder UI** — the controls are Start / Screen /
  Voice only / Camera / Prompter. §P0-1 confirmed from the user's side.
- **Broken clips render as normal cards.** The library's first card is "Screen recording", 0:00,
  placeholder gradient thumb, badge "● indexed", no track badges, no error — indistinguishable
  from a good clip. Several other cards show 0:00 (the duration bug, now user-visible).
- **Duration costs a full download.** Right after pressing play, `video.buffered.end(0)` already
  equals the full 46.6 s of a 30 MB file — the browser fetches the whole thing before it knows the
  duration, so scrubbing waits on the entire download. §P0-4, measured from the viewer's side.
- Good and worth keeping: the in-app player, moment-seek, the cinematic editor (Zoom/Cut/Speed/
  Undo, backgrounds, Render MP4, `.clipxd` export), the Import view's framing, and the BYOK
  settings (NVIDIA / Gemini / Moondream keys + "run captioning locally in my browser"), which are
  live in production now.
