//! A **critically-damped spring** — velocity-continuous cursor-follow that converges to the
//! target without overshoot or wobble (smoother than a plain EMA, especially on direction
//! reversals), plus a **dead-zone** so tiny cursor jitter doesn't move the camera at all.
//!
//! Clean-room: this is the standard damped-harmonic-oscillator (ζ = 1) analytic update —
//! uncopyrightable physics — re-derived here, not taken from any tool's source
//! (`docs/recorder-feature-catalog.md` §B).

/// A 1-D spring with position + velocity. Drive it toward a target each frame.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub pos: f64,
    pub vel: f64,
}

impl Spring {
    pub fn new(pos: f64) -> Self {
        Self { pos, vel: 0.0 }
    }

    /// Advance toward `target` by `dt` seconds at natural frequency `omega` (stiffness),
    /// critically damped. Stable for any `dt` (closed-form, not Euler integration).
    pub fn step(&mut self, target: f64, dt: f64, omega: f64) {
        let omega = omega.max(0.0);
        let d = self.pos - target; // displacement from target
        let c2 = self.vel + omega * d;
        let e = (-omega * dt).exp();
        let x = (d + c2 * dt) * e; // position relative to target at time dt
        self.pos = target + x;
        self.vel = (c2 - omega * (d + c2 * dt)) * e;
    }
}

/// Dead-zone: hold `anchor` until the target moves more than `radius` away — kills sub-pixel
/// chase on a near-still cursor.
pub fn dead_zone(target: f64, anchor: f64, radius: f64) -> f64 {
    if (target - anchor).abs() <= radius {
        anchor
    } else {
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_to_target_without_overshoot() {
        let mut s = Spring::new(0.0);
        let mut max = 0.0_f64;
        for _ in 0..120 {
            s.step(1.0, 1.0 / 60.0, 18.0);
            max = max.max(s.pos);
        }
        assert!((s.pos - 1.0).abs() < 1e-3, "should settle on target, got {}", s.pos);
        assert!(max <= 1.0 + 1e-6, "critically damped must not overshoot, peaked {max}");
    }

    #[test]
    fn dead_zone_ignores_small_moves() {
        assert_eq!(dead_zone(0.503, 0.5, 0.01), 0.5); // within → held
        assert_eq!(dead_zone(0.7, 0.5, 0.01), 0.7); // beyond → moves
    }
}
