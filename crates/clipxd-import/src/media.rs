//! Media I/O: fetch (yt-dlp for URLs), probe (ffprobe), and demux (ffmpeg) — the only
//! places clipxd-import shells out. Everything downstream is pure Rust.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Probed media facts.
#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub duration_s: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
}

/// True for `http://` / `https://` inputs (vs a local file path).
pub fn is_url(input: &str) -> bool {
    input.starts_with("http://") || input.starts_with("https://")
}

/// Resolve `input` to a local video file. URLs are downloaded with `yt-dlp` into
/// `work_dir`; local paths are returned as-is.
pub fn fetch(input: &str, work_dir: &Path) -> Result<PathBuf> {
    if !is_url(input) {
        let p = PathBuf::from(input);
        anyhow::ensure!(p.exists(), "input file does not exist: {}", p.display());
        return Ok(p);
    }
    let out_tmpl = work_dir.join("source.%(ext)s");
    let status = Command::new("yt-dlp")
        .arg("-f")
        .arg("mp4/best")
        .arg("-o")
        .arg(&out_tmpl)
        .arg(input)
        .status()
        .context("failed to run yt-dlp (is it installed?)")?;
    anyhow::ensure!(status.success(), "yt-dlp failed for {input}");
    // find whatever it wrote (source.*)
    for entry in std::fs::read_dir(work_dir)? {
        let p = entry?.path();
        if p.file_stem().and_then(|s| s.to_str()) == Some("source") {
            return Ok(p);
        }
    }
    anyhow::bail!("yt-dlp produced no output file")
}

/// Probe duration / resolution / fps via `ffprobe`.
pub fn probe(video: &Path) -> Result<MediaInfo> {
    let out = Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_streams", "-show_format"])
        .arg(video)
        .output()
        .context("failed to run ffprobe (is ffmpeg installed?)")?;
    anyhow::ensure!(out.status.success(), "ffprobe failed for {}", video.display());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).context("parsing ffprobe JSON")?;

    let vstream = v
        .get("streams")
        .and_then(|s| s.as_array())
        .and_then(|streams| {
            streams
                .iter()
                .find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("video"))
        });
        // Audio-only containers (voice-only recordings: an opus-in-webm with no video
        // stream) have no `video` stream entry. Treat that as a zero-dimension clip rather
        // than an error — downstream enrich branches on width==0 && height==0 to run
        // transcript-only. `find` returns `None`, so the `.and_then`s below all yield None.

    let mut duration_s = v
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|d| *d > 0.0)
        .or_else(|| {
            vstream
                .and_then(|s| s.get("duration"))
                .and_then(|d| d.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|d| *d > 0.0)
        })
        .unwrap_or(0.0);

    // A recording assembled by concatenating streamed MediaRecorder chunks (see
    // `concat_chunks`) is raw-byte-concatenated WebM, not a single re-muxed container — ffprobe
    // has no reliable top-level *or* per-stream duration to read for it (confirmed: both are
    // simply absent, not just zero). Fall back to actually walking the file: `-c copy` just
    // remuxes (no re-encode cost), but ffmpeg still has to read every packet to do it, so its
    // final progress line reports the real total time regardless of what the container's own
    // metadata says.
    if duration_s == 0.0 {
        duration_s = probe_duration_via_decode(video).unwrap_or(0.0);
    }

    let width = vstream.and_then(|s| s.get("width")).and_then(|w| w.as_u64()).unwrap_or(0) as u32;
    let height = vstream.and_then(|s| s.get("height")).and_then(|h| h.as_u64()).unwrap_or(0) as u32;
    let fps = vstream
        .and_then(|s| s.get("r_frame_rate"))
        .and_then(|r| r.as_str())
        .map(parse_rational)
        .unwrap_or(0.0);

    Ok(MediaInfo { duration_s, width, height, fps })
}

/// Last-resort duration: remux the video stream to nowhere and read ffmpeg's own last
/// `time=` progress line, which reflects real elapsed playback time regardless of whether
/// the container's duration metadata is present, zero, or wrong.
fn probe_duration_via_decode(video: &Path) -> Option<f64> {
    let out = Command::new("ffmpeg")
        .args(["-i"])
        .arg(video)
        .args(["-map", "0:v:0", "-c", "copy", "-f", "null", "-"])
        .output()
        .ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let last_time = stderr.rmatch_indices("time=").next().map(|(i, _)| &stderr[i + 5..])?;
    let time_str = last_time.split_whitespace().next()?;
    parse_ffmpeg_timestamp(time_str)
}

