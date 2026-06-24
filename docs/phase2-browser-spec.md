# clipxd Phase 2 — browser backend (spec)

## 0. Scope and invariant

This spec defines the **browser capture backend** for clipxd. It introduces:

- A small, clean-room **browser-trace JSON format** that a capture script emits.
- A new Rust crate, **`clipxd-browser`**, that ingests a trace and produces a Phase-1-identical `Index`.
- A **browser-salience model** that decides which events become `visual_timeline` moments and `summary.chapters`.
- An **optional** Node Playwright/CDP capture script that emits the trace format.

**Schema-identity invariant (load-bearing).** The browser backend MUST populate the *same* `Index` shape as Phase 1 (`source: "browser"` is the only top-level discriminator). The output shape of `visual_timeline[]`, `on_screen_text[]`, `event_track[]`, and `summary{}` is byte-for-byte the same as the `screen`/`import` sources. Browser specifics live **inside** the existing per-entry payloads (`event_track[].data{}`, `on_screen_text[].source="dom"` + `bbox=null`), never as new top-level fields. The `delta` kinds — `state_settle` in particular — are frozen and shared with veyo's pixel path.

Where browser mode differs from pixel modes:

- **No pixel codec.** `visual_timeline` entries are derived from **salient events**, not pixel deltas.
- **`event_track` is rich and lossless.** Every raw event is recorded pre-salience.
- **`on_screen_text` is DOM/a11y verbatim**: `source="dom"`, `bbox` absent/null, `selector`-style path present. Never `source="ocr"`.
- **`transcript` is usually empty.**

---

## 1. Browser-trace JSON format (`*.trace.json`)

A trace is a single JSON object: a small header plus a **flat, time-ordered `events[]` array**. Each event is a tagged union discriminated by `type`. The format is rrweb/CDP-compatible *in spirit* (one wall-clock per event; navigate/snapshot/mutation/console/network/input/a11y/screenshot surfaces) but is our own minimal shape — easy to emit from either an rrweb post-processor or a Playwright/CDP script.

### 1.1 Top-level envelope

```jsonc
{
  "clipxd_trace_version": "1",          // string, format version
  "session_id": "string",               // capture-assigned id (becomes Index.id basis)
  "captured_by": "string",              // e.g. "clipxd-capture-playwright/0.1"
  "started_at_ms": 1710854008000,       // int, ms since Unix epoch; t=0 anchor for the whole trace
  "viewport": { "w": 1280, "h": 800 },  // initial viewport (ints); updated by navigate/viewport events
  "url": "https://shop.example.com/cart",// initial top-level URL (string, may be "" if blank tab)
  "events": [ /* TraceEvent[] — see 1.3 */ ]
}
```

### 1.2 Common event fields (every event has these)

```jsonc
{
  "type": "string",   // discriminator: one of the kinds in 1.3
  "t_ms": 1710854008431  // int, ms since Unix epoch (the master clock). rel = t_ms - started_at_ms.
}
```

`t_ms` is **always normalized to wall-clock ms-since-epoch by the capture script** before it lands in the trace. The ingestor trusts `t_ms` and never re-derives time from monotonic/performance clocks (see §5.4). Events SHOULD be emitted in non-decreasing `t_ms` order; the ingestor stable-sorts defensively.

### 1.3 Event variants

Each variant is `{type, t_ms, ...}`. Unknown `type` values MUST be ignored (forward-compat), not error.

#### `navigate` — top-level URL / route change

```jsonc
{
  "type": "navigate",
  "t_ms": 1710854008100,
  "url": "https://shop.example.com/checkout",  // destination URL (required)
  "from": "https://shop.example.com/cart",      // source URL (string|null)
  "nav_kind": "load",   // "load" | "reload" | "back_forward" | "spa" | "hashchange"
  "title": "Checkout"   // document.title after settle (string|null; may be filled later)
}
```

#### `dom_snapshot` — full DOM baseline (rrweb FullSnapshot analogue)

```jsonc
{
  "type": "dom_snapshot",
  "t_ms": 1710854008120,
  "url": "https://shop.example.com/checkout",
  "node_count": 842,           // int, size hint for magnitude baselines
  "text": "Checkout\nPayment\nPlace order\n…",  // string|null: concatenated visible text at snapshot
  "scroll": { "x": 0, "y": 0 } // initial scroll (ints), optional
}
```

The capture script flattens the snapshot's visible text into `text`. The ingestor does **not** need to rebuild a virtual DOM; `dom_snapshot.text` seeds initial `on_screen_text` and resets node-id context for any selectors that follow.

#### `dom_mutation` — a batched DOM change

