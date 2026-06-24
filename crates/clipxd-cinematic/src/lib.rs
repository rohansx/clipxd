//! `clipxd-cinematic` — Phase 3: the clean-room cinematic **auto-zoom** engine.
//!
//! Given a cursor/click event track (the kind `clipxd-recorder` will emit), it produces a
//! smooth camera path — a [`ZoomKeyframe`] per frame — that zooms toward clicks, follows the
//! cursor while held, and eases back out. The renderer turns each keyframe into a pixel
//! [`crop_rect`] and crop+scales the video.
//!
//! **Clean-room (legally load-bearing).** This is built from uncopyrightable math — Penner
//! easing (`ease_out_quart`/`ease_in_out_cubic`), `lerp`, and an exponential moving average
//! for anti-jitter — *not* from Cap (AGPL) or OpenVid (Non-Commercial) source, neither of
//! which may be copied into clipxd's Apache-2.0 + closed-tier codebase. See
//! `docs/phase3-recorder-plan.md`.
//!
//! ```
//! use clipxd_cinematic::{compute_zoom_track, crop_rect, Click, ZoomConfig};
//! let clicks = [Click { t: 2.0, x: 0.7, y: 0.4 }];
//! let track = compute_zoom_track(&[], &clicks, 4.0, &ZoomConfig::default());
//! let peak = track.iter().map(|k| k.scale).fold(0.0_f64, f64::max);
//! assert!((peak - 2.0).abs() < 1e-6);                // zooms to the configured 2×
//! let held = track.iter().find(|k| (k.t - 2.4).abs() < 0.02).unwrap();
//! let _rect = crop_rect(held, 1920, 1080);           // → the pixel window to crop
//! ```

pub mod easing;
pub mod render;
pub mod types;
pub mod zoom;

pub use render::{crop_rect, CropRect};
pub use types::{Click, CursorSample, ZoomConfig, ZoomKeyframe};
pub use zoom::compute_zoom_track;
