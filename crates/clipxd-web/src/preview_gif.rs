//! Animated GIF preview generation — the "waving GIF in an email" distribution primitive
//! (Loom's highest-converting share surface, per research: a hyperlinked animated thumbnail
//! that plays inline in any email client, unlike `og:image` which every unfurl consumer
//! renders as a static frame regardless of the source format).
//!
//! Built from already-extracted salient frames (no new video decode) — up to
//! [`MAX_FRAMES`] evenly spaced across the clip's `visual_timeline`, palette-optimized by
//! ffmpeg's two-pass GIF filter for a reasonable file size. Generated once per clip, cached
//! to storage at `<id>/preview.gif` (same "generate on first request, serve the cached copy
//! after" shape as everything else derived-and-immutable in this codebase — zoom.json, frames).

use crate::storage;
use anyhow::{bail, Context, Result};
use clipxd_index::Index;
use std::path::Path;

/// Frames beyond this are downsampled evenly — enough to read as a preview without producing
/// a multi-megabyte GIF (email clients and Slack both have practical size ceilings).
const MAX_FRAMES: usize = 8;
/// How long each frame holds, in the output GIF.
const FRAME_DELAY_S: f64 = 0.9;

/// Generate a preview GIF for `idx` by fetching its salient frames through `storage` and
/// running ffmpeg locally. Returns the GIF bytes; the caller decides whether/where to cache
/// them (kept storage-agnostic here — this function only reads frame bytes, doesn't assume
/// local disk).
pub async fn generate(storage: &dyn storage::Storage, id: &str, idx: &Index) -> Result<Vec<u8>> {
    let frame_refs = pick_frames(idx);
    if frame_refs.is_empty() {
        bail!("clip has no salient frames yet to build a preview from");
    }

    let tmp = std::env::temp_dir().join(format!("clipxd-preview-gif-{id}-{}", std::process::id()));
    tokio::fs::create_dir_all(&tmp).await.context("scratch dir")?;
    let cleanup = ScratchDir(tmp.clone());

    for (i, frame_ref) in frame_refs.iter().enumerate() {
        let bytes = storage
            .read_object(&format!("{id}/{frame_ref}"))
            .await
            .with_context(|| format!("reading {frame_ref}"))?
            .ok_or_else(|| anyhow::anyhow!("frame {frame_ref} missing from storage"))?;
        let ext = Path::new(frame_ref).extension().and_then(|e| e.to_str()).unwrap_or("jpg");
        tokio::fs::write(tmp.join(format!("f{i:03}.{ext}")), bytes).await?;
    }

    let out = tmp.join("preview.gif");
    let pattern = tmp.join(format!("f%03d.{}", Path::new(&frame_refs[0]).extension().and_then(|e| e.to_str()).unwrap_or("jpg")));
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-y", "-framerate", &(1.0 / FRAME_DELAY_S).to_string(), "-i"])
        .arg(&pattern)
        // split into a palette stream (better GIF color quality) + the frames, then combine —
        // the standard ffmpeg palette-based GIF recipe.
        .args(["-vf", "scale=480:-1:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse", "-loop", "0"])
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .context("running ffmpeg")?;
    anyhow::ensure!(status.success(), "ffmpeg GIF encode failed");

    let gif = tokio::fs::read(&out).await.context("reading generated gif")?;
    drop(cleanup);
    Ok(gif)
}

/// Evenly-spaced pick of up to `MAX_FRAMES` salient frames that actually have a `frame_ref`
/// (some moments — e.g. keyframe-floor entries the codec chose not to retain a frame for —
/// don't). Preserves timeline order so the GIF plays forward, not shuffled.
fn pick_frames(idx: &Index) -> Vec<String> {
    let refs: Vec<&str> = idx.visual_timeline.iter().filter_map(|m| m.frame_ref.as_deref()).collect();
    if refs.len() <= MAX_FRAMES {
        return refs.into_iter().map(String::from).collect();
    }
    let step = refs.len() as f64 / MAX_FRAMES as f64;
    (0..MAX_FRAMES).map(|i| refs[((i as f64 * step) as usize).min(refs.len() - 1)].to_string()).collect()
}

/// RAII best-effort cleanup of the scratch dir — GIF generation failing shouldn't leak temp
/// frame copies onto disk.
struct ScratchDir(std::path::PathBuf);
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clipxd_index::VisualMoment;

    fn moment(t: f64, frame_ref: Option<&str>) -> VisualMoment {
        VisualMoment { t, salience: 1.0, caption: String::new(), delta: "d".into(), frame_ref: frame_ref.map(String::from), label: None }
    }

    fn idx_with(moments: Vec<VisualMoment>) -> Index {
        use clipxd_index::{Metadata, Source};
        let mut idx = Index::new(
            "clp_1",
            Source::Screen,
            Metadata { duration: 10.0, resolution: [100, 100], fps: 30.0, created_at: "0".into(), title: "t".into(), description: String::new(), app_focus: vec![], url_context: None, has_video: true },
        );
        idx.visual_timeline = moments;
        idx
    }

    #[test]
    fn pick_frames_skips_moments_with_no_frame_ref() {
        let idx = idx_with(vec![moment(0.0, Some("frames/a.jpg")), moment(1.0, None), moment(2.0, Some("frames/b.jpg"))]);
        assert_eq!(pick_frames(&idx), vec!["frames/a.jpg", "frames/b.jpg"]);
    }

    #[test]
    fn pick_frames_keeps_all_when_under_the_cap() {
        let moments: Vec<_> = (0..5).map(|i| moment(i as f64, Some("frames/x.jpg"))).collect();
        assert_eq!(pick_frames(&idx_with(moments)).len(), 5);
    }

    #[test]
    fn pick_frames_downsamples_evenly_and_preserves_order() {
        let moments: Vec<_> = (0..20).map(|i| moment(i as f64, Some(Box::leak(format!("frames/{i:02}.jpg").into_boxed_str())))).collect();
        let picked = pick_frames(&idx_with(moments));
        assert_eq!(picked.len(), MAX_FRAMES);
        // strictly increasing frame numbers -> timeline order preserved, not shuffled
        let nums: Vec<i32> = picked.iter().map(|p| p.trim_start_matches("frames/").trim_end_matches(".jpg").parse().unwrap()).collect();
        assert!(nums.windows(2).all(|w| w[0] < w[1]), "{nums:?} should be strictly increasing");
    }

    #[test]
    fn pick_frames_empty_when_no_frame_refs_exist() {
        let idx = idx_with(vec![moment(0.0, None), moment(1.0, None)]);
        assert!(pick_frames(&idx).is_empty());
    }
}
