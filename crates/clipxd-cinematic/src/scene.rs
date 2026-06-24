//! The "produced look" layout — background + padding + rounded corners + drop shadow. Pure
//! geometry: given the source size and a scene config, where does the (beautified) video sit
//! on the output canvas, and what radius/shadow to draw. The renderer paints the background
//! first, then the video into `content`.
//!
//! Clean-room: the feature *baselines* (896px reference, padding/radius/shadow scaling) are
//! uncopyrightable constants recorded from observable behavior, re-implemented here
//! (`docs/recorder-feature-catalog.md` §E).

use serde::{Deserialize, Serialize};

/// A background fill for the canvas behind the video.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Background {
    Solid(String),
    /// `angle` degrees + CSS-style color stops.
    Linear { angle: f64, stops: Vec<String> },
    Image(String),
}

/// Scene knobs (slider values 0..100) + output canvas size.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneConfig {
    pub background: Background,
    pub padding: f64,
    pub corners: f64,
    pub shadow: f64,
    pub out_w: u32,
    pub out_h: u32,
}

impl Default for SceneConfig {
    fn default() -> Self {
        Self {
            background: Background::Linear { angle: 135.0, stops: vec!["#1f6feb".into(), "#0d1117".into()] },
            padding: 6.0,
            corners: 14.0,
            shadow: 40.0,
            out_w: 1920,
            out_h: 1080,
        }
    }
}

/// Where the video sits on the canvas + how to draw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameLayout {
    pub content_x: u32,
    pub content_y: u32,
    pub content_w: u32,
    pub content_h: u32,
    pub corner_radius: u32,
    pub shadow_blur: u32,
}

const BASELINE: f64 = 896.0;

/// Lay out a `src_w`×`src_h` video onto the scene's output canvas: padded, centered,
/// aspect-preserved (contain), with radius/shadow scaled to the content width.
pub fn frame_layout(src_w: u32, src_h: u32, cfg: &SceneConfig) -> FrameLayout {
    let pad = ((cfg.padding * 0.5 / 100.0) * cfg.out_w as f64).round() as u32;
    let avail_w = cfg.out_w.saturating_sub(2 * pad).max(2);
    let avail_h = cfg.out_h.saturating_sub(2 * pad).max(2);

    let s = (avail_w as f64 / src_w.max(1) as f64).min(avail_h as f64 / src_h.max(1) as f64);
    let cw = ((src_w as f64 * s).round() as u32).clamp(2, cfg.out_w);
    let ch = ((src_h as f64 * s).round() as u32).clamp(2, cfg.out_h);
    let cx = (cfg.out_w - cw) / 2;
    let cy = (cfg.out_h - ch) / 2;

    FrameLayout {
        content_x: cx,
        content_y: cy,
        content_w: cw,
        content_h: ch,
        corner_radius: (cfg.corners * (cw as f64 / BASELINE)).round() as u32,
        shadow_blur: (cfg.shadow * (cw as f64 / BASELINE) * 0.3).round() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(out_w: u32, out_h: u32, padding: f64) -> SceneConfig {
        SceneConfig { padding, out_w, out_h, ..Default::default() }
    }

    #[test]
    fn padding_shrinks_and_centers_the_content() {
        let l = frame_layout(1920, 1080, &cfg(1920, 1080, 10.0));
        assert!(l.content_w < 1920, "padding should shrink width");
        // centered
        assert_eq!(l.content_x, (1920 - l.content_w) / 2);
        assert_eq!(l.content_y, (1080 - l.content_h) / 2);
    }

    #[test]
    fn aspect_ratio_is_preserved() {
        let l = frame_layout(1600, 900, &cfg(1920, 1080, 8.0));
        let src = 1600.0 / 900.0;
        let got = l.content_w as f64 / l.content_h as f64;
        assert!((src - got).abs() < 0.02, "aspect drift: {src} vs {got}");
    }

    #[test]
    fn zero_padding_fills_a_matching_canvas() {
        let l = frame_layout(1920, 1080, &cfg(1920, 1080, 0.0));
        assert_eq!((l.content_w, l.content_h), (1920, 1080));
    }
}
