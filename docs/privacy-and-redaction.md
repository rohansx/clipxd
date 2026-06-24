# clipxd — privacy & redaction

> The privacy thesis is not a feature bolted on; it's structural. **No raw imagery leaves the device** (veyo's design) and **nothing sensitive is ever shared** (CloakPipe's job). Together they make an agent-legible recording safe to hand out — something Loom, Cap, and Screen Studio structurally cannot offer, and Builder's Clips skips entirely ([competitive-analysis.md](competitive-analysis.md)).

---

## 1. The two guarantees

1. **Local-first by default.** Capture, the veyo salience gate, local transcription (whisper.cpp), local OCR, and CloakPipe redaction all run **on-device**. In the default mode, nothing leaves your machine at all ([architecture §5](architecture.md#5-where-the-work-runs-trust-boundaries)).
2. **Redacted-before-shareable.** An index is never servable until CloakPipe has passed over it. Whatever the sharing topology — local, tunnel, or hosted — the *content* that crosses any boundary is already clean ([architecture §7](architecture.md#7-deployment-topologies)).

These two together are what let clipxd offer a *shareable* agent-readable artifact without the privacy disaster that "an LLM can read everything in my recording" would otherwise be.

---

## 2. The trust boundary

```
   ON-DEVICE (always)                    │  ROUTED (optional, user-controlled)   │  SHAREABLE (post-CloakPipe)
   ───────────────────────────────────  │  ────────────────────────────────────  │  ──────────────────────────
   capture · veyo-core salience gate ·  │  heavy captioning / large-model        │  index.json · redacted frames ·
   whisper.cpp · OCR · CloakPipe         │  enrichment MAY route out — but ONLY    │  MCP responses · share page
   redaction                             │  on already-CloakPipe-cleaned input     │
                                         │                                         │
   ════════════════════════════════ raw imagery + un-redacted transcript NEVER cross this line ═══════════════════
```

The rule is absolute: **raw imagery and un-redacted transcript never cross into "routed" or "shareable."** veyo guarantees imagery stays on the box; CloakPipe guarantees what's left is clean. A hosted tier or a routed-enrichment option is only safe *because* this boundary holds.

---

## 3. CloakPipe in the pipe

[CloakPipe](../../cloakpipe) is the sibling product: a Rust-native privacy proxy that detects, masks, and (for round-trips) unmasks PII/secrets — 33+ entity types, sub-5ms latency, local-first. In clipxd it runs as the **redaction pass** between enrichment and storage ([architecture §1](architecture.md#1-the-pipeline-end-to-end)).

It redacts across **every stream** of the index:

| Stream | What gets caught | Action |
|---|---|---|
| `transcript` | spoken card numbers, emails, names, secrets | mask token |
| `on_screen_text` | API keys, tokens, PII in OCR'd/DOM text | mask token |
| `visual_timeline` `frame_ref` | secrets visible in a stored salient screenshot | region blur before storage |
| `event_track` | secrets in console output, request bodies, typed keystrokes | mask token |

Every masking is recorded in the **redaction manifest** ([index-schema §8](index-schema.md#8-redaction--the-manifest)) — so redaction is *auditable*, not silent. A consumer can see *that* something was masked, *where*, and *what kind*, without seeing the value.

---

## 4. The browser-mode advantage: redact before screenshot

In browser mode, clipxd has something pixel-only recorders can never have: the **DOM**. So redaction happens **at the DOM level before the screenshot is taken** — strip the SSN node, *then* snapshot ([capture-backends §2](capture-backends.md#2-the-browser-backend)).

- **Cleaner:** the secret is gone from the source, not blurred out of pixels after the fact (no "blur missed a pixel" failure mode).
- **Cheaper:** no post-hoc image inpainting.
- **Exact:** the DOM knows *exactly* which node is the credit-card field; OCR-then-redact only guesses.

This is structurally impossible for Cap/Loom/Screen Studio (they only ever have pixels) and is a direct consequence of the [two-backend design](capture-backends.md).

---

## 5. Why this is a *structural* differentiator, not a checkbox

Anyone can add a "blur secrets" toggle. clipxd's privacy story is structural because:

- It rides on an **owned codec** (veyo) whose core design principle is *no imagery leaves the device* — privacy and cost are fixed by the **same** move (cheap text deltas at the edge).
- It rides on an **owned redaction engine** (CloakPipe) already built and battle-tested for exactly this.
- The **redaction manifest** makes it auditable, which is what a compliance/regulated buyer actually needs (and is the basis of the commercial compliance layer in the open-core split — [licensing.md](licensing.md)).

A competitor can't replicate this without building both a codec and a redaction layer (see [competitive-analysis §4](competitive-analysis.md#4-why-the-moat-holds)).

---

## 6. Phasing

Redaction is **stubbed from Phase 1** (the manifest field exists; the pass is a no-op) and **enforced in Phase 4** ([phases.md](phases.md#phase-4--cloakpipe-pass--hosted-optional-tier--make-it-safe-to-share)). Stubbing early means the schema carries the manifest from day one, so turning redaction on is a *swap*, not a schema change. The hosted tier (also Phase 4) is only offered *after* redaction is enforced — durable external links must never carry un-redacted content.

---

## 7. What clipxd does **not** do

- It doesn't promise to catch *every* secret with zero misses — no redactor can. It catches CloakPipe's 33+ entity types at its tested protection rate, records what it did, and fails safe (when unsure, mask).
- It doesn't send raw frames anywhere in default mode. Routed enrichment is **opt-in** and only on cleaned input.
- It isn't a DLP product. It's a recorder whose index is *safe enough to share with an agent*, with an auditable trail — not a full data-loss-prevention suite.
