# clipxd — MCP server & JSON API

> How an agent reads a clip. The MCP server is the native surface for Claude and any MCP-speaking agent; the JSON API + per-clip sidecar serve everyone else. Both sit on top of the one [index schema](index-schema.md) — they are *views*, not new sources of truth.

The whole point: **the agent queries the URL, never the pixels.** If a query can only be answered by watching the video, the index ([index-schema.md](index-schema.md)) is incomplete, not the API.

---

## 1. MCP server

MCP (Model Context Protocol) is the open standard for exposing tools/resources to an agent. clipxd's `clipxd-mcp` crate ([architecture §3](architecture.md#3-crate--workspace-layout)) exposes a clip — or the whole library — as MCP **tools** plus MCP **resources**.

### 1.1 Tools

```
query_clip(url|id, q)            -> answer grounded in the index, with timestamped citations
get_frame_context(url|id, t)     -> everything true at time t: caption, on-screen text,
                                     transcript line, focused app, nearby events
search_text(url|id?, q)          -> hits across transcript + on_screen_text + captions
                                     (omit clip id to search the whole library — §3)
get_events(url|id, start, end)   -> the event_track slice in [start, end]
                                     (clicks, keys, console, network, dom)
get_transcript(url|id, range?)   -> transcript, optionally time-bounded
get_summary(url|id)              -> tldr + chapters (convenience; not ground truth)
```

Every tool returns **timestamps** so the agent can cite *when* (and a human can jump there in the player). `query_clip` is the high-level entry point — it composes the lower-level tools internally and returns a grounded answer; the others are for agents that want to drive the index themselves.

### 1.2 The headline interaction, concretely

> *"What error showed up and what was the user doing right before it?"*

```
query_clip(url, "what error showed up and what was the user doing right before it")
  → internally:
      search_text(url, "error|500|failed")      → on_screen_text hit "Payment failed (500)" @ t=13.0
      get_events(url, 11.0, 13.0)                → click button#place-order @12.4, POST /api/checkout 500 @13.1
      get_frame_context(url, 13.0)               → caption "red error toast…", transcript "…throws an error"
  → answer: "At 0:13 a 'Payment failed (500)' toast appeared. Right before it (0:12.4) the user clicked
             'Place order'; the POST /api/checkout returned 500 after 1.84s." [cites t=12.4, 13.0, 13.1]
```

The agent never fetched the video. That is the product.

### 1.3 Resources

Each clip is also exposed as MCP **resources** for agents that prefer to pull raw context:

```
clipxd://clip/{id}/index.json      -> the full index ([index-schema.md])
clipxd://clip/{id}/transcript      -> transcript stream
clipxd://clip/{id}/frames/{t}      -> a single salient (redacted) screenshot, if it exists
```

---

## 2. JSON API + sidecar (non-MCP consumers)

For tools, scripts, and CI that don't speak MCP, the same data is a plain HTTP API and a static sidecar.

```
GET  /clip/{id}                 -> the share page (human: video player)
GET  /clip/{id}/index.json      -> the full index (machine; the sidecar)
GET  /clip/{id}/search?q=…      -> search_text equivalent
GET  /clip/{id}/events?from=&to=-> get_events equivalent
GET  /clip/{id}/frame/{t}.jpg   -> a salient redacted screenshot
```

**The sidecar convention:** every clip URL has a sibling `…/index.json`. Paste a clip link anywhere; an agent (or a curl) can append `/index.json` (or content-negotiate `Accept: application/json`) and get the structured object behind the same URL. This is the "behind the same URL" promise from [overview §4.3](overview.md#43-agent-readable--processable-recordings-the-headline) made concrete.

---

## 3. One server per clip, or one for the library?

This is an **open question** ([risks-and-open-questions.md](risks-and-open-questions.md)). The current lean:

- **Per-clip is the unit of sharing** — a single clip link must be queryable on its own (the headline demo is one clip). So per-clip addressing (`query_clip(url)`) is always supported.
- **Library-wide is a mode of the same server** — `search_text` with the clip id omitted searches the whole local library (this is [features §4.7](features.md)). One running `clipxd-mcp` indexes everything in the local store; per-clip queries just scope to one id.

So: **one server, two scopes** — not one process per clip. A shared/ephemeral clip can still be served standalone by pointing the same server at a single `{ video, index.json }` bundle ([architecture §6](architecture.md#6-storage-model)).

---

## 4. Auth & trust

- **Local (default):** the MCP server binds localhost; the agent is on the same machine. No auth needed; nothing leaves the box.
- **Shared (tunnel):** an ephemeral token in the URL gates access for the lifetime of the tunnel.
- **Hosted (later):** standard bearer auth; the served index is already post-CloakPipe, so even a leaked link exposes only redacted content ([privacy-and-redaction.md](privacy-and-redaction.md)).

---

## 5. Versioning

The index carries `clipxd_version` ([index-schema.md §1](index-schema.md#1-top-level-shape)); the MCP/JSON surface is versioned alongside it. Because every tool is a *view* over the index and the schema crate is the single source of truth, adding a stream (e.g. a future `emotion` or `audio_events` track) is additive — existing tools keep working, new tools expose the new stream. The contract stays small on purpose ([index-schema §9](index-schema.md#9-invariants-what-consumers-can-rely-on)).
