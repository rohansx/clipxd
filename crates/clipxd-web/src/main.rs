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
    /// Read-only public mode (safe behind a tunnel): no ingest/render/cursor, no clip listing.
    /// Also enabled by `CLIPXD_PUBLIC=1`.
    #[arg(long)]
    public: bool,
    /// One-off maintenance: (re)generate an LLM title + description for every clip still sitting
    /// at the recorder's default "Screen recording" title (e.g. clips recorded while the LLM
    /// backend was down/over-quota), then exit WITHOUT serving. Needs a backend key in the
    /// environment (OLLAMA_API_KEY / NVIDIA_API_KEY / GEMINI_API_KEY) and, in hosted mode, the
    /// CLIPXD_STORAGE + S3 keys so the regenerated index mirrors back to object storage.
    #[arg(long)]
    backfill_titles: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("clipxd=info,tower_http=info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_target(false).init();

    ensure_tessdata();

    let args = Args::parse();
    anyhow::ensure!(args.clips_dir.is_dir(), "clips dir not found: {}", args.clips_dir.display());

    if args.backfill_titles {
        return backfill_titles(&args.clips_dir).await;
    }

    let public = args.public
        || std::env::var("CLIPXD_PUBLIC").map(|v| !v.is_empty() && v != "0").unwrap_or(false);
    let app = clipxd_web::app(args.clips_dir.clone(), public);
    let addr = format!("0.0.0.0:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.with_context(|| format!("binding {addr}"))?;
    let mode = if public { "READ-ONLY public" } else { "full" };
    tracing::info!("clipxd-web serving {} ({mode}) on http://{addr}", args.clips_dir.display());
    axum::serve(listener, app).await?;
    Ok(())
}

/// One-off maintenance backfill: (re)title every clip still at the default "Screen recording".
///
/// Reuses the exact indexing-time pass (`deeppass::generate_title_and_description`) so the prompt
/// and merge rules are identical to normal enrichment — it's idempotent, guarded on the default
/// title (never stomps a real one), and only writes when the model returns a usable title. After
/// each success the updated `index.json` is mirrored to object storage, because the hosted box
/// serves from S3 first (a local-only write would never be seen). Clips with no transcript / OCR /
/// captions to summarize are reported and skipped — there is nothing to title them from.
async fn backfill_titles(clips_dir: &std::path::Path) -> anyhow::Result<()> {
    let storage = clipxd_web::storage::StorageKind::from_env(clips_dir);
    let (mut titled, mut reconciled, mut nothing) = (0u32, 0u32, 0u32);

    let mut dirs: Vec<PathBuf> = std::fs::read_dir(clips_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    for dir in dirs {
        let index_path = dir.join("index.json");
        if !index_path.exists() {
            continue;
        }
        let id = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let was_default = read_title(&index_path).as_deref() == Some("Screen recording");

        // Title it if it's still at the recorder default.
        if was_default {
            if let Err(e) = clipxd_web::deeppass::generate_title_and_description(&dir, &id, None, None).await {
                // Usually "no transcript/OCR/captions yet to title from" — a silent, text-less
                // recording the LLM has nothing to work with. Not a failure.
                println!("nothing {id}: {e:#}");
                nothing += 1;
                continue;
            }
        }

        // Reconcile: push the (possibly newly-titled) local index.json to storage so the
        // S3-served copy matches disk. Local is always the source of truth — every title/index
        // write in the server pairs a local write with an S3 mirror — so this only ever fixes
        // drift (e.g. a clip titled while CLIPXD_STORAGE was misconfigured), never regresses S3.
        match read_title(&index_path) {
            Some(t) if t != "Screen recording" => {
                let mirrored_ok = match storage.make_storage().await {
                    Ok(st) => match std::fs::read(&index_path) {
                        Ok(body) => st.write_object(&format!("{id}/index.json"), body, "application/json").await.is_ok(),
                        Err(_) => false,
                    },
                    Err(_) => false,
                };
                let warn = if mirrored_ok { "" } else { "  (STORAGE MIRROR FAILED — check CLIPXD_STORAGE)" };
                if was_default {
                    println!("titled    {id}: {t}{warn}");
                    titled += 1;
                } else {
                    println!("reconcile {id}: {t}{warn}");
                    reconciled += 1;
                }
            }
            _ => {
                nothing += 1;
            }
        }
    }
    println!("\nbackfill complete: {titled} titled, {reconciled} reconciled to storage, {nothing} had nothing to title from");
    Ok(())
}

/// The `metadata.title` in a clip's `index.json`, if readable.
fn read_title(index_path: &std::path::Path) -> Option<String> {
    let s = std::fs::read_to_string(index_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    v.get("metadata")?.get("title")?.as_str().map(str::to_string)
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
