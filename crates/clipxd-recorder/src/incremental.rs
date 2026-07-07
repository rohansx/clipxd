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
use clipxd_index::{Index, Source, TranscriptSegment};
use std::path::{Path, PathBuf};
use veyo_core::{CodecConfig, Delta};
use veyo_enrich::{CaptionSource, EnrichInput, Enricher, Enrichment};

/// How much of the newest audio each incremental pass leaves untouched. Whisper is given a
/// `[watermark, boundary - HOLDBACK)` slice, never the raw tail — an utterance can still be
/// mid-word right at a pass's boundary (which only exists because that's where a 15s
/// MediaRecorder chunk happened to end, not because anyone stopped talking), and the next
/// pass sees more of it. Coarser than real streaming-ASR chunking (VAD-based segmentation,
/// overlapping windows) would do, but proportionate to what a single-speaker screen-recording
/// narration needs, and it never loses audio — a word only shows up a few seconds later than
/// it was spoken, in the same slice-boundary tradeoff the frame holdback above already makes.
const TRANSCRIBE_HOLDBACK_MS: u64 = 3_000;

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
    /// BYOK/local-mode override for this session's owner, decided once at session creation
    /// (the owner is already known by then — see `ingest_stage_create`) and applied to every
    /// incremental pass and the final one alike. `None` = the server's usual env-driven cascade.
    caption_source: Option<CaptionSource>,
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
    /// Transcript segments committed so far, timestamps already offset to be video-relative
    /// (whisper itself only ever sees one slice at a time and reports slice-relative times).
    transcript: Vec<TranscriptSegment>,
    /// How far (video-relative ms) audio has been transcribed. `None` = nothing yet.
    max_transcript_ms: Option<u64>,
}

impl IncrementalIndexer {
    pub fn new(frames_dir: PathBuf, sample_fps: f32, caption_source: Option<CaptionSource>) -> Self {
        Self {
            frames_dir,
            sample_fps,
            caption_source,
            enrichment: Enrichment::default(),
            all_deltas: Vec::new(),
            max_delta_ms: None,
            max_frame_ms: None,
            covered_ms: std::collections::HashSet::new(),
            transcript: Vec::new(),
            max_transcript_ms: None,
        }
    }

    /// Re-decode + re-gate `video_so_far`, enriching only what's new since the last call. Safe
    /// to call repeatedly as the video grows (idempotent on already-processed frames). Also
    /// transcribes any newly-arrived audio (holdback applies — see [`TRANSCRIBE_HOLDBACK_MS`])
    /// so speech is searchable while the recording is still running, not just after Stop.
    pub fn add_increment(&mut self, video_so_far: &Path, title: &str) -> Result<()> {
        self.run_pass(video_so_far, title, true)?;
        self.transcribe_pass(video_so_far, true);
        Ok(())
    }

    /// Transcribe the `[max_transcript_ms, boundary)` audio slice (minus holdback on a
    /// mid-recording pass) and append it. Extracting only the *new* slice — not the whole
    /// growing recording — is what keeps this cheap every ~15s chunk instead of O(n²) over a
    /// long recording; whisper on a 15s slice is itself well under a second of new work on a
    /// tiny/base model. Best-effort: no audio track, ffmpeg failure, or no whisper binary
    /// installed all just mean this pass contributes nothing, same as `transcribe::transcribe`'s
    /// existing empty-on-failure contract — never fatal to the recording.
    fn transcribe_pass(&mut self, video_so_far: &Path, holdback: bool) {
        let Ok(info) = media::probe(video_so_far) else { return };
        let boundary_ms = (info.duration_s * 1000.0).round() as u64;
        let start_ms = self.max_transcript_ms.unwrap_or(0);
        let end_ms = if holdback {
            match boundary_ms.checked_sub(TRANSCRIBE_HOLDBACK_MS) {
                Some(e) if e > start_ms => e,
                _ => return, // not enough new audio past the holdback yet -- try next pass
            }
        } else {
            boundary_ms
        };
        if end_ms <= start_ms {
            return;
        }
        let start_s = start_ms as f64 / 1000.0;
        let slice_wav = self.frames_dir.join("transcribe-slice.wav");
        let Some(wav) = media::extract_audio_range(video_so_far, &slice_wav, start_s, Some((end_ms - start_ms) as f64 / 1000.0)) else {
            return; // no audio track (yet) or extraction failed -- retry next pass
        };
        let mut segs = crate::transcribe::transcribe(&wav);
        let _ = std::fs::remove_file(&wav);
        for seg in &mut segs {
            seg.start += start_s;
            seg.end += start_s;
        }
        self.transcript.extend(segs);
        // Advance regardless of whether this slice held speech -- a silent slice has still
        // been "looked at" and must not be re-transcribed forever waiting for it to speak.
        self.max_transcript_ms = Some(end_ms);
    }

