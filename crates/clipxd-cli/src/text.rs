//! Minimal text rasterizer for the cinematic overlay (keystroke pills). Loads the system
//! sans-bold font at runtime via `fc-match` (no bundled font → no font-licensing question),
//! measures a string, and alpha-blends glyph coverage onto an RGBA canvas. `ab_glyph` does
//! the outline rasterization.

use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};

/// Load the system sans-bold font (via `fc-match`). `None` if unavailable — callers skip
/// text rather than fail the render.
pub fn load_font() -> Option<FontVec> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", "sans:bold"]).output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return None;
    }
    FontVec::try_from_vec(std::fs::read(path).ok()?).ok()
}

/// Advance width of `s` at pixel size `px`.
pub fn text_width(font: &FontVec, px: f32, s: &str) -> f32 {
    let sf = font.as_scaled(PxScale::from(px));
    s.chars().map(|c| sf.h_advance(sf.scaled_glyph(c).id)).sum()
}

/// Alpha-blend `s` onto `img` with its top-left at `(x, y)`.
pub fn draw_text(img: &mut RgbaImage, x: f32, y: f32, s: &str, px: f32, font: &FontVec, color: [u8; 3]) {
    let sf = font.as_scaled(PxScale::from(px));
    let ascent = sf.ascent();
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let mut caret = x;
    for c in s.chars() {
        let mut g = sf.scaled_glyph(c);
        let id = g.id;
        g.position = point(caret, y + ascent);
        if let Some(o) = font.outline_glyph(g) {
            let bb = o.px_bounds();
            o.draw(|gx, gy, cov| {
                let (px_, py_) = (bb.min.x as i32 + gx as i32, bb.min.y as i32 + gy as i32);
                if px_ >= 0 && py_ >= 0 && px_ < iw && py_ < ih {
                    let a = cov.clamp(0.0, 1.0);
                    let bg = img.get_pixel(px_ as u32, py_ as u32).0;
                    let mix = |f: u8, b: u8| (f as f32 * a + b as f32 * (1.0 - a)).round() as u8;
                    img.put_pixel(px_ as u32, py_ as u32, Rgba([mix(color[0], bg[0]), mix(color[1], bg[1]), mix(color[2], bg[2]), 255]));
                }
            });
        }
        caret += sf.h_advance(id);
    }
}