/// Parse ffmpeg's `HH:MM:SS.ms` progress timestamp into seconds.
fn parse_ffmpeg_timestamp(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// Parse an ffmpeg rational like `"30/1"` or `"30000/1001"` into fps.
fn parse_rational(r: &str) -> f32 {
    match r.split_once('/') {
        Some((n, d)) => {
            let n: f32 = n.parse().unwrap_or(0.0);
            let d: f32 = d.parse().unwrap_or(1.0);
            if d == 0.0 {
                0.0
            } else {
                n / d
            }
        }
        None => r.parse().unwrap_or(0.0),
    }
}

/// Extract frames at `sample_fps` into `frames_dir`, returning `(t_ms, path)` per frame
/// in time order.
///
/// Frames are written as high-quality JPEG (`-q:v 2`, visually lossless for screen
/// content), not lossless PNG: a 5-minute 1080p clip at 4 fps is ~1,200 frames, and PNGs
/// made that 2–4 GB of encode work + disk + S3-mirror upload per clip, all of which the
/// gate/OCR/caption stages then re-decode. Old clips with `.png` frames keep working —
/// every consumer resolves frames by the path the extractor returned, and the frame HTTP
/// endpoint falls back across extensions.
pub fn extract_frames(video: &Path, frames_dir: &Path, sample_fps: f32) -> Result<Vec<(u64, PathBuf)>> {
    std::fs::create_dir_all(frames_dir)?;
    let pattern = frames_dir.join("%05d.jpg");
    let status = Command::new("ffmpeg")
        .arg("-i")
        .arg(video)
        .arg("-vf")
        .arg(format!("fps={sample_fps}"))
        .args(["-q:v", "2"])
        .arg("-y")
        .arg(&pattern)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("failed to run ffmpeg for frame extraction")?;
    anyhow::ensure!(status.success(), "ffmpeg frame extraction failed");

    let by_ext = |ext: &str| -> Vec<PathBuf> {
        std::fs::read_dir(frames_dir)
            .map(|it| {
                it.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().and_then(|x| x.to_str()) == Some(ext))
                    .collect()
            })
            .unwrap_or_default()
    };
    // One extension only — mixing (a pre-JPEG frames dir revisited after a deploy) would
    // double-count frames and skew every timestamp derived from the enumeration below.
    let mut frames = by_ext("jpg");
    if frames.is_empty() {
        frames = by_ext("png");
    }
    frames.sort();

    // ffmpeg's `fps` filter emits frame i (0-based) at t = i / sample_fps seconds.
    Ok(frames
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let t_ms = ((i as f64 / sample_fps as f64) * 1000.0).round() as u64;
            (t_ms, p)
        })
        .collect())
}

/// Best-effort: extract a `[start_s, start_s+duration_s)` mono 16 kHz WAV slice for
/// transcription, timestamps still relative to the *original* video (the caller offsets
/// segment starts/ends by `start_s` itself, since whisper's own output is relative to the
/// slice it was given). Used by [`extract_audio`] (whole file) and by incremental
/// transcription (only the newly-arrived audio each pass, not the whole growing recording —
/// re-transcribing from t=0 on every ~15s chunk would be O(n²) over a long recording).
/// `-ss` after `-i` is deliberately slower-but-frame-accurate: transcription quality is more
/// sensitive to a word being cut in half at a slice boundary than to seek speed here.
pub fn extract_audio_range(video: &Path, out_wav: &Path, start_s: f64, duration_s: Option<f64>) -> Option<PathBuf> {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-i").arg(video);
    if start_s > 0.0 {
        cmd.args(["-ss", &start_s.to_string()]);
    }
    if let Some(d) = duration_s {
        cmd.args(["-t", &d.to_string()]);
    }
    cmd.args(["-ar", "16000", "-ac", "1", "-vn", "-y"]).arg(out_wav);
    let status = cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().ok()?;
    if status.success() && out_wav.exists() {
        Some(out_wav.to_path_buf())
    } else {
        None
    }
}

/// Best-effort: extract mono 16 kHz WAV for transcription. Returns `None` when the video
/// has no audio (or extraction fails) — a transcript-less index is still valid.
pub fn extract_audio(video: &Path, out_wav: &Path) -> Option<PathBuf> {
    let status = Command::new("ffmpeg")
        .arg("-i")
        .arg(video)
        .args(["-ar", "16000", "-ac", "1", "-vn", "-y"])
        .arg(out_wav)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if status.success() && out_wav.exists() {
        Some(out_wav.to_path_buf())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection() {
        assert!(is_url("https://loom.com/share/abc"));
        assert!(is_url("http://example.com/v.mp4"));
        assert!(!is_url("/home/me/clip.mp4"));
        assert!(!is_url("clip.mp4"));
    }

    #[test]
    fn rational_parsing() {
        assert_eq!(parse_rational("30/1"), 30.0);
        assert!((parse_rational("30000/1001") - 29.97).abs() < 0.01);
        assert_eq!(parse_rational("24"), 24.0);
        assert_eq!(parse_rational("5/0"), 0.0);
    }

    #[test]
    fn ffmpeg_timestamp_parsing() {
        assert_eq!(parse_ffmpeg_timestamp("00:00:48.97"), Some(48.97));
        assert_eq!(parse_ffmpeg_timestamp("00:01:05.00"), Some(65.0));
        assert_eq!(parse_ffmpeg_timestamp("01:00:00.00"), Some(3600.0));
        assert_eq!(parse_ffmpeg_timestamp("garbage"), None);
        assert_eq!(parse_ffmpeg_timestamp(""), None);
    }
}
