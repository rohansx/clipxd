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
    /// One-off maintenance: repair clips already on disk, then exit WITHOUT serving.
    ///
    /// Remuxes any recording whose container carries no duration (every clip assembled by the
    /// old raw-concat path), refills `metadata.duration/resolution/fps` from the repaired file,
    /// recomputes the zoom track — which collapses to a single flat keyframe whenever the
    /// duration is 0 — moves clips stuck at `recording`/`enriching` to a terminal status, and
    /// removes empty clip directories a crashed session left behind.
    #[arg(long)]
    repair: bool,
    /// With `--repair`: re-upload every clip's `video.webm` to object storage even if this run
    /// changed nothing. Needed once after a repair whose mirror was misconfigured — the local
    /// files are already correct, so nothing would otherwise be re-sent.
    #[arg(long)]
    remirror: bool,
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
    if args.repair {
        return repair_clips(&args.clips_dir, args.remirror).await;
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

/// How long a clip may sit at a non-terminal status before `--repair` calls it dead. Generous:
/// a long recording legitimately stays at `recording` for its whole duration, and enrichment of
/// a big clip can run for many minutes behind the phase-2 semaphore.
const STUCK_AFTER_SECS: u64 = 6 * 3600;

/// One-off maintenance repair for clips already on disk — see the `--repair` flag docs.
///
/// The load-bearing fix is the remux. Recordings assembled by the old raw-concat path carry no
/// duration in the container, which cascades: `metadata.duration` is 0.0, the player can't scrub
/// without downloading the whole file, and `compute_zoom_track` emits `duration * fps + 1` = ONE
/// keyframe, so auto-zoom is dead on exactly those clips (verified across production: every clip
/// with a real duration has 286–3051 keyframes reaching 2.0×; every 0.0-duration clip has one
/// flat keyframe). Repairing the container repairs all three.
async fn repair_clips(clips_dir: &std::path::Path, force_mirror: bool) -> anyhow::Result<()> {
    let storage = clipxd_web::storage::StorageKind::from_env(clips_dir);
    let mut mirrored = 0u32;
    // Say which backend this run is writing to, up front. The hosted box serves S3 first, so a
    // repair that silently fell back to Local (an unset/unreadable CLIPXD_STORAGE — the env file
    // is root-only, and `source`ing it as the service user yields nothing) fixes every file on
    // disk and changes nothing anyone can see. That happened on the first production run.
    match &storage {
        clipxd_web::storage::StorageKind::Local { root } => println!(
            "storage: LOCAL ({}) — if this box serves from S3, set CLIPXD_STORAGE or the repair \
             will not be visible over HTTP\n",
            root.display()
        ),
        clipxd_web::storage::StorageKind::S3 { bucket, .. } => println!("storage: S3 ({bucket})\n"),
    }
    let (mut remuxed, mut retimed, mut rezoomed, mut unstuck, mut pruned) = (0u32, 0u32, 0u32, 0u32, 0u32);

    let mut dirs: Vec<PathBuf> =
        std::fs::read_dir(clips_dir)?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();

    for dir in dirs {
        let id = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let index_path = dir.join("index.json");
        let video = dir.join("video.webm");

        // An empty directory is the tombstone of a session that died before its first write —
        // nothing to serve, nothing to recover, and it shows up in no listing. Prune it.
        if !index_path.exists() && std::fs::read_dir(&dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
            if std::fs::remove_dir(&dir).is_ok() {
                println!("pruned    {id}: empty directory");
                pruned += 1;
            }
            continue;
        }
        if !index_path.exists() {
            println!("skip      {id}: no index.json (has files — left alone for a human)");
            continue;
        }

        let mut index: serde_json::Value = match std::fs::read_to_string(&index_path).ok().and_then(|s| serde_json::from_str(&s).ok()) {
            Some(v) => v,
            None => {
                println!("skip      {id}: unreadable index.json");
                continue;
            }
        };
        let mut dirty = false;
        // A repaired *video* also has to reach storage: the box serves S3 first, so a remux that
        // only lands on local disk leaves every viewer on the old, unseekable container.
        let mut video_dirty = force_mirror;

        // 1. Repair the container, then re-probe it.
        if video.exists() {
            // Ask the *container*, not `media::probe` — probe falls back to walking the file when
            // the header has no duration, so it happily reports 46.6 s for a file whose header
            // says nothing. That fallback is what a browser can't do: it has to download
            // everything before it can seek. So the remux decision has to read the header alone.
            if !container_has_duration(&video) {
                let staged = dir.join("video.remux.webm");
                if clipxd_web::remux_seekable(&video, &staged) {
                    // remux_seekable removes its source on success; put the repaired file back.
                    if std::fs::rename(&staged, &video).is_ok() {
                        println!("remuxed   {id}");
                        remuxed += 1;
                        video_dirty = true;
                    }
                }
            }
            if let Ok(info) = clipxd_import::media::probe(&video) {
                let stored = index.pointer("/metadata/duration").and_then(|d| d.as_f64()).unwrap_or(0.0);
                if info.duration_s > 0.0 && (stored - info.duration_s).abs() > 0.05 {
                    index["metadata"]["duration"] = serde_json::json!(info.duration_s);
                    index["metadata"]["resolution"] = serde_json::json!([info.width, info.height]);
                    index["metadata"]["fps"] = serde_json::json!(info.fps);
                    dirty = true;
                    retimed += 1;
                    println!("retimed   {id}: duration {stored:.2} -> {:.2}s", info.duration_s);

                    // 2. A real duration means the zoom track can finally be more than one frame.
                    if let Some(track) = read_event_track(&dir) {
                        let zoom = clipxd_recorder::zoom_track(&track, info.duration_s, info.fps as f64);
                        if zoom.len() > 1 && std::fs::write(dir.join("zoom.json"), serde_json::to_string(&zoom)?).is_ok() {
                            println!("rezoomed  {id}: {} keyframes", zoom.len());
                            rezoomed += 1;
                        }
                    }
                }
            }
        }

        // 3. Re-run the cleaner. It's idempotent by construction (there's a test asserting a
        //    second pass is a no-op), so this is free for already-clean clips and it back-fills
        //    whatever the cleaner has learned since a clip was indexed — currently the
        //    "A screenshot shows …" preamble strip that makes chapter lists readable.
        if let Ok(mut typed) = serde_json::from_value::<clipxd_index::Index>(index.clone()) {
            let before = typed.clone();
            clipxd_index::clean_index(&mut typed);
            if typed != before {
                index = serde_json::to_value(&typed)?;
                dirty = true;
                println!("recleaned {id}");
            }
        }

        // 4. Nothing may spin forever. A clip left at `recording`/`enriching` long past any
        //    plausible job is a crashed session, and the UI shows it as an eternal spinner.
        let status = index.get("status").and_then(|s| s.as_str()).unwrap_or("").to_string();
        if status == "recording" || status == "enriching" {
            let age = std::fs::metadata(&index_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if age > STUCK_AFTER_SECS {
                index["status"] = serde_json::json!("partial");
                dirty = true;
                unstuck += 1;
                println!("unstuck   {id}: {status} for {}h -> partial", age / 3600);
            }
        }

        if dirty {
            std::fs::write(&index_path, serde_json::to_string_pretty(&index)?)?;
        }
        // The hosted box serves S3 first, so a local-only write would never be seen.
        if dirty || video_dirty {
            if let Ok(st) = storage.make_storage().await {
                // `force_mirror` covers index.json too: an earlier repair run that fell back to
                // Local storage already cleaned the file on disk, so a later run finds nothing to
                // change, stays "not dirty", and would leave the stale copy in S3 forever.
                if dirty || force_mirror {
                    if let Ok(body) = std::fs::read(&index_path) {
                        if st.write_object(&format!("{id}/index.json"), body, "application/json").await.is_err() {
                            println!("          {id}: INDEX MIRROR FAILED — check CLIPXD_STORAGE");
                        }
                    }
                }
                if video_dirty && video.exists() {
                    match std::fs::read(&video) {
                        Ok(body) => {
                            let mb = body.len() as f64 / 1e6;
                            if st.write_object(&format!("{id}/video.webm"), body, "video/webm").await.is_err() {
                                println!("          {id}: VIDEO MIRROR FAILED — check CLIPXD_STORAGE");
                            } else {
                                println!("mirrored  {id}: video.webm ({mb:.1} MB)");
                                mirrored += 1;
                            }
                        }
                        Err(e) => println!("          {id}: could not read video to mirror: {e}"),
                    }
                }
            }
        }
    }

    println!(
        "\nrepair complete: {remuxed} remuxed, {retimed} retimed, {rezoomed} zoom tracks rebuilt, {unstuck} unstuck, {pruned} empty dirs pruned, {mirrored} videos mirrored"
    );
    Ok(())
}

/// Does the file's own header carry a duration? `-show_entries format=duration` alone, with no
/// decode fallback: this is exactly what a browser reads before deciding whether it can seek.
fn container_has_duration(video: &std::path::Path) -> bool {
    let out = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"])
        .arg(video)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().map(|d| d > 0.0).unwrap_or(false)
        }
        _ => false,
    }
}

/// A clip's recorded input events (`events.json`), if it has any — the input to the zoom track.
fn read_event_track(dir: &std::path::Path) -> Option<clipxd_recorder::EventTrack> {
    let s = std::fs::read_to_string(dir.join("events.json")).ok()?;
    clipxd_recorder::EventTrack::from_json(&s).ok()
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