    /// One final pass over `video` (the fully-committed recording, no holdback — this really
    /// is the end), then build + write the completed `Index` + zoom track into `clip_dir`,
    /// moving the incrementally-populated frames directory into place. Consumes `self` — a
    /// session's indexer is used exactly once.
    ///
    /// Unlike `pipeline::enrich_clip`'s from-scratch finalize (which overlaps a whole-file
    /// transcribe against the enrich pass in a scoped thread, since it has no prior transcript
    /// and whisper-over-everything is the long pole), this finalize doesn't need that: most of
    /// the transcript already accumulated during recording via `add_increment`'s per-chunk
    /// `transcribe_pass`, so all that's left is one small no-holdback tail slice — cheap enough
    /// to just run inline after the visual pass rather than fight `self`'s split-borrow to
    /// parallelize two `&mut self` calls.
    pub fn finalize(mut self, video: &Path, clip_dir: &Path, id: &str, title: &str, events: &EventTrack) -> Result<Index> {
        self.run_pass(video, title, false)?;
        self.transcribe_pass(video, false);
        let info = media::probe(video)?;

        let mut index = map::to_index(id, Source::Screen, &info, title, &unix_secs(), &self.enrichment);

        let track = if !events.is_empty() {
            events.clone()
        } else {
            std::fs::read_to_string(clip_dir.join("events.json")).ok().and_then(|s| EventTrack::from_json(&s).ok()).unwrap_or_default()
        };
        index.event_track = to_index_events(&track);

        if !self.transcript.is_empty() {
            index.transcript = self.transcript;
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
            let enricher = Enricher::with_caption_source(self.caption_source.clone());
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

    fn have_ffmpeg() -> bool {
        std::process::Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    /// A short video WITH an audio track (silence is fine — whisper isn't installed in this
    /// environment either, so every real test run exercises the "extraction succeeds,
    /// transcribe() finds nothing" branch; that branch is exactly what needs covering: the
    /// watermark must still advance on empty results, or a silent recording would spin
    /// re-transcribing the same dead air every single chunk forever).
    fn make_video_with_audio(path: &Path, duration_s: u32) {
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i", &format!("testsrc=duration={duration_s}:size=320x240:rate=10")])
            .args(["-f", "lavfi", "-i", &format!("anullsrc=r=16000:cl=mono:d={duration_s}")])
            .args(["-c:v", "libvpx", "-c:a", "libvorbis", "-f", "webm"])
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("ffmpeg should run");
        assert!(status.success(), "audio+video fixture generation failed");
    }

    #[test]
    fn transcribe_pass_holdback_advances_watermark_without_reaching_the_live_edge() {
        if !have_ffmpeg() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        let tmp = std::env::temp_dir().join(format!("clipxd-incr-tx-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let video = tmp.join("video.webm");
        make_video_with_audio(&video, 10);

        // ffmpeg's own output duration for a "10s" testsrc/anullsrc pair isn't exactly
        // 10.000s (frame-rate rounding on the video side vs. the audio side) -- probe the
        // real value rather than assume a round number the fixture doesn't actually produce.
        let boundary_ms = (media::probe(&video).unwrap().duration_s * 1000.0).round() as u64;

        let frames_dir = tmp.join("frames");
        // transcribe_pass writes its scratch slice into frames_dir; in the real add_increment/
        // finalize call sequence run_pass (via extract_frames) always creates it first. This
        // test isolates transcribe_pass alone, so create it explicitly to match that invariant.
        std::fs::create_dir_all(&frames_dir).unwrap();
        let mut indexer = IncrementalIndexer::new(frames_dir, 4.0, None);
        indexer.transcribe_pass(&video, true);
        let after_holdback = indexer.max_transcript_ms.expect("a 10s clip minus the 3s holdback should commit some watermark");
        assert_eq!(
            after_holdback,
            boundary_ms - TRANSCRIBE_HOLDBACK_MS,
            "a holdback pass with no prior watermark should commit exactly [0, boundary - HOLDBACK)"
        );

        // A second holdback pass over the SAME (unchanged) video must not regress or spin --
        // there's no new audio since the first pass already consumed everything short of the
        // live edge, so the watermark should hold steady rather than re-processing.
        let before = indexer.max_transcript_ms;
        indexer.transcribe_pass(&video, true);
        assert_eq!(indexer.max_transcript_ms, before, "no new audio available -> watermark must not move");

        // The final (no-holdback) pass must reach all the way to the true end.
        indexer.transcribe_pass(&video, false);
        assert_eq!(indexer.max_transcript_ms, Some(boundary_ms), "final pass should commit through the true end, holdback lifted");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn transcribe_pass_on_video_with_no_audio_track_is_a_harmless_noop() {
        if !have_ffmpeg() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        let tmp = std::env::temp_dir().join(format!("clipxd-incr-tx-noaudio-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let video = tmp.join("video.webm");
        // testsrc alone -- no -f lavfi audio input -- produces a video-only file.
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i", "testsrc=duration=5:size=320x240:rate=10"])
            .args(["-c:v", "libvpx", "-f", "webm"])
            .arg(&video)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("ffmpeg should run");
        assert!(status.success());

        let frames_dir = tmp.join("frames");
        std::fs::create_dir_all(&frames_dir).unwrap();
        let mut indexer = IncrementalIndexer::new(frames_dir, 4.0, None);
        indexer.transcribe_pass(&video, true);
        assert_eq!(indexer.max_transcript_ms, None, "no audio track -> watermark stays unset, never fails/panics");
        assert!(indexer.transcript.is_empty());

        std::fs::remove_dir_all(&tmp).ok();
    }
}
