//! Record from a video file, end to end → a beautified, **agent-queryable** clip.
//!
//! Reuses the Phase-1 import pipeline (frames → veyo gate → enrich → index) but stamps
//! `source: screen` and folds in the recording's own [`EventTrack`]: the clicks/keystrokes
//! become index `event_track` entries (queryable) and the cursor path produces the cinematic
//! zoom track (beautify). This is the file-source path — the live `scap`/PipeWire backend
//! produces the same `(frames, EventTrack)` and flows through here unchanged.

use crate::capture::LiveCapture;
use crate::{cinematic_track, to_index_events, EventTrack};
use anyhow::{ensure, Result};
use clipxd_cinematic::ZoomConfig;
use clipxd_import::{gate, map, media};
use clipxd_index::{Index, Source};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use veyo_core::CodecConfig;
use veyo_enrich::{EnrichInput, Enricher};

pub struct RecordOutput {
    pub clip_dir: PathBuf,
    pub index: Index,
    pub zoom_keyframes: usize,
}

/// Produce a clip from `video` + its `events`, written under `out_dir/<id>/`.
pub fn record_from_video(video: &Path, events: &EventTrack, out_dir: &Path, sample_fps: f32) -> Result<RecordOutput> {
    ensure!(video.exists(), "no such video: {}", video.display());
    let id = clip_id(video);
    let clip_dir = out_dir.join(&id);
    std::fs::create_dir_all(&clip_dir)?;
    let title = title_of(video);

    let info = media::probe(video)?;
    let frames = media::extract_frames(video, &clip_dir.join("frames"), sample_fps)?;
    ensure!(!frames.is_empty(), "no frames extracted from {}", video.display());
    let audio = media::extract_audio(video, &clip_dir.join("audio.wav"));

    // same salience gate + enrichment as import…
    let gated = gate::run_gate(&frames, (info.width.max(1), info.height.max(1)), &title, CodecConfig::default())?;
    let enricher = Enricher::with_local_defaults();
    let (tb, ob, cb) = enricher.backends();
    eprintln!("enrich backends: transcriber={tb} ocr={ob} caption={cb}");
    let enrichment = enricher.enrich(&EnrichInput {
        deltas: &gated.deltas,
        frames: &gated.salient_frames,
        audio: audio.as_deref(),
    })?;

    // …but it's a recording: source = screen, and the interaction track is part of the index.
    let mut index = map::to_index(&id, Source::Screen, &info, &title, &unix_secs(), &enrichment);
    index.event_track = to_index_events(events);

    // real speech-to-text if a whisper backend is installed (audio stays on the box); the
    // recording's narration becomes queryable alongside its on-screen text.
    if let Some(a) = audio.as_deref() {
        let tx = crate::transcribe::transcribe(a);
        if !tx.is_empty() {
            index.transcript = tx;
        }
    }

    // beautify: cursor path → cinematic zoom track. With no input track (e.g. a browser
    // screen recording) we derive the focus from veyo's salient deltas — content-aware
    // auto-zoom — so the recording still pushes in on the action.
    let focus = if events.is_empty() {
        crate::autofocus::focus_track_from_deltas(&gated.deltas, info.width, info.height)
    } else {
        events.clone()
    };
    let zoom = cinematic_track(&focus, info.duration_s, &ZoomConfig { fps: info.fps as f64, ..Default::default() });

    std::fs::write(clip_dir.join("index.json"), serde_json::to_string_pretty(&index)?)?;
    std::fs::write(clip_dir.join("zoom.json"), serde_json::to_string(&zoom)?)?;
    let _ = std::fs::copy(video, clip_dir.join("video.mp4"));

    Ok(RecordOutput { clip_dir, index, zoom_keyframes: zoom.len() })
}

/// Record a clip from a [`LiveCapture`] backend (frames already on disk — no ffmpeg
/// extraction). This is the live-recording path: a `FramesDirCapture` today, a scap or
/// PipeWire backend later, all flow through here identically.
pub fn record_from_capture(cap: &dyn LiveCapture, id: &str, title: &str, out_dir: &Path) -> Result<RecordOutput> {
    let info = cap.info();
    let frames = cap.frames();
    ensure!(!frames.is_empty(), "capture produced no frames");
    let events = cap.events();

    let clip_dir = out_dir.join(id);
    std::fs::create_dir_all(&clip_dir)?;

    let media_info = media::MediaInfo {
        duration_s: info.duration_s,
        width: info.width,
        height: info.height,
        fps: info.fps as f32,
    };
    let gated = gate::run_gate(&frames, (info.width.max(1), info.height.max(1)), title, CodecConfig::default())?;
    let enricher = Enricher::with_local_defaults();
    let enrichment = enricher.enrich(&EnrichInput {
        deltas: &gated.deltas,
        frames: &gated.salient_frames,
        audio: None,
    })?;

    let mut index = map::to_index(id, Source::Screen, &media_info, title, &unix_secs(), &enrichment);
    index.event_track = to_index_events(&events);
    let focus = if events.is_empty() {
        crate::autofocus::focus_track_from_deltas(&gated.deltas, info.width, info.height)
    } else {
        events.clone()
    };
    let zoom = cinematic_track(&focus, info.duration_s, &ZoomConfig { fps: info.fps, ..Default::default() });

    std::fs::write(clip_dir.join("index.json"), serde_json::to_string_pretty(&index)?)?;
    std::fs::write(clip_dir.join("zoom.json"), serde_json::to_string(&zoom)?)?;

    Ok(RecordOutput { clip_dir, index, zoom_keyframes: zoom.len() })
}

fn clip_id(p: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.display().to_string().hash(&mut h);
    format!("clp_{:08x}", h.finish() as u32)
}

fn title_of(p: &Path) -> String {
    p.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.replace(['_', '-'], " "))
        .unwrap_or_else(|| "Recording".into())
}

fn unix_secs() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs().to_string()).unwrap_or_else(|_| "0".into())
}
