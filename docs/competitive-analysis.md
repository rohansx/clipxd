# clipxd — competitive analysis

> Why now, and why no incumbent fills the gap. The recorder market is mature for *humans*; it is empty for *agents*. clipxd is positioned in that emptiness. Facts below reflect the 2025–2026 state of the field.

---

## 1. The market in one picture

```
            HUMAN-LEGIBLE ONLY                          AGENT-LEGIBLE
            (watch the video)                           (query the index)
   ┌──────────────────────────────────┐      ┌────────────────────────────────────┐
   │  Loom        — async video msg     │      │  Builder's Clips — agent-native,     │
   │  Cap         — beautiful, local    │      │                    but cloud/serverless│
   │  Screen Studio — cinematic zoom    │      │                    template, no codec  │
   │  CleanShot, Kap, OBS, …            │      │                    no redaction, not    │
   └──────────────────────────────────┘      │                    local-first          │
                                              │  ┌──────────────────────────────────┐ │
            LOCAL-FIRST  ◄───────────────────────┤  clipxd — local-first, owned       │ │
                                              │  │  recorder + on-device index +      │ │
                                              │  │  redaction. The index IS the product│ │
                                              │  └──────────────────────────────────┘ │
                                              └────────────────────────────────────────┘
```

Everyone on the left makes a great *human* artifact. The one player on the right is cloud-shaped and lacks an on-device codec and redaction. **clipxd is the only point that is local-first, owned, agent-legible, and privacy-guarded at once.**

---

## 2. The incumbents, specifically

### Loom — the human-only giant, now squeezing
- **What it is:** the category-defining async video messaging tool; record-and-share-a-link.
- **2025–2026 reality:** acquired by Atlassian; billing restructured. The free **Creator Lite** tier is being discontinued — accounts created after Feb 2026 don't have it, and existing free Creator Lite users get **auto-upgraded to paid seats**. Business is ~$12.50–18/user/mo. Post-migration users report lag, audio-sync issues, failed uploads.
- **Why it's the opening:** a Loom is a sealed human artifact. You **cannot hand it to an agent** — no transcript API an LLM can reason over from the link, no event track, no on-screen-text index. As it gets pricier and clunkier, the "stop sending me Looms I can't process" pain sharpens. That pain is clipxd's wedge ([features §4.4](features.md#44-import-from-url--process-recordings-that-already-exist--adoption-ships-first)).

### Cap (cap.so) — the closest on capture, wrong license to copy
- **What it is:** the open-source Loom alternative. Genuinely excellent. **Rust + Tauri** (low CPU, no Electron), three modes — **Instant** (upload-while-recording, link ready on stop), **Studio** (local full-quality, then backgrounds/padding/corners/shadows/cursor effects), **Screenshot**. Adds AI title/transcript/chapters/summary; threaded comments + reactions on the share page. macOS + Windows.
- **License:** **AGPL-3.0** for the app; the **`scap` and `cap-camera` crate families are MIT.**
- **Why it's not the answer — and why it constrains us:** Cap is a *human* recorder with AI *garnish*, not an agent-queryable index — its transcript/summary are features of the share page, not a structured object an agent drives over MCP, and there's no event track, no on-device salience codec, no redaction. And its AGPL license means **we must not port its app source** (an AGPL derivative forecloses a closed hosted tier). We *can* reuse its MIT capture crates (`scap`) and we **interoperate by importing Cap recordings** rather than forking ([licensing.md](licensing.md)).

### Screen Studio (and the cinematic-zoom category) — beautiful, human-only, now subscription
- **What it is:** the reference for cinematic auto-zoom, smooth cursor motion, backgrounds, device mockups. The "make my demo look produced" tool. (This is the category the overview calls the "cinematic layer" / "openvid" reference.)
- **2025–2026 reality:** moved from an **$89 one-time** license to **subscription (~$20–29/mo)**, which spawned a wave of one-time-priced alternatives (CursorClip, ScreenBuddy, Borumi, FocuSee) and free ones (OBS, Screenforge).
- **Why it's not the answer:** purely about the *human* output. Zero agent legibility, no index, no redaction. clipxd **matches** this layer (clean-room, owned — [features §4.2](features.md#42-beautiful-recording--the-cinematic-layer--adoption-table-stakes)) and treats it as table-stakes, not the product.

