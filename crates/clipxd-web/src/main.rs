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

    ensure_tessdata();

    let args = Args::parse();
    anyhow::ensure!(args.clips_dir.is_dir(), "clips dir not found: {}", args.clips_dir.display());

    let app = clipxd_web::app(args.clips_dir.clone());
    let addr = format!("0.0.0.0:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!("clipxd-web serving {} on http://{addr}", args.clips_dir.display());
    axum::serve(listener, app).await?;
    Ok(())
}

/// Make OCR work out of the box: if `TESSDATA_PREFIX` isn't set, find a tessdata dir with
/// `eng.traineddata` in the usual places and point at it (so ingested recordings get
/// on-screen text without the operator configuring anything).
fn ensure_tessdata() {
    if std::env::var_os("TESSDATA_PREFIX").is_some() {
        return;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        format!("{home}/.local/share/tessdata"),
        "/usr/share/tessdata".to_string(),
        "/usr/share/tesseract-ocr/5/tessdata".to_string(),
        "/usr/share/tesseract-ocr/4.00/tessdata".to_string(),
        "/opt/homebrew/share/tessdata".to_string(),
        "/usr/local/share/tessdata".to_string(),
    ];
    for c in candidates {
        if std::path::Path::new(&c).join("eng.traineddata").exists() {
            std::env::set_var("TESSDATA_PREFIX", &c);
            tracing::info!("OCR: TESSDATA_PREFIX = {c}");
            return;
        }
    }
    tracing::warn!("OCR: no eng.traineddata found — on-screen text will be empty until tesseract English data is installed");
}
