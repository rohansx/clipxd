//! The Phase-1 import pipeline: fetch → demux → veyo gate → veyo-enrich → `index.json`.
//!
//! ```text
//!   input (URL|file) ─► fetch ─► probe ─► extract frames ─┐
//!                                         extract audio ─┐ │
//!                                                        ▼ ▼
//!                                  veyo-core gate (which moments matter)
//!                                                        │ deltas + retained salient frames
//!                                                        ▼
//!                                  veyo-enrich (transcript · OCR · caption)
//!                                                        │
//!                                                        ▼
//!                                  map ─► index.json  (the clip artifact)
//! ```

use crate::{gate, map, media};
use anyhow::{Context, Result};
use clipxd_index::{Index, Source};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use veyo_core::CodecConfig;
use veyo_enrich::{EnrichInput, Enricher};

/// Tunables for an import run.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// Frames per second to sample from the source video.
    pub sample_fps: f32,
    /// Override the codec salience floor (lower = emit more densely — "degrade mode"
    /// while veyo's gate is unproven). `None` uses veyo's default (0.4).
    pub salience_min: Option<f32>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            sample_fps: 4.0,
            salience_min: None,
        }
    }
}

/// The result of an import: where the clip lives, its index, and which enrich backends ran.
pub struct ImportOutput {
    pub clip_dir: PathBuf,
    pub index: Index,
    pub backends: (String, String, String),
}

/// Import `input` (a URL or local file) into a clip directory under `out_dir`.
pub fn import(input: &str, out_dir: &Path, opts: &ImportOptions) -> Result<ImportOutput> {
    let id = clip_id(input);
    let clip_dir = out_dir.join(&id);
    std::fs::create_dir_all(&clip_dir).context("creating clip dir")?;
    let frames_dir = clip_dir.join("frames");

    // 1. fetch + probe
    let video = media::fetch(input, &clip_dir)?;
    let info = media::probe(&video)?;
    tracing::info!(
        duration = info.duration_s,
        w = info.width,
        h = info.height,
        fps = info.fps,
        "probed source"
    );

    // 2. demux frames (+ best-effort audio)
    let frames = media::extract_frames(&video, &frames_dir, opts.sample_fps)?;
    anyhow::ensure!(!frames.is_empty(), "no frames extracted from {input}");
    let audio = media::extract_audio(&video, &clip_dir.join("audio.wav"));
    tracing::info!(frames = frames.len(), audio = audio.is_some(), "demuxed");

    // 3. veyo-core salience gate
    let mut cfg = CodecConfig::default();
    if let Some(sm) = opts.salience_min {
        cfg.salience_min = sm;
    }
    let title = derive_title(input);
    let gate = gate::run_gate(&frames, (info.width.max(1), info.height.max(1)), &title, cfg)?;
    tracing::info!(
        deltas = gate.deltas.len(),
        salient_frames = gate.salient_frames.len(),
        "gated"
    );

    // 4. veyo-enrich
    let enricher = Enricher::with_local_defaults();
    let (tb, ob, cb) = enricher.backends();
    let enrichment = enricher.enrich(&EnrichInput {
        deltas: &gate.deltas,
        frames: &gate.salient_frames,
        audio: audio.as_deref(),
    })?;
    tracing::info!(
        transcript = enrichment.transcript.len(),
        on_screen_text = enrichment.on_screen_text.len(),
        moments = enrichment.visual_timeline.len(),
        "enriched (transcriber={tb}, ocr={ob}, captioner={cb})"
    );

    // 5. map → index, persist
    let index = map::to_index(&id, Source::Import, &info, &title, &unix_secs(), &enrichment);
    persist_video(&video, &clip_dir).ok();
    std::fs::write(
        clip_dir.join("index.json"),
        serde_json::to_string_pretty(&index)?,
    )
    .context("writing index.json")?;

    Ok(ImportOutput {
        clip_dir,
        index,
        backends: (tb.to_string(), ob.to_string(), cb.to_string()),
    })
}

/// Stable-ish clip id derived from the input string.
fn clip_id(input: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut h);
    format!("clp_{:08x}", h.finish() as u32)
}

/// Human title from a file stem or URL tail.
fn derive_title(input: &str) -> String {
    let tail = input.trim_end_matches('/').rsplit(['/', '\\']).next().unwrap_or(input);
    let stem = tail.split('?').next().unwrap_or(tail);
    let stem = Path::new(stem)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(stem);
    if stem.is_empty() {
        "Untitled clip".to_string()
    } else {
        stem.replace(['_', '-'], " ")
    }
}

fn unix_secs() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// Copy the source video into the clip dir (so the clip is self-contained), unless it's
/// already there.
fn persist_video(video: &Path, clip_dir: &Path) -> Result<()> {
    if video.parent() == Some(clip_dir) {
        return Ok(());
    }
    let ext = video.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    std::fs::copy(video, clip_dir.join(format!("video.{ext}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_from_path_and_url() {
        assert_eq!(derive_title("/home/me/checkout_flow.mp4"), "checkout flow");
        assert_eq!(derive_title("https://loom.com/share/abc123.mp4?t=1"), "abc123");
        assert_eq!(derive_title("clip-one.webm"), "clip one");
    }

    #[test]
    fn clip_id_is_stable_and_prefixed() {
        let a = clip_id("/x/y.mp4");
        let b = clip_id("/x/y.mp4");
        assert_eq!(a, b);
        assert!(a.starts_with("clp_"));
    }
}
