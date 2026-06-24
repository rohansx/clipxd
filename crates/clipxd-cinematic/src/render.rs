//! Pure render helpers — map a normalized [`ZoomKeyframe`] to a pixel crop rectangle the
//! renderer (ffmpeg / the `image` crate) applies frame by frame. No I/O here, so the engine
//! stays dependency-free and unit-testable.

use crate::types::ZoomKeyframe;

/// A pixel crop rectangle (top-left + size). Width/height are kept **even** (H.264/VP9
/// encoders reject odd dimensions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

/// The crop window for `kf` over a `width`×`height` source: the source divided by `scale`,
/// centered on `(cx, cy)`, clamped inside the frame, with even dimensions.
pub fn crop_rect(kf: &ZoomKeyframe, width: u32, height: u32) -> CropRect {
    let scale = kf.scale.max(1.0);
    let w = even(((width as f64 / scale).round() as u32).clamp(2, width));
    let h = even(((height as f64 / scale).round() as u32).clamp(2, height));
    let cx_px = kf.cx * width as f64;
    let cy_px = kf.cy * height as f64;
    let x = (cx_px - w as f64 / 2.0).round().clamp(0.0, (width - w) as f64) as u32;
    let y = (cy_px - h as f64 / 2.0).round().clamp(0.0, (height - h) as f64) as u32;
    CropRect { x, y, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kf(scale: f64, cx: f64, cy: f64) -> ZoomKeyframe {
        ZoomKeyframe { t: 0.0, scale, cx, cy }
    }

    #[test]
    fn unzoomed_is_the_full_frame() {
        let r = crop_rect(&kf(1.0, 0.5, 0.5), 1920, 1080);
        assert_eq!(r, CropRect { x: 0, y: 0, w: 1920, h: 1080 });
    }

    #[test]
    fn two_x_zoom_is_half_size_centered() {
        let r = crop_rect(&kf(2.0, 0.5, 0.5), 1920, 1080);
        assert_eq!((r.w, r.h), (960, 540));
        assert_eq!((r.x, r.y), (480, 270)); // centered
    }

    #[test]
    fn crop_stays_in_bounds_and_even() {
        for &(scale, cx, cy) in &[(2.0, 0.0, 0.0), (3.0, 1.0, 1.0), (1.5, 0.9, 0.1)] {
            let r = crop_rect(&kf(scale, cx, cy), 1280, 720);
            assert!(r.w % 2 == 0 && r.h % 2 == 0, "dims must be even");
            assert!(r.x + r.w <= 1280 && r.y + r.h <= 720, "crop out of bounds: {r:?}");
        }
    }
}
