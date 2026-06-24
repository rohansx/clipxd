//! `clipxd-web <clips-dir> [--port 8787]` — serve a directory of clips over HTTP.
//!
//! Each `clips-dir/<id>/index.json` becomes a clip at `/clip/<id>` (share page) with the
//! agent-readable `/clip/<id>/index.json` sidecar + `query`/`search`/`events` endpoints.

use anyhow::Context;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "clipxd-web", about = "Serve clipxd clips over HTTP (share page + agent index).")]
struct Args {
    /// Directory containing clip folders (each with an index.json).
    clips_dir: PathBuf,
    #[arg(long, default_value_t = 8787)]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("clipxd=info,tower_http=info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_target(false).init();

    let args = Args::parse();
    anyhow::ensure!(args.clips_dir.is_dir(), "clips dir not found: {}", args.clips_dir.display());

    let app = clipxd_web::app(args.clips_dir.clone());
    let addr = format!("0.0.0.0:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!("clipxd-web serving {} on http://{addr}", args.clips_dir.display());
    axum::serve(listener, app).await?;
    Ok(())
}
