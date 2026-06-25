//! `clipxd-recorder` — Phase 3: the recorder backbone.
//!
//! A recording is two things at once in clipxd:
//! 1. a **beautiful video** — the [`EventTrack`]'s cursor path drives the clean-room
//!    [cinematic](clipxd_cinematic) auto-zoom ([`cinematic_track`]);
//! 2. an **agent-queryable artifact** — the same clicks/keystrokes become index
//!    [`Event`](clipxd_index::Event)s ([`to_index_events`]), so the recording flows through
//!    the existing veyo gate → enrich → index pipeline and is *queryable*, not an opaque MP4.
//!
//! That second half is the moat: no other recorder makes the recording legible to an agent.
//! Capture comes through the [`CaptureSource`] trait (live scap, video file, or in-memory).

pub mod autofocus;
pub mod capture;
pub mod index_map;
pub mod pipeline;
pub mod redact;
pub mod source;
pub mod transcribe;
pub mod types;

pub use autofocus::focus_track_from_deltas;
pub use capture::{FramesDirCapture, LiveCapture};
pub use index_map::to_index_events;
pub use transcribe::transcribe;
pub use pipeline::{record_from_capture, record_from_video, RecordOutput};
pub use redact::{apply_redaction, blur_at, redaction_for, BlurRegion};
pub use source::{CaptureSource, InMemorySource};
pub use types::{Click, CursorSample, EventTrack, KeyPress, SourceInfo};

use clipxd_cinematic::{ZoomConfig, ZoomKeyframe};

/// Compute the cinematic auto-zoom track for a session — the cursor/click track in, a
/// smooth camera path out (one keyframe per frame at `cfg.fps`).
pub fn cinematic_track(track: &EventTrack, duration_s: f64, cfg: &ZoomConfig) -> Vec<ZoomKeyframe> {
    clipxd_cinematic::compute_zoom_track(&track.cursors, &track.clicks, duration_s, cfg)
}

/// Convenience: the cinematic zoom track at `fps` with default tuning. Used to recompute a
/// clip's zoom from a real cursor track (so the camera follows the cursor, not the content).
pub fn zoom_track(track: &EventTrack, duration_s: f64, fps: f64) -> Vec<ZoomKeyframe> {
    cinematic_track(track, duration_s, &ZoomConfig { fps, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_is_both_beautified_and_queryable() {
        let events = EventTrack {
            cursors: vec![CursorSample { t: 0.0, x: 0.2, y: 0.2 }, CursorSample { t: 3.0, x: 0.7, y: 0.6 }],
            clicks: vec![Click { t: 2.0, x: 0.7, y: 0.6 }],
            keys: vec![KeyPress { t: 2.5, key: "Enter".into() }],
        };
        let source = InMemorySource {
            info: SourceInfo { width: 1920, height: 1080, fps: 30.0, duration_s: 4.0 },
            events: events.clone(),
        };

        // beautified: the click produces a real zoom
        let track = cinematic_track(&source.event_track(), source.info().duration_s, &ZoomConfig::default());
        let peak = track.iter().map(|k| k.scale).fold(0.0_f64, f64::max);
        assert!((peak - 2.0).abs() < 1e-6, "session should auto-zoom on the click");

        // queryable: the click + key become index events
        let idx_events = to_index_events(&source.event_track());
        assert_eq!(idx_events.len(), 2);
        assert!(idx_events.iter().any(|e| e.kind == "click"));
        assert!(idx_events.iter().any(|e| e.kind == "key" && e.text.as_deref() == Some("Enter")));
    }
}