### Builder's Clips — same thesis, wrong shape
- **What it is:** the one competitor with the *agent-native recording* thesis.
- **Why it's not the answer:** it's a **cloud serverless framework template**, not a local-first product. **No on-device codec** (so no cheap salience gate — the ~2.6B-tokens/day problem is unsolved), **no redaction**, **not local-first** (imagery leaves the box). It validates the thesis without occupying clipxd's position.

---

## 3. The feature matrix

| | Loom | Cap | Screen Studio | Builder's Clips | **clipxd** |
|---|---|---|---|---|---|
| Instant share link | ✅ | ✅ | ➖ | ✅ | ✅ |
| Cinematic auto-zoom / beautify | ➖ | ✅ | ✅✅ | ➖ | ✅ |
| Local-first / self-host | ❌ | ✅ | ✅ (app) | ❌ | ✅ |
| Open source | ❌ | ✅ (AGPL) | ❌ | ~ (template) | ✅ (Apache core) |
| **Agent-queryable index (from URL)** | ❌ | ❌ | ❌ | ✅ | ✅✅ |
| **On-device salience codec** (cheap index) | ❌ | ❌ | ❌ | ❌ | ✅ (veyo) |
| **PII/secret redaction in the pipe** | ❌ | ❌ | ❌ | ❌ | ✅ (CloakPipe) |
| Import others' videos → index | ❌ | ❌ | ❌ | ❌ | ✅ |
| Searchable cross-clip library | ➖ | ➖ | ❌ | ➖ | ✅ |
| Browser DOM/console/network capture | ❌ | ❌ | ❌ | ➖ | ✅ |

`✅✅` = best-in-class · `✅` = yes · `➖` = partial/weak · `❌` = no.

The bottom five rows are clipxd's territory. **No competitor has more than one of them; clipxd has all five** — because it's the only one sitting on an on-device codec (veyo) and a redaction layer (CloakPipe).

---

## 4. Why the moat holds

A fast-follower would have to:
1. Build or license an **on-device salience codec** that makes per-frame agent legibility affordable (the veyo problem — hard, and the reason naive "caption every frame" costs ~2.6B tokens/day).
2. Build a **redaction layer** good enough that an index is safe to share (the CloakPipe problem).
3. Resist becoming a fat editor/analytics product (Loom's and Screen Studio's gravity).
4. Do it **local-first** (cuts against the cloud-template instinct Builder's Clips followed).

The recorder is copyable; Cap proves that. The *index over an owned codec with redaction, local-first* is not a weekend clone — it's two additional products (veyo, CloakPipe) under it. That's the moat ([features: differentiation map](features.md#feature--phase--differentiation-map)).

---

## 5. Positioning statement

> **clipxd is the screen recorder for the agent era.** It matches Cap's capture and Screen Studio's polish, but the recording it produces is a structured index an agent can query from the URL — transcript, on-screen text, UI events, and salient moments — with no raw imagery ever leaving your device. Loom can't be read by an agent; Cap's license can't host it closed; Builder's Clips isn't local-first; none of them redact. clipxd is the only one where **the index is the product.**

---

*Sources for the market facts above:*
- [Cap — about / repo (AGPL, scap MIT, Rust+Tauri, modes)](https://cap.so/about) · [github.com/CapSoftware/cap](https://github.com/CapSoftware/cap)
- [Screen Studio pricing & alternatives (subscription shift)](https://www.uneed.best/alternatives/screen_studio)
- [Loom pricing 2026 / Atlassian billing changes / Creator Lite discontinuation](https://supademo.com/blog/loom-pricing)
- [rrweb — DOM recording vs video (browser-backend basis)](https://github.com/rrweb-io/rrweb)
