//! Per-channel perceptual hashing for keyframe dedup.
//!
//! Screen recordings are overwhelmingly static, and the gate's keyframe floor keeps a frame
//! every 2s *regardless* of change — so on a quiet screen most kept frames are pixel-identical
//! and each one costs an OCR pass plus a caption call. Hashing lets the gate skip a floor
//! frame that looks the same as the last frame it kept.
//!
//! The hash is a dHash (horizontal gradient sign over a 9×8 downscale) computed **per RGB
//! channel**, not on luma: the floor exists precisely because veyo's salience is luma-based
//! and can miss colour-only changes (red→green at equal brightness) — a grayscale hash would
//! share that blindness and skip exactly the frames the floor is there to catch.

use anyhow::Result;
use std::path::Path;

/// dHash width+1 × height of the downscale grid; 8×8 comparisons → 64 bits per channel.
const W: u32 = 9;
const H: u32 = 8;

/// One 64-bit gradient hash per RGB channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHash(pub [u64; 3]);

impl FrameHash {
    /// Total Hamming distance across the three channel hashes (0..=192).
    pub fn distance(&self, other: &FrameHash) -> u32 {
        self.0
            .iter()
            .zip(other.0.iter())
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }
}

/// Hash the image at `path`. Decode cost dominates (the 9×8 resize is trivial); callers
/// should hash only frames they're already considering keeping.
pub fn hash_frame(path: &Path) -> Result<FrameHash> {
    let img = image::open(path)?.to_rgb8();
    let small = image::imageops::resize(&img, W, H, image::imageops::FilterType::Triangle);
    let mut hashes = [0u64; 3];
    for (c, h) in hashes.iter_mut().enumerate() {
        let mut bits = 0u64;
        let mut bit = 0u32;
        for y in 0..H {
            for x in 0..W - 1 {
                let a = small.get_pixel(x, y).0[c];
                let b = small.get_pixel(x + 1, y).0[c];
                if a > b {
                    bits |= 1u64 << bit;
                }
                bit += 1;
            }
        }
        *h = bits;
    }
    Ok(FrameHash(hashes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn write_png(path: &Path, color_left: [u8; 3], color_right: [u8; 3]) {
        let mut img = RgbImage::new(64, 64);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            *p = Rgb(if x < 32 { color_left } else { color_right });
        }
        img.save(path).unwrap();
    }

    #[test]
    fn identical_frames_hash_identically() {
        let tmp = std::env::temp_dir().join(format!("clipxd-phash-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let (a, b) = (tmp.join("a.png"), tmp.join("b.png"));
        write_png(&a, [200, 30, 30], [30, 30, 200]);
        write_png(&b, [200, 30, 30], [30, 30, 200]);
        assert_eq!(hash_frame(&a).unwrap().distance(&hash_frame(&b).unwrap()), 0);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn colour_only_change_is_visible_to_the_hash() {
        // Two colours chosen for near-equal luma — the case the keyframe floor exists for.
        let tmp = std::env::temp_dir().join(format!("clipxd-phash-c-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let (a, b) = (tmp.join("a.png"), tmp.join("b.png"));
        write_png(&a, [200, 60, 60], [60, 60, 60]);
        write_png(&b, [60, 200, 60], [60, 60, 60]); // red block → green block
        let d = hash_frame(&a).unwrap().distance(&hash_frame(&b).unwrap());
        assert!(d > 8, "colour-only change should flip many channel bits, got {d}");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
