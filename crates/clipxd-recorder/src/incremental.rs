//! Incremental Phase-2 indexing for the streaming-upload path: as each ~15s chunk lands on
//! the server, re-run frame extraction + the salience gate over the video assembled so far,
//! but only OCR/caption deltas and frames that are new since the last pass. By the time the
//! recording stops, most of the enrichment work is already done and only the last partial
//! chunk's tail remains — instead of the whole clip's OCR/captioning running after `stop`.
//!
//! The gate is re-run from scratch (a fresh [`veyo_core::Codec`] over the whole frame prefix)
//! on every increment rather than threading a live `Codec` + seek-based extraction across HTTP
//! requests. `Codec::observe` is purely causal — a region's past decisions are never revised by
//! later frames — so a full replay reproduces bit-identical deltas to the eventual whole-clip
//! gate. This trades some redundant frame decode+gate work (cheap: sub-few-seconds even for
//! several minutes at 4fps) for zero risk of the continuity bugs a bespoke incremental Codec
//! would carry. Only the expensive OCR/caption step is truly incremental.

use crate::{autofocus, cinematic_track, pipeline::unix_secs, to_index_events, EventTrack};
use anyhow::Result;
use clipxd_cinematic::ZoomConfig;
use clipxd_import::{gate, map, media};
use clipxd_index::{Index, Source};
use std::path::{Path, PathBuf};
use veyo_core::{CodecConfig, Delta};
use veyo_enrich::{EnrichInput, Enricher, Enrichment};

/// Per-session accumulator, alive for the lifetime of one staged recording upload.
///
/// Deltas and salient frames need different "have we already enriched this" watermarks:
/// `Codec::observe` is purely causal (a region's past decisions are never revised by later
/// frames), so a delta at a given `t_event` is bit-identical no matter how much more video
/// follows it — safe to commit the moment it's seen. `gate::run_gate`'s keyframe-floor
/// guarantee, however, always keeps *whatever frame it was last given* — correct for a
/// complete clip's final scene, but on a mid-recording pass that "last frame" is only an
/// artifact of where this particular pass happened to stop, not a real ending. So the one
/// frame sitting exactly at each pass's boundary is deliberately left off the frame watermark
/// (held back for the next, later-horizoned pass to re-evaluate) while everything before it —
/// and all deltas up to and including the boundary — commit immediately.
pub struct IncrementalIndexer {
    frames_dir: PathBuf,
    sample_fps: f32,
    enrichment: Enrichment,
    all_deltas: Vec<Delta>,
    /// `None` means nothing committed yet — distinct from `Some(0)` (the frame/delta *at*
    /// t=0 already committed), since t=0 is itself a legitimate timestamp.
    max_delta_ms: Option<u64>,
    max_frame_ms: Option<u64>,
    /// Timestamps already represented in `enrichment.visual_timeline` (from either a delta or
    /// a keyframe-floor frame). `Enricher::enrich` dedupes a floor frame against a delta at the
    /// same instant *within one call*, but each incremental pass is a separate call — without
    /// this, a delta committed in an earlier pass and a floor mark landing on that same instant
    /// in a later pass (once it's no longer that pass's own boundary) would double up.
    covered_ms: std::collections::HashSet<u64>,
}

impl IncrementalIndexer {
    pub fn new(frames_dir: PathBuf, sample_fps: f32) -> Self {
        Self {
            frames_dir,
            sample_fps,
            enrichment: Enrichment::default(),
            all_deltas: Vec::new(),
            max_delta_ms: None,
            max_frame_ms: None,
            covered_ms: std::collections::HashSet::new(),
        }
    }

    /// Re-decode + re-gate `video_so_far`, enriching only what's new since the last call. Safe
    /// to call repeatedly as the video grows (idempotent on already-processed frames).
    pub fn add_increment(&mut self, video_so_far: &Path, title: &str) -> Result<()> {
        self.run_pass(video_so_far, title, true)
    }

