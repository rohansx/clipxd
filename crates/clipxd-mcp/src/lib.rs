//! `clipxd-mcp` — the MCP server over a clip [`Index`](clipxd_index::Index).
//!
//! This is the **agent surface**: any MCP-speaking agent (Claude included) reasons over a
//! clip by calling these tools, never downloading the video. Each tool is a thin wrapper
//! over the [`query`](clipxd_index::query) primitives — the index is the single source of
//! truth ([mcp-api.md](../../docs/mcp-api.md)).
//!
//! One server serves one clip (loaded from its `index.json`); library-wide scope is a
//! later phase. The transport is stdio, matching the rest of the stack.

use clipxd_index::{query, Index};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    tool, tool_handler, tool_router,
    transport::stdio,
    ServerHandler, ServiceExt,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct QueryParams {
    /// The natural-language question about the clip.
    question: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchParams {
    /// Words/phrase to find across transcript + on-screen text + captions.
    query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct FrameParams {
    /// Time in seconds from clip start.
    t: f64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EventsParams {
    /// Start of the window in seconds (default 0).
    start: Option<f64>,
    /// End of the window in seconds (default: end of clip).
    end: Option<f64>,
}

/// The MCP handler, holding one clip's index.
#[derive(Clone)]
pub struct ClipxdHandler {
    index: Arc<Index>,
    #[allow(dead_code)]
    tool_router: ToolRouter<ClipxdHandler>,
}

#[tool_router]
impl ClipxdHandler {
    pub fn new(index: Index) -> Self {
        Self {
            index: Arc::new(index),
            tool_router: Self::tool_router(),
        }
    }

    fn json<T: serde::Serialize>(v: &T) -> String {
        serde_json::to_string_pretty(v).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
    }

    /// Answer a question grounded in the index, with timestamped citations.
    #[tool(
        description = "Answer a natural-language question about the clip, grounded in its index with timestamped citations. The agent never needs the video."
    )]
    fn query_clip(&self, Parameters(QueryParams { question }): Parameters<QueryParams>) -> String {
        Self::json(&query::query_clip(&self.index, &question))
    }

    /// Rank text hits across transcript + on-screen text + captions.
    #[tool(
        description = "Full-text search across the clip's transcript, on-screen text, and salient-moment captions. Returns ranked, timestamped hits."
    )]
    fn search_text(&self, Parameters(SearchParams { query }): Parameters<SearchParams>) -> String {
        Self::json(&query::search_text(&self.index, &query))
    }

    /// Everything the index knows at one instant.
    #[tool(
        description = "Return everything true at time t (seconds): the salient caption, on-screen text, transcript line, focused app, nearby events, and the redacted frame reference."
    )]
    fn get_frame_context(&self, Parameters(FrameParams { t }): Parameters<FrameParams>) -> String {
        Self::json(&query::get_frame_context(&self.index, t))
    }

    /// The event-track slice in a time window (clicks, console, network, DOM).
    #[tool(
        description = "Return the event-track entries within [start, end] seconds — clicks, keys, console, network, DOM mutations. Empty for imported clips (no input events exist)."
    )]
    fn get_events(&self, Parameters(EventsParams { start, end }): Parameters<EventsParams>) -> String {
        let lo = start.unwrap_or(0.0);
        let hi = end.unwrap_or(f64::INFINITY);
        let slice: Vec<_> = self
            .index
            .event_track
            .iter()
            .filter(|e| e.t >= lo && e.t <= hi)
            .collect();
        Self::json(&slice)
    }

    /// Orient: metadata, summary, and stream sizes. Call this first.
    #[tool(
        description = "A concise orientation: clip id/title/duration, the derived summary, and the size of each index stream. Call first."
    )]
    fn get_summary(&self) -> String {
        let m = &self.index.metadata;
        serde_json::json!({
            "id": self.index.id,
            "title": m.title,
            "duration_s": m.duration,
            "resolution": m.resolution,
            "source": self.index.source,
            "status": self.index.status,
            "summary": self.index.summary,
            "streams": {
                "transcript": self.index.transcript.len(),
                "on_screen_text": self.index.on_screen_text.len(),
                "visual_timeline": self.index.visual_timeline.len(),
                "event_track": self.index.event_track.len(),
            }
        })
        .to_string()
    }
}

#[tool_handler]
impl ServerHandler for ClipxdHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

/// Serve one clip over MCP (stdio transport).
pub struct ClipxdMcpServer {
    index: Index,
}

impl ClipxdMcpServer {
    pub fn new(index: Index) -> Self {
        Self { index }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let service = ClipxdHandler::new(self.index).serve(stdio()).await?;
        service.waiting().await?;
        Ok(())
    }
}