```jsonc
{
  "type": "dom_mutation",
  "t_ms": 1710854009950,
  "target": "#notifications",   // CSS-selector path of the mutation root (string)
  "op": "insert",               // "insert" | "remove" | "replace" | "text" | "attr"
  "added": 1,                   // int: added node count
  "removed": 0,                 // int: removed node count
  "text_delta": 18,             // int: net change in subtree text length (may be 0/negative)
  "role": "alert",              // string|null: a11y role of the most salient added/changed node
  "name": "Payment failed (500)", // string|null: accessible name / textContent (<=200 chars) of that node
  "attr": null                  // string|null: attribute name for op=="attr" (e.g. "aria-busy")
}
```

`role`/`name` are the **only** fields needed to drive salience and `on_screen_text`; the capture script is responsible for picking the most salient added/changed node and providing its verbatim text. Cosmetic attribute-only mutations (`op=="attr"` with `attr` in `{class, style}`) are low-salience by construction.

#### `console` — a console message or uncaught exception

```jsonc
{
  "type": "console",
  "t_ms": 1710854009920,
  "level": "error",   // "log" | "info" | "debug" | "warn" | "error" | "assert" | "trace"
  "text": "Checkout failed: HTTP 500 at /api/checkout",  // verbatim message (string)
  "stack": ["at submitOrder (checkout.js:84:13)", "at onClick (checkout.js:51:5)"], // string[]|null
  "source": "javascript", // "javascript" | "network" | "security" | "deprecation" | "other"
  "uncaught": false       // bool: true if this is an uncaught exception (not a console.* call)
}
```

#### `network` — one completed request/response (already correlated)

The capture script correlates request→response and emits **one** `network` event per finished request (one row per redirect hop is acceptable but discouraged for the same logical request).

```jsonc
{
  "type": "network",
  "t_ms": 1710854009900,         // completion time (response end), ms since epoch
  "method": "POST",              // string
  "url": "https://shop.example.com/api/checkout",  // full URL (string)
  "status": 500,                 // int|null (null/0 for failed/opaque; resource-timing entries may be null)
  "status_text": "Internal Server Error", // string|null
  "resource_type": "fetch",      // "document"|"xhr"|"fetch"|"script"|"image"|"css"|"font"|"media"|"websocket"|"other"
  "mime": "application/json",    // string|null
  "duration_ms": 1840,           // number|null: response_end - request_start
  "request_id": "req-7f3a",      // string|null: capture-local correlation id
  "error_text": null,            // string|null: e.g. "net::ERR_CONNECTION_REFUSED"
  "initiator": "script"          // "parser"|"script"|"preload"|"other"|null
}
```

`is_error` is derived by the ingestor: `status != null && status >= 400`, OR `status == 0`, OR `error_text != null`.

#### `click` — pointer interaction

```jsonc
{
  "type": "click",
  "t_ms": 1710854009800,
  "click_kind": "click",  // "click" | "dblclick" | "contextmenu" | "mousedown" | "mouseup" | "touch"
  "target": "button#place-order",  // CSS-selector path (string)
  "label": "Place order",          // accessible name / visible text of target (string|null)
  "x": 642, "y": 511               // int|null pointer coords
}
```

#### `key` / `input` — value-change input (NOT raw keystrokes)

Mirrors rrweb input semantics: the **resulting value**, not keydowns.

```jsonc
{
  "type": "input",
  "t_ms": 1710854009600,
  "target": "input#card-number",
  "label": "Card number",      // accessible label (string|null)
  "value": "****",             // string: post-mask value; "****" if masked/sensitive
  "checked": null,             // bool|null for checkboxes/radios
  "masked": true,              // bool: true if value was redacted at capture
  "submit": false              // bool: true if this input is an Enter/submit on a form
}
```

A bare keystroke stream is intentionally not modeled. Enter-on-form is represented either as `input.submit=true` or a `click` on the submit button — both feed the gesture→effect join (§3.5).

#### `scroll` — scroll of document or element

```jsonc
{
  "type": "scroll",
  "t_ms": 1710854009000,
  "target": "window",   // CSS-selector path or "window" (string)
  "x": 0, "y": 1200     // ints
}
```

#### `a11y_text` — verbatim on-screen text from the accessibility tree / DOM

The primary `on_screen_text` source. The capture script flattens an accessibility/aria snapshot (or a targeted node) into one or more entries.

```jsonc
{
  "type": "a11y_text",
  "t_ms": 1710854009960,
  "selector": "#notifications > .toast",  // CSS/a11y path (string)
  "role": "alert",                        // string|null
  "text": "Payment failed (500)",         // VERBATIM accessible name / textContent (string, required)
  "valid_until_ms": null,                 // int|null: when this text stops being visible (else open-ended)
  "sensitive": false                      // bool: true if from a password/payment field (redaction hook)
}
```

#### `screenshot` — a real-pixels keyframe reference (sparse, post-redaction)

```jsonc
{
  "type": "screenshot",
  "t_ms": 1710854009965,
  "path": "frames/000003.png", // string: path relative to the trace file's dir
  "reason": "state_settle",    // "navigation" | "state_settle" | "error" | "manual"
  "viewport": { "w": 1280, "h": 800 }, // ints
  "redacted": true             // bool: capture asserts DOM-level redaction ran before this shot
}
```

