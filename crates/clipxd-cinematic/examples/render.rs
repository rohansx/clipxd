//! Demo renderer — apply the auto-zoom to a real video, frame by frame.
//!
//! `cargo run --example render -- <in.mp4> <events.json> <out.mp4>`
//!
//! `events.json` = `{ "cursors": [{t,x,y}…], "clicks": [{t,x,y}…] }` (x,y normalized 0..1).
//! This is the demo path for the clean-room cinematic engine; the real recorder will feed
//! it a live capture + event track. ffmpeg does decode/encode; the crop+scale is ours.

use anyhow::{ensure, Context, Result};
use clipxd_cinematic::{compute_zoom_track, crop_rect, Click, CursorSample, ZoomConfig};
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Info {
    w: u32,
    h: u32,
    fps: f64,
    duration: f64,
}

fn probe(p: &str) -> Result<Info> {
    let out = Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_streams", "-show_format", p])
        .output()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let vs = v["streams"].as_array().context("no streams")?.iter()
        .find(|s| s["codec_type"] == "video").context("no video stream")?;
    let r = vs["r_frame_rate"].as_str().unwrap_or("30/1");
    let (n, d) = r.split_once('/').unwrap_or(("30", "1"));
    Ok(Info {
        w: vs["width"].as_u64().unwrap_or(0) as u32,
        h: vs["height"].as_u64().unwrap_or(0) as u32,
        fps: n.parse::<f64>().unwrap_or(30.0) / d.parse::<f64>().unwrap_or(1.0),
        duration: v["format"]["duration"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0),
    })
}

fn run(c: &mut Command) -> Result<()> {
    ensure!(c.stdout(Stdio::null()).stderr(Stdio::null()).status()?.success(), "ffmpeg/ffprobe failed");
    Ok(())
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    ensure!(a.len() >= 4, "usage: render <in.mp4> <events.json> <out.mp4>");
    let (inp, events, outp) = (&a[1], &a[2], &a[3]);

    let info = probe(inp)?;
    let ev: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(events)?)?;
    let cursors: Vec<CursorSample> = serde_json::from_value(ev["cursors"].clone()).unwrap_or_default();
    let clicks: Vec<Click> = serde_json::from_value(ev["clicks"].clone()).unwrap_or_default();
    let track = compute_zoom_track(&cursors, &clicks, info.duration, &ZoomConfig { fps: info.fps, ..Default::default() });
    eprintln!("{}x{} @ {:.0}fps, {:.1}s → {} keyframes, {} clicks", info.w, info.h, info.fps, info.duration, track.len(), clicks.len());

    let tmp = std::env::temp_dir().join("clipxd-cine");
    let (fin, fout) = (tmp.join("in"), tmp.join("out"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&fin)?;
    std::fs::create_dir_all(&fout)?;

    run(Command::new("ffmpeg").args(["-y", "-i", inp, "-vf", &format!("fps={}", info.fps)]).arg(fin.join("%05d.png")))?;

    let mut frames: Vec<PathBuf> = std::fs::read_dir(&fin)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    frames.sort();

    for (i, f) in frames.iter().enumerate() {
        let kf = track.get(i).or_else(|| track.last()).copied().context("empty track")?;
        let img = image::open(f)?.to_rgba8();
        let (w, h) = img.dimensions();
        let r = crop_rect(&kf, w, h);
        let sub = image::imageops::crop_imm(&img, r.x, r.y, r.w, r.h).to_image();
        let zoomed = image::imageops::resize(&sub, w, h, image::imageops::FilterType::Lanczos3);
        zoomed.save(fout.join(format!("{:05}.png", i + 1)))?;
    }

    run(Command::new("ffmpeg")
        .args(["-y", "-framerate", &info.fps.to_string(), "-i"])
        .arg(fout.join("%05d.png"))
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", outp]))?;
    println!("✓ wrote {outp}");
    Ok(())
}
