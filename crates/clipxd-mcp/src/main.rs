//! `clipxd-mcp <clip-dir | index.json>` — serve one clip's index to MCP agents over stdio.
//!
//! Point an MCP client (e.g. Claude) at this binary with a clip path; the agent can then
//! call `query_clip`, `search_text`, `get_frame_context`, `get_events`, `get_summary`.

use anyhow::{Context, Result};
use clipxd_index::Index;
use clipxd_mcp::ClipxdMcpServer;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    // Logs MUST go to stderr — stdout carries the JSON-RPC protocol stream.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("clipxd=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .init();

    let clip = std::env::args()
        .nth(1)
        .context("usage: clipxd-mcp <clip-dir | index.json>")?;
    let p = PathBuf::from(&clip);
    let path = if p.is_dir() { p.join("index.json") } else { p };
    let txt = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let index: Index =
        serde_json::from_str(&txt).with_context(|| format!("parsing {}", path.display()))?;

    tracing::info!(
        "serving clip {} (\"{}\") over MCP/stdio",
        index.id,
        index.metadata.title
    );
    ClipxdMcpServer::new(index).run().await
}
