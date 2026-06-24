//! Live-capture abstraction (Phase 3.3 / 3.4).
//!
//! A capture backend produces the *same two things* the recorder needs — a sequence of
//! frames and an [`EventTrack`] — so live recording flows through the rest of the pipeline
//! identically to a video file. Real backends are platform-specific and feature-gated:
//!
//! - **`scap`** (MIT) on macOS (ScreenCaptureKit) / Windows (D3D11) — the premium path,
//!   added behind a `capture-scap` feature when building on that hardware.
//! - **clean-room PipeWire** (xdg-desktop-portal `ScreenCast`) on Linux/Wayland — needs
//!   `libpipewire` dev headers; **not buildable on this box** (PipeWire 1.6.7, no headers),
//!   so it's written and tested where those exist.
//!
//! [`FramesDirCapture`] is the **portable stand-in**: it streams a directory of frames
//! (what ffmpeg, or a real backend, writes to disk), so the *entire* recorder pipeline is
//! exercised end-to-end on any platform — including here — without a capture device.

use crate::types::{EventTrack, SourceInfo};
use std::path::{Path, PathBuf};

/// A live source of capture frames + the interaction event track. Whatever produces these
/// (scap, PipeWire, or a frames dir), the recorder treats them the same.
pub trait LiveCapture {
    fn info(&self) -> SourceInfo;
    /// Frames in time order, each `(t_ms, path)`.
    fn frames(&self) -> Vec<(u64, PathBuf)>;
    fn events(&self) -> EventTrack;
}

/// Streams a directory of PNG frames at a fixed fps — the portable capture stand-in.
pub struct FramesDirCapture {
    info: SourceInfo,
    frames_dir: PathBuf,
    events: EventTrack,
}

impl FramesDirCapture {
    /// Open a frames directory; duration is derived from the frame count / `fps`.
    pub fn open(frames_dir: impl Into<PathBuf>, fps: f64, width: u32, height: u32, events: EventTrack) -> Self {
        let frames_dir = frames_dir.into();
        let n = list_pngs(&frames_dir).len();
        let info = SourceInfo { width, height, fps, duration_s: n as f64 / fps.max(1.0) };
        Self { info, frames_dir, events }
    }
}

impl LiveCapture for FramesDirCapture {
    fn info(&self) -> SourceInfo {
        self.info
    }
    fn frames(&self) -> Vec<(u64, PathBuf)> {
        let fps = self.info.fps.max(1.0);
        list_pngs(&self.frames_dir)
            .into_iter()
            .enumerate()
            .map(|(i, p)| (((i as f64 / fps) * 1000.0).round() as u64, p))
            .collect()
    }
    fn events(&self) -> EventTrack {
        self.events.clone()
    }
}

fn list_pngs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_dir_capture_streams_in_order_with_timestamps() {
        let dir = std::env::temp_dir().join(format!("clipxd-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["00003.png", "00001.png", "00002.png"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let cap = FramesDirCapture::open(&dir, 10.0, 1920, 1080, EventTrack::default());
        let frames = cap.frames();
        assert_eq!(frames.len(), 3);
        assert!(frames[0].1.ends_with("00001.png"), "sorted by name");
        assert_eq!(frames[0].0, 0);
        assert_eq!(frames[1].0, 100); // 1/10s
        assert!((cap.info().duration_s - 0.3).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
