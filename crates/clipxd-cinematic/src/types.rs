//! Cinematic engine I/O types. All spatial coordinates are **normalized `0..1`** (origin
//! top-left), so the engine is resolution- and aspect-independent; the renderer maps them
//! back to pixels.

use serde::{Deserialize, Serialize};

/// A cursor position sample at time `t` (seconds).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CursorSample {
    pub t: f64,
    pub x: f64,
    pub y: f64,
}

/// A click at time `t` (seconds) and position — the gravity well the camera zooms toward.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Click {
    pub t: f64,
    pub x: f64,
    pub y: f64,
}

/// One frame of the camera path: a `scale` (≥ 1.0; 1.0 = full frame) centered on `(cx, cy)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoomKeyframe {
    pub t: f64,
    pub scale: f64,
    pub cx: f64,
    pub cy: f64,
}

/// Tunables for the auto-zoom. Defaults are demo-grade.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ZoomConfig {
    /// Sampling rate of the produced keyframe track.
    pub fps: f64,
    /// Target zoom scale during a hold (e.g. 2.0 = 2×).
    pub zoom: f64,
    /// Ease-in / ease-out duration (seconds).
    pub transition_s: f64,
    /// Begin zooming this long *before* a click (anticipation).
    pub pre_click_s: f64,
    /// Hold this long *after* the last click in a segment.
    pub post_click_s: f64,
    /// Clicks closer than this merge into one zoom segment.
    pub merge_gap_s: f64,
    /// EMA smoothing factor for cursor-follow (`0..1`; lower = smoother / more anti-jitter).
    /// Used when [`spring`](Self::spring) is `None`.
    pub smoothing: f64,
    /// Optional critically-damped spring stiffness (natural frequency ω). When set, the
    /// camera follows the cursor with a spring instead of an EMA — velocity-continuous, no
    /// wobble on reversals. Typical: `12.0`–`24.0`. `None` = use EMA.
    pub spring: Option<f64>,
}

impl Default for ZoomConfig {
    fn default() -> Self {
        Self {
            fps: 30.0,
            zoom: 2.0,
            transition_s: 0.5,
            pre_click_s: 0.4,
            post_click_s: 1.2,
            merge_gap_s: 1.5,
            smoothing: 0.18,
            spring: None,
        }
    }
}
