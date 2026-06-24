//! The clean-room cinematic auto-zoom engine.
//!
//! Turns a cursor/click event track into a smooth camera path (a [`ZoomKeyframe`] per frame):
//!
//! 1. **Segment** clicks into zoom episodes (merge nearby clicks; pad before/after).
//! 2. Per episode, run a **3-phase** camera: *ease-in* (zoom 1→Z toward the click),
//!    *hold* (stay zoomed, follow the cursor), *ease-out* (zoom Z→1, recenter).
//! 3. **EMA-smooth** the camera center (anti-jitter) and **clamp** it so the zoomed crop
//!    window never leaves the frame.
//!
//! All formulas live in [`crate::easing`] and are uncopyrightable math — no Cap/OpenVid
//! source was read (see `docs/phase3-recorder-plan.md` §4).

use crate::easing::{clamp01, ease_out_quart, ema, lerp};
use crate::types::{Click, CursorSample, ZoomConfig, ZoomKeyframe};

struct Segment {
    start: f64,
    end: f64,
    /// Anchor (the first click of the episode) — where the ease-in zooms toward.
    ax: f64,
    ay: f64,
}

/// Compute the per-frame zoom track for a clip of `duration_s`.
pub fn compute_zoom_track(
    cursors: &[CursorSample],
    clicks: &[Click],
    duration_s: f64,
    cfg: &ZoomConfig,
) -> Vec<ZoomKeyframe> {
    let segments = build_segments(clicks, cfg, duration_s);
    let fps = cfg.fps.max(1.0);
    let n = (duration_s * fps).floor() as usize + 1;
    let tr = cfg.transition_s.max(1e-3);

    let mut out = Vec::with_capacity(n);
    let (mut sx, mut sy) = (0.5, 0.5); // running smoothed center
    for i in 0..n {
        let t = i as f64 / fps;
        let active = segments.iter().find(|s| t >= s.start && t <= s.end);

        let (scale, tx, ty) = match active {
            None => (1.0, 0.5, 0.5),
            Some(s) => {
                if t < s.start + tr {
                    // ease-in: zoom up, glide the target from frame-center toward the click
                    let e = ease_out_quart((t - s.start) / tr);
                    (lerp(1.0, cfg.zoom, e), lerp(0.5, s.ax, e), lerp(0.5, s.ay, e))
                } else if t > s.end - tr {
                    // ease-out: zoom back to full frame, recenter
                    let e = ease_out_quart((t - (s.end - tr)) / tr);
                    (lerp(cfg.zoom, 1.0, e), 0.5, 0.5)
                } else {
                    // hold: stay zoomed, follow the (smoothed) cursor
                    let (cxp, cyp) = cursor_at(cursors, t).unwrap_or((s.ax, s.ay));
                    (cfg.zoom, clamp01(cxp), clamp01(cyp))
                }
            }
        };

        sx = ema(sx, tx, cfg.smoothing);
        sy = ema(sy, ty, cfg.smoothing);

        // keep the zoomed crop window inside the frame: at `scale`, the half-extent is
        // `0.5/scale`, so the center is bounded to `[half, 1-half]`.
        let half = 0.5 / scale;
        out.push(ZoomKeyframe {
            t,
            scale,
            cx: sx.clamp(half, 1.0 - half),
            cy: sy.clamp(half, 1.0 - half),
        });
    }
    out
}

/// Group clicks into padded, non-overlapping zoom episodes.
fn build_segments(clicks: &[Click], cfg: &ZoomConfig, duration: f64) -> Vec<Segment> {
    if clicks.is_empty() {
        return Vec::new();
    }
    let mut cs = clicks.to_vec();
    cs.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));

    let seg_of = |first: &Click, last: &Click| Segment {
        start: (first.t - cfg.pre_click_s).max(0.0),
        end: (last.t + cfg.post_click_s).min(duration),
        ax: clamp01(first.x),
        ay: clamp01(first.y),
    };

    // merge clicks within `merge_gap_s`
    let mut raw: Vec<Segment> = Vec::new();
    let mut first = cs[0];
    let mut last = cs[0];
    for c in cs.iter().skip(1) {
        if c.t - last.t <= cfg.merge_gap_s {
            last = *c;
        } else {
            raw.push(seg_of(&first, &last));
            first = *c;
            last = *c;
        }
    }
    raw.push(seg_of(&first, &last));

    // coalesce intervals that overlap after padding (keep the earlier anchor)
    raw.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<Segment> = Vec::new();
    for s in raw {
        match out.last_mut() {
            Some(prev) if s.start <= prev.end => prev.end = prev.end.max(s.end),
            _ => out.push(s),
        }
    }
    out
}

