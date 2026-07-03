//! Multi-tenant MCP server at `/mcp` — the "add clipxd.com as an MCP server, works for every
//! clip" surface (the Jam-MCP pattern, generalized). One Streamable HTTP endpoint; every tool
//! takes an explicit `clip_id` since this server spans the whole hosted library, not one clip.
//!
//! `clipxd-mcp` (the sibling crate) is unrelated and unchanged: it's a local, single-clip,
//! stdio-transport server someone runs by hand against a downloaded `index.json` — the right
//! shape for a local-first, one-clip-at-a-time workflow. This module is the hosted counterpart
//! for "paste a clipxd.com link into any MCP-speaking agent" — the tools are the same
//! (`query_clip`, `search_text`, `get_frame_context`, `get_events`, `get_summary`), just
//! parameterized by which clip.

use crate::{load_index, AppState};
use clipxd_index::query;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService},
    ServerHandler,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ClipIdParams {
    /// The clip id from its share URL, e.g. `clp_1efc6ad3` (from `clipxd.com/clip/clp_1efc6ad3`).
    clip_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct QueryParams {
    /// The clip id from its share URL, e.g. `clp_1efc6ad3`.
    clip_id: String,
    /// The natural-language question about the clip.
    question: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchParams {
    /// The clip id from its share URL, e.g. `clp_1efc6ad3`.
    clip_id: String,
    /// Words/phrase to find across transcript + on-screen text + captions.
    query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct FrameParams {
    /// The clip id from its share URL, e.g. `clp_1efc6ad3`.
    clip_id: String,
    /// Time in seconds from clip start.
    t: f64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EventsParams {
    /// The clip id from its share URL, e.g. `clp_1efc6ad3`.
    clip_id: String,
    /// Start of the window in seconds (default 0).
    start: Option<f64>,
    /// End of the window in seconds (default: end of clip).
    end: Option<f64>,
}

#[derive(Clone)]
pub struct McpHandler {
    state: AppState,
    #[allow(dead_code)]
    tool_router: ToolRouter<McpHandler>,
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

fn not_found(clip_id: &str) -> String {
    serde_json::json!({ "error": format!("no such clip: {clip_id}") }).to_string()
}

#[tool_router]
impl McpHandler {
    fn new(state: AppState) -> Self {
        Self { state, tool_router: Self::tool_router() }
    }

    #[tool(
        description = "Answer a natural-language question about a clip, grounded in its index with timestamped citations. The agent never needs the video. Call get_summary first if you don't already have the clip_id from its share URL."
    )]
    async fn query_clip(&self, Parameters(QueryParams { clip_id, question }): Parameters<QueryParams>) -> String {
        match load_index(&self.state, &clip_id).await {
            Ok(idx) => json(&query::query_clip(&idx, &question)),
            Err(_) => not_found(&clip_id),
        }
    }

    #[tool(
        description = "Full-text search across a clip's transcript, on-screen text, and salient-moment captions. Returns ranked, timestamped hits."
    )]
    async fn search_text(&self, Parameters(SearchParams { clip_id, query: q }): Parameters<SearchParams>) -> String {
        match load_index(&self.state, &clip_id).await {
            Ok(idx) => json(&query::search_text(&idx, &q)),
            Err(_) => not_found(&clip_id),
        }
    }

    #[tool(
        description = "Return everything true at time t (seconds) in a clip: the salient caption, on-screen text, transcript line, focused app, nearby events, and the redacted frame reference."
    )]
    async fn get_frame_context(&self, Parameters(FrameParams { clip_id, t }): Parameters<FrameParams>) -> String {
        match load_index(&self.state, &clip_id).await {
            Ok(idx) => json(&query::get_frame_context(&idx, t)),
            Err(_) => not_found(&clip_id),
        }
    }

    #[tool(
        description = "Return a clip's event-track entries within [start, end] seconds — clicks, keys, console, network, DOM mutations. Empty for imported clips (no input events exist)."
    )]
    async fn get_events(&self, Parameters(EventsParams { clip_id, start, end }): Parameters<EventsParams>) -> String {
        let Ok(idx) = load_index(&self.state, &clip_id).await else {
            return not_found(&clip_id);
        };
        let lo = start.unwrap_or(0.0);
        let hi = end.unwrap_or(f64::INFINITY);
        let slice: Vec<_> = idx.event_track.iter().filter(|e| e.t >= lo && e.t <= hi).collect();
        json(&slice)
    }

    #[tool(
        description = "Orient on one clip: id/title/duration/status, the derived summary (title/tldr/chapters), and the size of each index stream. Call this first when you have a clipxd share URL and need its clip_id + a quick overview before asking a specific question."
    )]
    async fn get_summary(&self, Parameters(ClipIdParams { clip_id }): Parameters<ClipIdParams>) -> String {
        let Ok(idx) = load_index(&self.state, &clip_id).await else {
            return not_found(&clip_id);
        };
        let m = &idx.metadata;
        json(&serde_json::json!({
            "id": idx.id,
            "title": m.title,
            "duration_s": m.duration,
            "resolution": m.resolution,
            "source": idx.source,
            "status": idx.status,
            "summary": idx.summary,
            "streams": {
                "transcript": idx.transcript.len(),
                "on_screen_text": idx.on_screen_text.len(),
                "visual_timeline": idx.visual_timeline.len(),
                "event_track": idx.event_track.len(),
            }
        }))
    }
}

#[tool_handler]
impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.instructions = Some(
            "clipxd: every recording is an agent-queryable index. Tools take an explicit \
             clip_id (the id from a clipxd share URL, e.g. clipxd.com/clip/clp_xxxxxxxx -> \
             clip_id \"clp_xxxxxxxx\"). Call get_summary first to orient, then query_clip / \
             search_text / get_frame_context / get_events as needed. The video is never \
             downloaded — every answer comes from the pre-built index (transcript, OCR, \
             scene captions, UI events)."
                .into(),
        );
        info
    }
}

/// Build the `/mcp` Tower service. Mount with `.route_service("/mcp", mcp_service(state))`.
///
/// `allowed_hosts` defaults (per the MCP Streamable HTTP spec's DNS-rebinding protection) to
/// loopback only — wrong for a public deployment, where every request's `Host` header is the
/// real domain, not `localhost`. Derived from `CLIPXD_PUBLIC_BASE` (already set in hosted mode
/// for share links) when present; falls back to the library default (loopback-only, correct
/// for local/LAN mode) otherwise.
pub fn mcp_service(state: AppState) -> StreamableHttpService<McpHandler, LocalSessionManager> {
    let mut config = StreamableHttpServerConfig::default();
    if let Some(base) = std::env::var("CLIPXD_PUBLIC_BASE").ok().filter(|s| !s.is_empty()) {
        if let Some(host) = base.split("://").nth(1) {
            let host = host.trim_end_matches('/').to_string();
            config.allowed_hosts.push(host);
        }
    }
    StreamableHttpService::new(move || Ok(McpHandler::new(state.clone())), Arc::new(LocalSessionManager::default()), config)
}