`screenshot.path` is **referenced**, never inlined. The ingestor copies the path into `visual_timeline[].frame_ref` when a salient moment is co-located in time (§3.7); it does not read pixels.

---

## 2. Mapping: trace → clipxd `Index`

The ingestor is a single deterministic pass plus a salience pass. This section is the lossless mapping (event_track + on_screen_text); §3 covers the salience-gated `visual_timeline`/`summary` derivation.

`Index` fields are filled as:

- `clipxd_version`: from the `clipxd-index` crate constant.
- `id`: derived from `session_id` (stable hash).
- `source`: `"browser"` (constant).
- `status`: `"complete"` after a successful ingest.
- `metadata`: `{ url_context, started_at_ms, viewport, captured_by, n_events }` (browser specifics inside `metadata`, not new top-level keys).
- `transcript`: `[]` (empty in browser mode).
- `redaction`: `{ items: [...] }`, accumulated from `a11y_text.sensitive` / `input.masked` (see §2.4).

### 2.1 `event_track[]` — lossless, pre-salience

**Every** trace event maps to exactly one `event_track` entry (except `dom_snapshot` and `screenshot`, which are structural, and high-frequency `scroll` which MAY be down-sampled). Each entry is `{ t, kind, text?, data{} }` where `t` is **seconds from clip start** = `(event.t_ms - started_at_ms) / 1000.0`.

| Trace event | `event_track.kind` | `data{}` (browser specifics live here) | `text?` |
|---|---|---|---|
| `navigate` | `navigation` | `{from, to:url, nav_kind, title}` | `url` |
| `dom_mutation` | `dom_mutation` | `{target, op, added, removed, text_delta, role, attr}` | `name` |
| `console` (level error/assert or `uncaught`) | `console_error` | `{level, source, stack, uncaught}` | `text` |
| `console` (level warn) | `console_warn` | `{level, source, stack}` | `text` |
| `console` (other levels) | `console_log` | `{level, source}` | `text` |
| `network` | `network` | `{method, url, status, status_text, resource_type, duration_ms, is_error, error_text, request_id, initiator}` | `"{method} {path(url)} {status}"` |
| `click` | `click` (or `context_menu` if `click_kind=="contextmenu"`) | `{click_kind, target, x, y}` | `label` |
| `input` (`submit==true`) | `form_submit` | `{target, value, masked}` | `label` |
| `input` / `key` (otherwise) | `input` | `{target, value, checked, masked}` | `label` |
| `scroll` | `scroll` | `{target, x, y}` | `null` |
| `a11y_text` | *(no event_track row — feeds on_screen_text only)* | — | — |
| `screenshot` | *(no event_track row — structural)* | — | — |

Rules:
- `event_track` is **lossless**: an event is recorded here regardless of salience. Never drop a low-salience event from `event_track` — "what was the user doing right before the error" depends on it.
- `mousemove`/`touchmove`-style high-frequency streams are not in this trace format at all (the capture script samples them out), so there is no per-move flooding to guard against here.

### 2.2 `on_screen_text[]` — DOM verbatim

`source` is **always `"dom"`**; `bbox` is **always absent/null**; `selector` is present. Each entry: `{ start, end, text, source:"dom", selector, bbox:null }` where `start`/`end` are seconds from clip start.

| Trace event | → `on_screen_text` entry |
|---|---|
| `dom_snapshot.text` | one entry per non-empty line (or one whole-snapshot entry), `selector:"<document>"`, `start = t`, `end` = next snapshot/navigation or end-of-trace. |
| `a11y_text` | `{start:t, end: valid_until_ms?→s else open, text, selector, source:"dom"}`. **Verbatim**, never paraphrased. |
| `dom_mutation` (with `name`) | `{start:t, end: open, text:name, selector:target, source:"dom"}` — newly inserted/changed visible text. |
| `console` (any) | `{start:t, end:t+ε, text, selector:"<console>", source:"dom"}` — so `search_text` finds the message. |
| `network` (is_error) | `{start:t, end:t+ε, text:"{method} {path} {status} {status_text}", selector:"<network-trace>", source:"dom"}` — so `search_text` finds the 500. |
| `input` (`masked==false`) | `{start:t, end: open, text:value, selector:target, source:"dom"}`. Masked inputs are NOT copied as text (only a redaction marker). |

`ε` is a small constant window (e.g. 2000 ms) for instantaneous messages.

### 2.3 Concrete worked example

A `network` event `{method:"POST", url:".../api/checkout", status:500, status_text:"Internal Server Error", duration_ms:1840}` produces:

- `event_track`: `{ t, kind:"network", text:"POST /api/checkout 500", data:{method:"POST", url:".../api/checkout", status:500, is_error:true, duration_ms:1840, ...} }`
- `on_screen_text`: `{ start:t, end:t+2, text:"POST /api/checkout 500 Internal Server Error", source:"dom", selector:"<network-trace>", bbox:null }`
- `visual_timeline` (because salient, §3.2): `{ t, salience:~0.9, caption:"POST /api/checkout → 500 (1840ms)", delta:"network_error", frame_ref:null }`