/// Linearly-interpolated cursor position at time `t` (clamped to the ends).
fn cursor_at(cursors: &[CursorSample], t: f64) -> Option<(f64, f64)> {
    let first = cursors.first()?;
    if t <= first.t {
        return Some((first.x, first.y));
    }
    let lastc = cursors.last().unwrap();
    if t >= lastc.t {
        return Some((lastc.x, lastc.y));
    }
    for w in cursors.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t >= a.t && t <= b.t {
            let f = if b.t > a.t { (t - a.t) / (b.t - a.t) } else { 0.0 };
            return Some((lerp(a.x, b.x, f), lerp(a.y, b.y, f)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ZoomConfig {
        ZoomConfig { fps: 30.0, ..Default::default() }
    }

    #[test]
    fn no_clicks_means_no_zoom() {
        let track = compute_zoom_track(&[], &[], 3.0, &cfg());
        assert!(!track.is_empty());
        assert!(track.iter().all(|k| (k.scale - 1.0).abs() < 1e-9 && (k.cx - 0.5).abs() < 1e-9));
    }

    #[test]
    fn a_click_zooms_in_then_back_out() {
        let clicks = [Click { t: 2.0, x: 0.8, y: 0.3 }];
        let track = compute_zoom_track(&[], &clicks, 4.0, &cfg());

        let at = |t: f64| *track.iter().min_by(|a, b| (a.t - t).abs().partial_cmp(&(b.t - t).abs()).unwrap()).unwrap();
        // click @2.0 → segment [1.6, 3.2], transition 0.5: ease-in [1.6,2.1], HOLD [2.1,2.7], ease-out [2.7,3.2]
        assert!((at(0.0).scale - 1.0).abs() < 1e-6, "starts un-zoomed");
        assert!((at(2.4).scale - 2.0).abs() < 1e-6, "fully zoomed during the hold");
        assert!((at(3.9).scale - 1.0).abs() < 0.05, "returns to full frame by the end");
        let max_scale = track.iter().map(|k| k.scale).fold(0.0_f64, f64::max);
        assert!((max_scale - 2.0).abs() < 1e-6, "peak zoom is the configured 2×, got {max_scale}");

        // during the hold the camera pans toward the click (x=0.8 → clamped to ~0.75 at 2×)
        assert!(at(2.4).cx > 0.6, "should pan toward the click x, got {}", at(2.4).cx);
    }

    #[test]
    fn crop_window_never_leaves_the_frame() {
        let clicks = [Click { t: 1.0, x: 0.99, y: 0.01 }, Click { t: 5.0, x: 0.02, y: 0.97 }];
        let cursors: Vec<CursorSample> = (0..120)
            .map(|i| CursorSample { t: i as f64 / 20.0, x: (i % 10) as f64 / 10.0, y: 0.5 })
            .collect();
        let track = compute_zoom_track(&cursors, &clicks, 6.0, &cfg());
        for k in &track {
            let half = 0.5 / k.scale;
            assert!(k.cx >= half - 1e-9 && k.cx <= 1.0 - half + 1e-9, "cx {} out of frame at scale {}", k.cx, k.scale);
            assert!(k.cy >= half - 1e-9 && k.cy <= 1.0 - half + 1e-9, "cy {} out of frame", k.cy);
            assert!(k.scale >= 1.0 - 1e-9 && k.scale <= cfg().zoom + 1e-9);
        }
    }

    #[test]
    fn nearby_clicks_merge_into_one_episode() {
        // two clicks 0.3s apart should not produce two separate zoom-out/zoom-in cycles
        let clicks = [Click { t: 2.0, x: 0.5, y: 0.5 }, Click { t: 2.3, x: 0.5, y: 0.5 }];
        let track = compute_zoom_track(&[], &clicks, 5.0, &cfg());
        // count contiguous zoomed spans (scale > 1.05)
        let mut episodes = 0;
        let mut inside = false;
        for k in &track {
            let z = k.scale > 1.05;
            if z && !inside {
                episodes += 1;
            }
            inside = z;
        }
        assert_eq!(episodes, 1, "nearby clicks should be one episode");
    }
}
