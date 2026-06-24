//! The veyo-core salience gate over decoded frames.
//!
//! This is where clipxd consumes the codec: each frame is downscaled to cells and fed to
//! [`Codec::observe`](veyo_core::Codec::observe); the codec decides which moments are
//! salient and emits [`Delta`](veyo_core::Delta)s. clipxd retains the salient frames (the
//! codec discards pixels) so they can be OCR'd and captioned by veyo-enrich.

use crate::downscale::rgba_to_cells;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use veyo_core::{Codec, CodecConfig, Delta, Frame, SurfaceRef};
use veyo_enrich::SalientFrame;

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

    // For each salient delta, keep the frame nearest in time. Dedup by path so a frame is
    // only OCR'd once even when several deltas land on it.
    let mut salient_frames: Vec<SalientFrame> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for d in &deltas {
        if let Some((t_ms, path)) = nearest_frame(frames, d.t_event) {
            if !seen.contains(path) {
                seen.push(path.clone());
                salient_frames.push(SalientFrame {
                    t_ms: *t_ms,
                    path: path.clone(),
                    region: None, // OCR the whole frame; the delta carries the region
                });
            }
        }
    }

    Ok(GateOutput { deltas, salient_frames })
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
