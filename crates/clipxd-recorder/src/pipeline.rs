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
use clipxd_index::{Index, Metadata, Source, Status};
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
    let _ = std::fs::copy(video, clip_dir.join("video.mp4"));
    let index = enrich_clip(video, &clip_dir, &id, &title, events, sample_fps)?;
    let zoom_keyframes = std::fs::read_to_string(clip_dir.join("zoom.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
        .map(|v| v.len())
        .unwrap_or(0);
    Ok(RecordOutput { clip_dir, index, zoom_keyframes })
}

/// **Phase 1 of an async ingest (Loom-style):** copy the video in and write a minimal index
/// with `status: enriching` so the clip is *immediately* watchable, listable, and shareable —
/// before any (slow) OCR/captioning runs. Fast: just a probe + a file copy. Returns the clip dir.
pub fn stub_clip(video: &Path, out_dir: &Path, id: &str, title: &str) -> Result<PathBuf> {
    ensure!(video.exists(), "no such video: {}", video.display());
    let clip_dir = out_dir.join(id);
    std::fs::create_dir_all(&clip_dir)?;
    let info = media::probe(video)?;
    let ext = video.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let _ = std::fs::copy(video, clip_dir.join(format!("video.{ext}")));
    let mut index = Index::new(
        id,
        Source::Screen,
        Metadata {
            duration: info.duration_s,
            resolution: [info.width, info.height],
            fps: info.fps,
            created_at: unix_secs(),
            title: title.to_string(),
            description: String::new(),
            app_focus: Vec::new(),
            url_context: None,
            has_video: true,
        },
    );
    index.status = Status::Enriching; // honest signal: streams are still filling in
    index.summary.tldr = "Indexing… the transcript, on-screen text, and captions are being built.".into();
    std::fs::write(clip_dir.join("index.json"), serde_json::to_string_pretty(&index)?)?;
    Ok(clip_dir)
}

/// Promote an instant-link `status: recording` stub once its final video is on disk: fill
/// in the real probe metadata and flip to `status: enriching`. The stub was written at
/// stage-open (before any video existed) so the share URL could resolve during recording;
/// this is the cheap commit-time counterpart of [`stub_clip`] — a probe + one JSON rewrite,
/// **no video copy** (the caller already moved the assembled file into `clip_dir`).
pub fn promote_recording_stub(clip_dir: &Path, video: &Path, id: &str, title: &str) -> Result<Index> {
    ensure!(video.exists(), "no such video: {}", video.display());
    let info = media::probe(video)?;
    let mut index = std::fs::read_to_string(clip_dir.join("index.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Index>(&s).ok())
        .unwrap_or_else(|| {
            Index::new(
                id,
                Source::Screen,
                Metadata {
                    duration: 0.0,
                    resolution: [0, 0],
                    fps: 0.0,
                    created_at: unix_secs(),
                    title: title.to_string(),
                    description: String::new(),
                    app_focus: Vec::new(),
                    url_context: None,
                    has_video: true,
                },
            )
        });
    index.metadata.duration = info.duration_s;
    index.metadata.resolution = [info.width, info.height];
    index.metadata.fps = info.fps;
    index.metadata.has_video = true;
    index.status = Status::Enriching;
    index.summary.tldr = "Indexing… the transcript, on-screen text, and captions are being built.".into();
    std::fs::write(clip_dir.join("index.json"), serde_json::to_string_pretty(&index)?)?;
    Ok(index)
}

/// **Phase 2:** the heavy part — frames → veyo gate → enrich (OCR/caption) → index, written
/// into an existing `clip_dir` (overwriting any stub). Used by both `record_from_video` and the
/// async ingest's background task. If `events` is empty, a sibling `events.json` (e.g. from a
/// browser cursor POST) is honored so the camera still follows the cursor.
pub fn enrich_clip(video: &Path, clip_dir: &Path, id: &str, title: &str, events: &EventTrack, sample_fps: f32) -> Result<Index> {
    ensure!(video.exists(), "no such video: {}", video.display());
    let info = media::probe(video)?;
    let frames = media::extract_frames(video, &clip_dir.join("frames"), sample_fps)?;
    ensure!(!frames.is_empty(), "no frames extracted from {}", video.display());
    let audio = media::extract_audio(video, &clip_dir.join("audio.wav"));

    // The transcript (whisper over audio.wav, CPU-bound and often the longest single stage)
    // and the visual enrichment (gate → OCR → caption network calls) share no data — run
    // them side by side instead of back-to-back.
    let (visual, tx) = std::thread::scope(|scope| {
        let tx_handle = scope.spawn(|| audio.as_deref().map(crate::transcribe::transcribe).unwrap_or_default());
        let visual = (|| -> Result<(gate::GateOutput, veyo_enrich::Enrichment)> {
            let gated = gate::run_gate(&frames, (info.width.max(1), info.height.max(1)), title, CodecConfig::default())?;
            let enricher = Enricher::with_local_defaults();
            let (tb, ob, cb) = enricher.backends();
            eprintln!("enrich backends: transcriber={tb} ocr={ob} caption={cb}");
            let enrichment = enricher.enrich(&EnrichInput {
                deltas: &gated.deltas,
                frames: &gated.salient_frames,
                audio: audio.as_deref(),
            })?;
            Ok((gated, enrichment))
        })();
        let tx = tx_handle.join().unwrap_or_else(|e| {
            eprintln!("transcribe thread panicked: {e:?} (continuing without a transcript)");
            Vec::new()
        });
        (visual, tx)
    });
    let (gated, enrichment) = visual?;

    let mut index = map::to_index(id, Source::Screen, &info, title, &unix_secs(), &enrichment);

    // Interaction track: the param, else a cursor track saved alongside (async cursor POST).
    let track = if !events.is_empty() {
        events.clone()
    } else {
        std::fs::read_to_string(clip_dir.join("events.json")).ok().and_then(|s| EventTrack::from_json(&s).ok()).unwrap_or_default()
    };
    index.event_track = to_index_events(&track);

    if !tx.is_empty() {
        index.transcript = tx;
    }

    let focus = if track.is_empty() {
        crate::autofocus::focus_track_from_deltas(&gated.deltas, info.width, info.height)
    } else {
        track
    };
    let zoom = cinematic_track(&focus, info.duration_s, &ZoomConfig { fps: info.fps as f64, ..Default::default() });

    clipxd_index::clean_index(&mut index); // dedup noisy streams + build the search corpus
    std::fs::write(clip_dir.join("index.json"), serde_json::to_string_pretty(&index)?)?;
    std::fs::write(clip_dir.join("zoom.json"), serde_json::to_string(&zoom)?)?;
    Ok(index)
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

    clipxd_index::clean_index(&mut index); // dedup noisy streams + build the search corpus
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

pub(crate) fn unix_secs() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs().to_string()).unwrap_or_else(|_| "0".into())
}
