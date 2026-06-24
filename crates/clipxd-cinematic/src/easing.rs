//! Easing + interpolation primitives.
//!
//! These are decades-old, uncopyrightable animation/signal-processing formulas (Penner
//! easing equations, linear interpolation, exponential moving average). They are
//! re-derived here from the math, not copied from any tool's source — the clean-room basis
//! for the cinematic layer (see `docs/phase3-recorder-plan.md` §4).

/// Clamp to `[0, 1]`.
pub fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// Linear interpolation: `a` at `t=0`, `b` at `t=1`.
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Quartic ease-out — fast start, gentle settle. `1 - (1-t)^4`.
pub fn ease_out_quart(t: f64) -> f64 {
    let t = clamp01(t);
    1.0 - (1.0 - t).powi(4)
}

/// Cubic ease-in-out — smooth acceleration and deceleration.
pub fn ease_in_out_cubic(t: f64) -> f64 {
    let t = clamp01(t);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// One exponential-moving-average step: `alpha*x + (1-alpha)*prev`. Lower `alpha` = more
/// smoothing (the anti-jitter knob for cursor follow).
pub fn ema(prev: f64, x: f64, alpha: f64) -> f64 {
    let a = clamp01(alpha);
    a * x + (1.0 - a) * prev
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_endpoints_and_monotonicity() {
        for f in [ease_out_quart as fn(f64) -> f64, ease_in_out_cubic] {
            assert!((f(0.0) - 0.0).abs() < 1e-9);
            assert!((f(1.0) - 1.0).abs() < 1e-9);
            // monotonic non-decreasing on [0,1]
            let mut prev = -1.0;
            for i in 0..=20 {
                let v = f(i as f64 / 20.0);
                assert!(v >= prev - 1e-9, "not monotonic at {i}");
                prev = v;
            }
        }
        // clamps outside [0,1]
        assert_eq!(ease_out_quart(-1.0), 0.0);
        assert_eq!(ease_out_quart(2.0), 1.0);
    }

    #[test]
    fn lerp_and_ema_behaviour() {
        assert_eq!(lerp(10.0, 20.0, 0.5), 15.0);
        // EMA with alpha=1 follows instantly; alpha=0 never moves
        assert_eq!(ema(0.0, 100.0, 1.0), 100.0);
        assert_eq!(ema(0.0, 100.0, 0.0), 0.0);
        // small alpha lags toward the target
        let mut v = 0.0;
        for _ in 0..50 {
            v = ema(v, 100.0, 0.2);
        }
        assert!(v > 99.0, "EMA should converge, got {v}");
    }
}
