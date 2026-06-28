//! The veyo-core salience gate over decoded frames.
//!
//! This is where clipxd consumes the codec: each frame is downscaled to cells and fed to
//! [`Codec::observe`](veyo_core::Codec::observe); the codec decides which moments are
//! salient and emits [`Delta`](veyo_core::Delta)s. clipxd retains the salient frames (the
//! codec discards pixels) so they can be OCR'd and captioned by veyo-enrich.

use crate::downscale::rgba_to_cells;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use veyo_core::{Codec, CodecConfig, Delta, Frame, SurfaceRef};
use veyo_enrich::SalientFrame;

/// Keyframe-floor cadence: veyo's salience is luma-based, so a scene that changes only in
/// colour (red→green) or that simply persists can go unflagged — and then the agent never
/// sees it (and may infer the opposite). We guarantee coverage by also keeping a frame at
/// every multiple of this interval, so no scene longer than it goes unindexed.
const KEYFRAME_FLOOR_MS: u64 = 2000;

/// Output of the gate: the salient deltas, and the unique frames they point at (deduped,
/// ready to hand to enrichment for OCR + captioning).
pub struct GateOutput {
    pub deltas: Vec<Delta>,
    pub salient_frames: Vec<SalientFrame>,
}

/// Run the codec over `frames` (each `(t_ms, png_path)`), at the given display `dims`.
///
/// `title` labels the synthetic surface (import has one focused surface). The codec
/// config is veyo's default — in "degrade mode" a caller can lower `salience_min` to emit
/// more densely until the codec gate is formally proven, without changing the schema.
pub fn run_gate(
    frames: &[(u64, PathBuf)],
    dims: (u32, u32),
    title: &str,
    cfg: CodecConfig,
) -> Result<GateOutput> {
    let surface = SurfaceRef {
        id: "import:0".to_string(),
        app: String::new(),
        title: title.to_string(),
        focused: true,
    };
    let grid = cfg.grid;
    let mut codec = Codec::new(cfg, surface, dims);

    let mut deltas: Vec<Delta> = Vec::new();
    for (t_ms, path) in frames {
        let cells = load_cells(path, grid).with_context(|| format!("decoding {}", path.display()))?;
        deltas.extend(codec.observe(Frame { t_ms: *t_ms, cells: &cells }));
    }

    // Frames to enrich, keyed (and deduped + time-ordered) by timestamp so a frame is OCR'd
    // once even when several deltas land on it. Start with the frame nearest each salient
    // delta, then add the keyframe-floor frames so colour-only / persistent scenes are covered.
    let mut keep: BTreeMap<u64, PathBuf> = BTreeMap::new();
    for d in &deltas {
        if let Some((t_ms, path)) = nearest_frame(frames, d.t_event) {
            keep.entry(*t_ms).or_insert_with(|| path.clone());
        }
    }
    let times: Vec<u64> = frames.iter().map(|(t, _)| *t).collect();
    for mark in floor_marks(&times, KEYFRAME_FLOOR_MS) {
        if let Some((t_ms, path)) = nearest_frame(frames, mark) {
            keep.entry(*t_ms).or_insert_with(|| path.clone());
        }
    }
    let salient_frames: Vec<SalientFrame> = keep
        .into_iter()
        .map(|(t_ms, path)| SalientFrame { t_ms, path, region: None })
        .collect();

    Ok(GateOutput { deltas, salient_frames })
}

/// The keyframe-floor sample marks across a clip whose frames occur at `times` (ms): `0`,
/// `floor_ms`, `2·floor_ms`, … through the last frame (always including the final frame so a
/// scene at the very end is covered). Pure, so the coverage guarantee is unit-tested.
fn floor_marks(times: &[u64], floor_ms: u64) -> Vec<u64> {
    let last = match times.last() {
        Some(&t) => t,
        None => return Vec::new(),
    };
    let step = floor_ms.max(1);
    let mut marks = Vec::new();
    let mut m = 0u64;
    loop {
        marks.push(m);
        if m >= last {
            break;
        }
        m = (m + step).min(last);
    }
    marks
}

/// Decode a PNG/JPEG to RGBA and downscale to the codec's cell grid.
fn load_cells(path: &Path, grid: (u8, u8)) -> Result<Vec<veyo_core::Cell>> {
    let img = image::open(path)?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok(rgba_to_cells(img.as_raw(), w, h, grid.0, grid.1))
}

fn nearest_frame(frames: &[(u64, PathBuf)], t: u64) -> Option<&(u64, PathBuf)> {
    frames.iter().min_by_key(|(ft, _)| ft.abs_diff(t))
}

#[cfg(test)]
mod tests {
    use super::floor_marks;

    #[test]
    fn floor_marks_cover_the_whole_clip_with_no_gap_over_floor() {
        // frames every 250ms across ~6.7s
        let times: Vec<u64> = (0..=26).map(|i| i * 250).collect(); // 0..6500
        let marks = floor_marks(&times, 2000);
        assert_eq!(*marks.first().unwrap(), 0, "first frame always kept");
        assert_eq!(*marks.last().unwrap(), 6500, "last frame always kept (end scene)");
        // no two consecutive marks are further apart than the floor
        for w in marks.windows(2) {
            assert!(w[1] - w[0] <= 2000, "gap {} exceeds floor", w[1] - w[0]);
        }
    }

    #[test]
    fn floor_marks_handle_short_and_empty_clips() {
        assert_eq!(floor_marks(&[], 2000), Vec::<u64>::new());
        assert_eq!(floor_marks(&[0], 2000), vec![0]);
        // a clip shorter than the floor still yields start + end
        assert_eq!(floor_marks(&[0, 500, 900], 2000), vec![0, 900]);
    }
}