### 2.4 `redaction.items[]`

For each `a11y_text.sensitive==true` or `input.masked==true`, append `{ t, selector, reason:"sensitive_field" }`. The contract: the capture script has already stripped values at the DOM level **before** screenshots and before text landed in the trace (§5.5); the ingestor only records the marker and refuses to copy masked values into `on_screen_text`.

---

## 3. Browser-salience rules

Salience reuses veyo's exact gate shape and the four tunable knobs, recast for events. No pixels are scored.

```
salience = clamp(w_focus * magnitude * novelty, 0.0, 1.0)
emit_to_visual_timeline  iff  salience >= salience_min     // inclusive, mirrors veyo should_emit
```

Knobs (defaults): `salience_min = 0.4`, `settle_window_ms = 400`, `novelty_decay = 0.9` (per window), plus browser-specific `causal_window_ms = 1500` (gesture→effect join) and `size_norm = 12` (mutation magnitude normalizer).

### 3.1 Factors

- **`w_focus`** — event-kind weight:
  | kind | w_focus |
  |---|---|
  | console_error | 1.5 |
  | navigation | 1.6 |
  | network | 1.4 |
  | gesture_request (join) | 1.5 |
  | dom_mutation | 1.0 (1.4 if the changed subtree contains the last clicked/focused element) |
  | focus_change | 1.2 |

- **`magnitude`** — intrinsic severity/size (per rule below).

- **`novelty`** = `1 − habituation`, from a rolling baseline keyed by a **normalized signature** per kind:
  - network: `(method, url_template, status_class)` — URL normalized so `/api/orders/123` → `/api/orders/:id`.
  - console: `(level, message_signature)` — numbers/uuids/timestamps stripped.
  - dom_mutation: `(target_selector, op)`.
  - navigation: `(url_template)`.
  
  The first occurrence of a signature → novelty ≈ 1.0. Each repeat decays novelty by `novelty_decay`. A **pattern-break** (a signature that was habituated returning a new status class — e.g. a URL that returned 200s now 500s) **resets novelty toward 1.0**. URL/signature normalization is load-bearing: without it nothing ever habituates and the timeline floods on SPA/polling pages.

### 3.2 Rule: `network_response_error` (status ≥ 400 or failure)

- Trigger: `network` event with `is_error`.
- `magnitude`: 5xx → 1.0; 401/403 → 0.85; 4xx → 0.8 (404 on a `document` request → 0.9); `error_text` set → 0.9.
- `delta`: `network_error`. Caption: `"{method} {path(url)} → {status} ({status_text})"`, append `" ({duration_ms}ms)"` if present; failure → `"{method} {path(url)} failed: {error_text}"`. Drop any parenthetical whose field is missing (never emit `"… returned undefined"`).

### 3.3 Rule: `console_error` / `console_warn`

- Trigger: `console` with `uncaught==true` or `level ∈ {error, assert, warn}`.
- `magnitude`: uncaught 1.0, `error` 0.9, `assert` 0.9, `warn` 0.5 (usually below `salience_min` unless novel).
- `delta`: `console_error`. Caption: `"Console error: {text[:80]}"`; uncaught → `"Uncaught: {text[:80]} (at {stack[0]})"`.

### 3.4 Rule: `navigation`

- Trigger: `navigate` on the top-level URL.
- `magnitude`: 0.9 (full `load` → 1.0). Navigations rarely habituate; exact-URL revisit within a short window decays slightly to avoid loop-spam.
- `delta`: `navigation`. Caption: `"Navigated to {path(url)}"` / `"Reloaded {path}"` / `"Back to {path}"`; append `" — {title}"` once available. Navigations are **de-facto always salient** and are the **primary chapter boundary**.

### 3.5 Rule: `large DOM mutation`

- Trigger: `dom_mutation`.
- `magnitude` = `clamp((added + removed + text_delta/200) / size_norm, 0, 1)`, **boosted to ≥ 0.9** when `role ∈ {alert, alertdialog, dialog, status}` (toast/modal), or when `op=="replace"` on a route container, or on a form settle (`attr=="aria-busy"`/`"aria-invalid"` toggling false). **Cosmetic** `op=="attr"` with `attr ∈ {class, style}` → magnitude ≈ 0.05 (dropped). Magnitude is **semantic, not raw record count**.
- `delta`: `node_inserted` (insert of an alert/status), `modal_opened` (dialog), or `dom_subtree_replaced` (route replace). Caption: `"{role} inserted into {target}: \"{name[:60]}\""` / `"Dialog \"{name}\" opened"` / `"Replaced {target} subtree (new view)"`.

### 3.6 Rule: `form_submit` / gesture→request join