    /// One final pass over `video` (the fully-committed recording, no holdback — this really
    /// is the end), then build + write the completed `Index` + zoom track into `clip_dir`,
    /// moving the incrementally-populated frames directory into place. Consumes `self` — a
    /// session's indexer is used exactly once.
    pub fn finalize(mut self, video: &Path, clip_dir: &Path, id: &str, title: &str, events: &EventTrack) -> Result<Index> {
        // The final tail pass (frames → gate → OCR/caption calls) and the audio transcript
        // share no data — overlap them; whisper on CPU is often the longest post-stop stage.
        let (pass, tx) = std::thread::scope(|scope| {
            let tx_handle = scope.spawn(|| {
                let audio = media::extract_audio(video, &clip_dir.join("audio.wav"));
                audio.as_deref().map(crate::transcribe::transcribe).unwrap_or_default()
            });
            let pass = self.run_pass(video, title, false);
            let tx = tx_handle.join().unwrap_or_else(|e| {
                eprintln!("transcribe thread panicked: {e:?} (continuing without a transcript)");
                Vec::new()
            });
            (pass, tx)
        });
        pass?;
        let info = media::probe(video)?;

        let mut index = map::to_index(id, Source::Screen, &info, title, &unix_secs(), &self.enrichment);

        let track = if !events.is_empty() {
            events.clone()
        } else {
            std::fs::read_to_string(clip_dir.join("events.json")).ok().and_then(|s| EventTrack::from_json(&s).ok()).unwrap_or_default()
        };
        index.event_track = to_index_events(&track);

        if !tx.is_empty() {
            index.transcript = tx;
        }

        let focus = if track.is_empty() { autofocus::focus_track_from_deltas(&self.all_deltas, info.width, info.height) } else { track };
        let zoom = cinematic_track(&focus, info.duration_s, &ZoomConfig { fps: info.fps as f64, ..Default::default() });

        let target_frames = clip_dir.join("frames");
        if self.frames_dir != target_frames {
            move_dir(&self.frames_dir, &target_frames)?;
        }

        clipxd_index::clean_index(&mut index); // dedup noisy streams + build the search corpus
        std::fs::write(clip_dir.join("index.json"), serde_json::to_string_pretty(&index)?)?;
        std::fs::write(clip_dir.join("zoom.json"), serde_json::to_string(&zoom)?)?;
        Ok(index)
    }

    /// Shared gate+enrich pass over the whole `video_so_far` (always decoded from t=0 — see
    /// the module doc for why). `holdback` excludes the pass's own boundary frame from the
    /// frame watermark (see the struct doc) — set for every mid-recording increment, cleared
    /// for `finalize`'s one true final pass.
    fn run_pass(&mut self, video_so_far: &Path, title: &str, holdback: bool) -> Result<()> {
        let info = media::probe(video_so_far)?;
        let frames = media::extract_frames(video_so_far, &self.frames_dir, self.sample_fps)?;
        if frames.is_empty() {
            return Ok(());
        }
        let boundary_ms = frames.last().map(|(t, _)| *t).unwrap_or(0);

        let gated = gate::run_gate(&frames, (info.width.max(1), info.height.max(1)), title, CodecConfig::default())?;

        let new_deltas: Vec<Delta> =
            gated.deltas.iter().filter(|d| self.max_delta_ms.map_or(true, |m| d.t_event > m)).cloned().collect();
        let new_frames: Vec<_> = gated
            .salient_frames
            .iter()
            .filter(|f| {
                self.max_frame_ms.map_or(true, |m| f.t_ms > m)
                    && (!holdback || f.t_ms != boundary_ms)
                    && !self.covered_ms.contains(&f.t_ms)
            })
            .cloned()
            .collect();

        if !new_deltas.is_empty() || !new_frames.is_empty() {
            let enricher = Enricher::with_local_defaults();
            let partial = enricher.enrich(&EnrichInput { deltas: &new_deltas, frames: &new_frames, audio: None })?;
            self.covered_ms.extend(partial.visual_timeline.iter().map(|m| m.t_ms));
            self.enrichment.on_screen_text.extend(partial.on_screen_text);
            self.enrichment.visual_timeline.extend(partial.visual_timeline);
        }

        // gated.deltas is a full deterministic replay (a superset of any prior pass) — keep the
        // freshest complete copy so the final zoom track sees the whole session's deltas.
        self.all_deltas = gated.deltas;
        self.max_delta_ms = Some(self.max_delta_ms.map_or(boundary_ms, |m| m.max(boundary_ms)));
        // The boundary frame itself stays un-watermarked on a holdback pass -- exactly the one
        // frame withheld above -- so a later, real horizon can still pick it up. Skip advancing
        // at all when boundary_ms is 0 (a single-frame pass): there's no u64 below it to mark
        // "everything before the boundary is safe" without also covering the boundary itself.
        let frame_advance = if holdback { boundary_ms.checked_sub(1) } else { Some(boundary_ms) };
        if let Some(adv) = frame_advance {
            self.max_frame_ms = Some(self.max_frame_ms.map_or(adv, |m| m.max(adv)));
        }
        Ok(())
    }
}

/// Move `src` to `dst` (renaming `dst` out of the way first, since `enrich_clip`'s frame
/// extraction — never run for the incremental path — would otherwise have already created it).
/// Falls back to a recursive copy across filesystem boundaries, where `rename` can't cross.
fn move_dir(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        std::fs::remove_dir_all(dst)?;
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_dir_all(src, dst)?;
    std::fs::remove_dir_all(src)?;
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_dir_relocates_contents() {
        let tmp = std::env::temp_dir().join(format!("clipxd-incr-test-{}", std::process::id()));
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.png"), b"fake").unwrap();

        move_dir(&src, &dst).unwrap();

        assert!(!src.exists());
        assert!(dst.join("a.png").exists());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