- Trigger: a `click` (or `input.submit==true`) **followed within `causal_window_ms`** by an outbound `network` request or a `navigate`, on the same target lineage.
- `magnitude`: gesture alone 0.3 (below threshold — a bare click is not salient); gesture + request 0.7; gesture + request-that-errored → **inherits the error magnitude (up to 1.0)**.
- `w_focus` = 1.5 (user-initiated, focused surface).
- `delta`: `gesture_request`. Caption: `"Clicked \"{label}\" → {METHOD} {path} ({status})"` e.g. `"Clicked \"Place order\" → POST /api/checkout (500)"`.
- This is **the** single-line answer to "what was the user doing right before the error". The join is heuristic (time + lineage); raw `click` and `network` rows stay separate in `event_track`. The joined caption is a derived convenience, like `summary` — not ground truth.

### 3.7 Rule: `state_settle` (browser FSM workhorse)

- Driven by the veyo STATIC/CHANGING/SETTLING FSM, fed by `dom_mutation` bursts + in-flight `network` instead of pixel diffs. A subtree is CHANGING while mutations/requests arrive; when it holds quiet for `settle_window_ms` with no mutations to that subtree and no pending requests it spawned, emit a settle.
- `magnitude`: from the size of the change that just settled (post-navigation full render ≈ 0.9; small widget ≈ 0.4). A periodically-settling region (poll→render→settle loop) habituates; a settle after a navigation stays salient.
- `delta`: `state_settle` (**frozen, shared with veyo's pixel path**). Caption: `"Page settled after {trigger} ({n} changes, {duration_ms}ms)"`.
- When a `screenshot` event with `reason ∈ {state_settle, navigation, error}` exists within ±`settle_window_ms`, set the settle's `visual_timeline.frame_ref = screenshot.path` (the one real-pixels, post-redaction capture for that moment). Otherwise `frame_ref = null`.

### 3.8 Coalescing (avoid triple-emit)

A checkout 500 caused by a "Place order" click can fire `network_error` **and** `gesture_request` **and** `dom_mutation` (error toast). These MUST coalesce, reusing veyo's adjacent/temporal coalescing, into a small number of `visual_timeline` lines at that `t` — ideally:
1. one `gesture_request` line naming cause+effect (`Clicked "Place order" → POST /api/checkout (500)`), and
2. one `node_inserted` line for the toast (`alert inserted into #notifications: "Payment failed (500)"`).

The standalone `network_error` line is suppressed when a `gesture_request` line already names the same request (same `request_id`/URL+status within `causal_window_ms`).

### 3.9 `summary` (second pass over salient timeline)

- `summary.chapters`: navigations are the **primary boundaries**; within each inter-navigation span, the single **highest-salience** error/settle becomes the chapter title. Chapters are derived from where salience clustered — never from pixels.
- `summary.tldr`: deterministic one-liner from the top-salience event(s), e.g. `"Checkout submission failed: POST /api/checkout returned 500; error toast 'Payment failed (500)' shown."`

### 3.10 Caption synthesis

All captions are **deterministic template-from-typed-fields** for the common cases (no LLM), which keeps browser mode cheap and exact. Templates **degrade gracefully** when a field is absent (drop the parenthetical rather than print `undefined`). A model is reserved only for genuinely ambiguous large mutations — out of scope for v1.

---

## 4. Crate plan: `clipxd-browser`

A new crate that depends on `clipxd-index` (for the `Index` schema + query types) and reuses the salience knobs from veyo's policy engine.

```
clipxd-browser/
  Cargo.toml                 # deps: clipxd-index, serde, serde_json, thiserror
  src/
    lib.rs                   # pub: ingest_trace(path|reader) -> Result<Index, IngestError>
    trace.rs                 # serde models for the trace format (§1) — #[serde(tag="type")] enum TraceEvent
    ingest.rs                # trace -> event_track + on_screen_text + metadata (§2); lossless pass
    salience.rs              # browser-salience model (§3): factors, rules, novelty baselines, coalescing
    caption.rs               # deterministic caption/tldr templates (§3.10)
    chapters.rs              # summary.chapters second pass (§3.9)
    url_norm.rs              # URL/message-signature normalization for novelty keys (§3.1)
    error.rs                 # IngestError (parse, schema, io) via thiserror
  tests/
    fixtures/
      checkout_500.trace.json      # §6 fixture
    ingest_test.rs                 # ingest fixture -> assert event_track/on_screen_text/visual_timeline
    salience_test.rs               # habituation (spinner doesn't flood) + pattern-break still fires
    schema_identity_test.rs        # Index serializes byte-identical-shape to a Phase-1 golden
    query_test.rs                  # uses clipxd-index query_clip/search_text on the ingested Index
```

### 4.1 Public API

```rust
// lib.rs
pub fn ingest_trace_from_path(p: &Path, opts: &IngestOptions) -> Result<Index, IngestError>;
pub fn ingest_trace_from_reader<R: Read>(r: R, opts: &IngestOptions) -> Result<Index, IngestError>;

pub struct IngestOptions {
    pub salience_min: f32,      // default 0.4
    pub settle_window_ms: u64,  // default 400
    pub novelty_decay: f32,     // default 0.9
    pub causal_window_ms: u64,  // default 1500
    pub size_norm: f32,         // default 12.0
    pub frames_dir: Option<PathBuf>, // resolve screenshot paths relative to this
}
```

`ingest.rs` and `salience.rs` are pure functions over deserialized events (no IO inside the salience model) so they unit-test without fixtures on disk.

### 4.2 CLI subcommand

Add to the existing `clipxd` binary:

```
clipxd ingest-browser <trace.json> [--out index.json] [--frames-dir DIR]
                       [--salience-min 0.4] [--settle-window-ms 400]
```

Behavior: parse → `ingest_trace_from_path` → write the resulting `Index` as `index.json` (the same writer Phase-1's `clipxd-import` uses). Then `clipxd-mcp` serves it unchanged (schema-identity).

### 4.3 Tests (mandatory)

1. **ingest_test**: ingest `checkout_500.trace.json`; assert the `event_track` contains the `click` (`Place order`), the `network` 500, and the `console_error`; assert `on_screen_text` contains the verbatim `"Payment failed (500)"` with `source:"dom"` and `bbox:null`.
2. **salience_test** (mirrors veyo's pixel habituation eval):
   - A spinner mutating `#app` 60×/sec MUST NOT flood `visual_timeline` (habituation drives it to silence).
   - A **pattern-break** (a habituated `GET /poll` 200 suddenly returning 500) MUST still emit a salient moment.
3. **schema_identity_test**: serialize the ingested `Index` and assert its **shape** (key set per array element, `delta` enum membership, `source:"dom"`/`bbox:null` on `on_screen_text`) matches a Phase-1 golden `Index` produced by `clipxd-import`. No browser-only top-level keys. `delta` values are a subset of the frozen veyo set, with `state_settle` present.
4. **query_test**: run `clipxd-index` `query_clip("what error showed up and what was the user doing right before it")` and `search_text("500")` on the ingested `Index`; assert the answer surfaces the `gesture_request` moment + the 500 (§6 expectations).

---

## 5. Capture approach (optional Node Playwright/CDP script)

The capture script is a **convenience emitter**, not the tested core. The ingestor + fixtures are the contract; any tool that emits a valid trace (rrweb post-processor, hand-written CDP, Playwright) is acceptable.

### 5.1 Shape

`clipxd-capture/capture.mjs` — a small Node script using `playwright`:

```
launch chromium → newContext({ recordHar? }) → newPage()
attach handlers that push normalized events into events[]:
  page.on('console',  m => push console{level=m.type(), text=m.text(), stack=loc, source})
  page.on('pageerror', e => push console{level:'error', uncaught:true, text:e.message, stack})
  page.on('response',  r => push network{method, url, status, status_text, mime, resource_type, duration_ms, request_id})
  page.on('requestfailed', r => push network{..., status:null, error_text:r.failure().errorText})
  page.on('framenavigated', f => if mainFrame push navigate{url:f.url(), nav_kind, title})
  page.on('load', () => maybe push dom_snapshot + screenshot{reason:'navigation'})
periodically / on settle:
  await page.accessibilitySnapshot()  // or page.ariaSnapshot() YAML → flatten to a11y_text[]
  await page.screenshot({path})        // → screenshot{reason:'state_settle'|'manual', redacted:true}
on close: write { ...header, events } to <out>.trace.json
```

DOM mutations + clicks/inputs/scrolls are gathered by injecting a tiny in-page recorder (`page.exposeBinding` + a `MutationObserver` / event listeners) that posts normalized `dom_mutation`/`click`/`input`/`scroll`/`a11y_text` records back to the script. (Equivalently, run rrweb in-page and post-process its events into our trace — clean-room mapping documented in the research findings.)

### 5.2 Normalization responsibilities (capture-side, not ingestor-side)

- **One timebase**: convert every surface to wall-clock ms-since-epoch for `t_ms`. CDP `MonotonicTime` (seconds since boot) and Playwright `timing().startTime` (ms-since-epoch with relative sub-fields) MUST be reconciled to epoch ms before emit.
- **Network correlation**: pair request→response into one `network` event; account for redirect chains; set `status:null` for resource-timing entries that expose no HTTP code.
- **Console args**: index `text()` only; do not block on async `jsonValue()` resolution.
- **a11y**: prefer `ariaSnapshot`/`accessibilitySnapshot`; flatten role+name into `a11y_text` entries; this is a heavy one-shot snapshot, taken at settle points, not a stream.

### 5.3 Screenshot policy

Sparse: capture on `load`/`framenavigated`, on `state_settle`, and on error moments — not per frame. PNG to `frames/NNNNNN.png`; emit a `screenshot` event referencing the relative path.

### 5.4 Why the ingestor trusts `t_ms`

All timebase reconciliation is done **once**, capture-side. The ingestor is pure and deterministic over `t_ms` and never touches monotonic/performance clocks — this keeps the tested core simple and the fixtures stable.

### 5.5 Redaction ordering (load-bearing)

CloakPipe runs **at the DOM level before any screenshot and before text is written to the trace**. A focus/input on a password/payment field MUST (a) mark `a11y_text.sensitive=true` / `input.masked=true`, (b) strip the value, and (c) ensure the subsequent `screenshot.redacted=true` reflects a redacted DOM. The ingestor records the `redaction.items` marker and refuses to copy masked values into `on_screen_text` — a secret must never reach the index.

---

## 6. Fixture trace — checkout-500 scenario

Scenario: user is on `/checkout`, clicks **Place order**, which fires `POST /api/checkout` returning **500**; a `console.error` is logged; an error toast (`role=alert`, text `"Payment failed (500)"`) is inserted into `#notifications`; the page settles. After ingest, `query_clip("what error showed up and what was the user doing right before it")` must answer with the `gesture_request` moment (`Clicked "Place order" → POST /api/checkout (500)`) plus the `console_error`/toast, and `search_text("500")` must find the network and toast text.

```json
{
  "clipxd_trace_version": "1",
  "session_id": "fixture-checkout-500",
  "captured_by": "clipxd-capture-playwright/0.1",
  "started_at_ms": 1710854008000,
  "viewport": { "w": 1280, "h": 800 },
  "url": "https://shop.example.com/cart",
  "events": [
    {
      "type": "navigate",
      "t_ms": 1710854008100,
      "url": "https://shop.example.com/checkout",
      "from": "https://shop.example.com/cart",
      "nav_kind": "load",
      "title": "Checkout"
    },
    {
      "type": "dom_snapshot",
      "t_ms": 1710854008120,
      "url": "https://shop.example.com/checkout",
      "node_count": 842,
      "text": "Checkout\nOrder summary\nPayment\nCard number\nPlace order",
      "scroll": { "x": 0, "y": 0 }
    },
    {
      "type": "a11y_text",
      "t_ms": 1710854008140,
      "selector": "main#checkout h1",
      "role": "heading",
      "text": "Checkout",
      "valid_until_ms": null,
      "sensitive": false
    },
    {
      "type": "a11y_text",
      "t_ms": 1710854008150,
      "selector": "button#place-order",
      "role": "button",
      "text": "Place order",
      "valid_until_ms": null,
      "sensitive": false
    },
    {
      "type": "screenshot",
      "t_ms": 1710854008600,
      "path": "frames/000001.png",
      "reason": "navigation",
      "viewport": { "w": 1280, "h": 800 },
      "redacted": true
    },
    {
      "type": "input",
      "t_ms": 1710854009200,
      "target": "input#card-number",
      "label": "Card number",
      "value": "****",
      "checked": null,
      "masked": true,
      "submit": false
    },
    {
      "type": "click",
      "t_ms": 1710854009800,
      "click_kind": "click",
      "target": "button#place-order",
      "label": "Place order",
      "x": 642,
      "y": 511
    },
    {
      "type": "network",
      "t_ms": 1710854009900,
      "method": "POST",
      "url": "https://shop.example.com/api/checkout",
      "status": 500,
      "status_text": "Internal Server Error",
      "resource_type": "fetch",
      "mime": "application/json",
      "duration_ms": 1840,
      "request_id": "req-7f3a",
      "error_text": null,
      "initiator": "script"
    },
    {
      "type": "console",
      "t_ms": 1710854009920,
      "level": "error",
      "text": "Checkout failed: HTTP 500 at /api/checkout",
      "stack": ["at submitOrder (checkout.js:84:13)", "at onClick (checkout.js:51:5)"],
      "source": "javascript",
      "uncaught": false
    },
    {
      "type": "dom_mutation",
      "t_ms": 1710854009950,
      "target": "#notifications",
      "op": "insert",
      "added": 1,
      "removed": 0,
      "text_delta": 18,
      "role": "alert",
      "name": "Payment failed (500)",
      "attr": null
    },
    {
      "type": "a11y_text",
      "t_ms": 1710854009960,
      "selector": "#notifications > .toast",
      "role": "alert",
      "text": "Payment failed (500)",
      "valid_until_ms": null,
      "sensitive": false
    },
    {
      "type": "screenshot",
      "t_ms": 1710854009965,
      "path": "frames/000002.png",
      "reason": "error",
      "viewport": { "w": 1280, "h": 800 },
      "redacted": true
    },
    {
      "type": "dom_mutation",
      "t_ms": 1710854010360,
      "target": "#checkout-form",
      "op": "attr",
      "added": 0,
      "removed": 0,
      "text_delta": 0,
      "role": null,
      "name": null,
      "attr": "aria-busy"
    },
    {
      "type": "screenshot",
      "t_ms": 1710854010400,
      "path": "frames/000003.png",
      "reason": "state_settle",
      "viewport": { "w": 1280, "h": 800 },
      "redacted": true
    }
  ]
}
```

### 6.1 Expected ingest result (assertions the test makes)

- **`event_track`** (lossless, in order) contains: `navigation` (→ /checkout), `input` (`Card number`, masked), `click` (`Place order`), `network` (POST /api/checkout, status 500, `is_error:true`), `console_error` (`Checkout failed: HTTP 500 …`), `dom_mutation` (insert into `#notifications`), `dom_mutation` (attr `aria-busy`).
- **`on_screen_text`** (all `source:"dom"`, `bbox:null`) contains verbatim: `"Checkout"`, `"Place order"`, `"Payment failed (500)"`, the console message, and the network status line `"POST /api/checkout 500 Internal Server Error"`. The masked `input#card-number` value is **absent**; a `redaction.items` marker for it is **present**.
- **`visual_timeline`** (after coalescing) contains, at `t ≈ 1.9s`:
  - `{ delta:"gesture_request", caption:"Clicked \"Place order\" → POST /api/checkout (500)", salience ≈ 1.0, frame_ref:"frames/000002.png" }`
  - `{ delta:"node_inserted", caption:"alert inserted into #notifications: \"Payment failed (500)\"", salience ≥ 0.9 }`
  - `{ delta:"console_error", caption:"Console error: Checkout failed: HTTP 500 at /api/checkout", salience ≥ 0.9 }` (or coalesced into the above per §3.8)
  - plus an earlier `{ delta:"navigation", caption:"Navigated to /checkout — Checkout" }` and a trailing `{ delta:"state_settle", caption:"Page settled after error (…)", frame_ref:"frames/000003.png" }`. The standalone `network_error` line is suppressed because `gesture_request` already names that request.
- **`summary.chapters`**: one chapter boundary at the `/checkout` navigation, titled by its highest-salience event → `"Checkout — payment failed (500)"`. **`summary.tldr`** ≈ `"On /checkout, clicking 'Place order' triggered POST /api/checkout → 500; error toast 'Payment failed (500)' was shown."`

### 6.2 Query expectations

- `search_text("500")` → returns the `network` on-screen-text entry and the `"Payment failed (500)"` toast entry.
- `query_clip("what error showed up and what was the user doing right before it")` → resolves to the `gesture_request` `visual_timeline` moment (the click on "Place order" and its POST /api/checkout → 500), with the `console_error` and toast as corroborating `on_screen_text`, and `get_frame_context` at that `t` returning `frames/000002.png`.

---

## 7. Implementation notes — as built (deliberate deviations)

The crate `clipxd-browser` implements this spec, with these conscious choices (a post-build adversarial review confirmed the trade-offs):

- **Schema-identity (§0) wins over §2.2's `selector`.** `on_screen_text` keeps the *exact* Phase-1 shape `{start, end, text, source, bbox}` — no `selector` field, `source:"dom"`, `bbox:null`. Adding a browser-only field would break schema-identity; the selector is dropped. Likewise `metadata` stays the Phase-1 struct (the viewport lands in `resolution`); `started_at_ms`/`captured_by`/`n_events` are not added as new keys.
- **`delta` vocabulary is an open string.** Browser deltas (`gesture_request`, `node_inserted`, `network_error`, `navigation`, `state_settle`, …) are descriptive strings in the same `delta` *field* Phase 1 uses — the JSON shape is identical; only the vocabulary is richer.
- **`state_settle` is screenshot-driven, not a full FSM (yet).** A `screenshot{reason:"state_settle"}` yields a settle moment; the full STATIC/CHANGING/SETTLING FSM over mutation+request bursts (§3.7) is deferred. Captions are the deterministic templates of §3.10.
- **Gesture→request join is a time+type heuristic.** It joins a click to a following *primary* request (`fetch`/`xhr`/`document`) within `causal_window_ms`; background assets (image/css/font/script) never join. It does **not** prove causation (no `request_id`↔click correlation in v1), so a sync request shortly after an unrelated click can still join — the raw `click` + `network` rows always remain separate in `event_track`. Rapid clicks: the latest within the window wins.
- **Redaction scope (privacy).** Capture-side DOM masking of form fields is honored now — masked `input`/sensitive `a11y_text` values **never** reach the index, only a `redaction.items` marker (tested). URL query strings are stripped from stored URLs (tokens hide in `?…`). Scanning arbitrary **console / network / DOM text** for secrets is CloakPipe's job and is **deferred to Phase 4** (`redaction.policy = "dom-mask-phase2; text-scan-deferred-phase4"`).
- **Defensive caps (no silent truncation).** `dom_snapshot` text is bounded (≤300 lines, ≤600 chars/line) and every stream is capped (`on_screen_text` 8k, `visual_timeline` 4k, `event_track` 100k, `redaction` 8k) with a `tracing::warn` when truncated — a hostile/pathological trace can't OOM the ingestor.
- **`network_is_error`** = `status≥400` OR `status==0` OR `error_text` present. A *missing* status is **not** an error by itself.
