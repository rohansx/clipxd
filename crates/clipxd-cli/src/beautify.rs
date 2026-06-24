//! `clipxd beautify` — apply the clean-room cinematic layer to a recording: auto-zoom that
//! follows the cursor/clicks, composited onto a background with padding. ffmpeg decodes +
//! encodes; the crop/zoom/composite is ours (`clipxd-cinematic`).

use anyhow::{ensure, Context, Result};
use clipxd_cinematic::{compute_zoom_track, crop_rect, frame_layout, Background, Click, CursorSample, SceneConfig, ZoomConfig};
use image::{imageops, Rgba, RgbaImage};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct BeautifyOpts {
    pub padding: f64,
    pub bg: String,
}

pub fn beautify(video: &Path, events: Option<&Path>, out: &Path, opts: &BeautifyOpts) -> Result<()> {
    let info = clipxd_import::media::probe(video)?;
    let (cursors, clicks) = load_events(events)?;
    let track = compute_zoom_track(
        &cursors,
        &clicks,
        info.duration_s,
        &ZoomConfig { fps: info.fps as f64, spring: Some(18.0), ..Default::default() },
    );
    eprintln!(
        "{}x{} @ {:.0}fps {:.1}s → {} keyframes, {} clicks; bg={} padding={}",
        info.width, info.height, info.fps, info.duration_s, track.len(), clicks.len(), opts.bg, opts.padding
    );

    let tmp = std::env::temp_dir().join("clipxd-beautify");
    let (fin, fout) = (tmp.join("in"), tmp.join("out"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&fin)?;
    std::fs::create_dir_all(&fout)?;
    run(Command::new("ffmpeg").args(["-y", "-i"]).arg(video).args(["-vf", &format!("fps={}", info.fps)]).arg(fin.join("%05d.png")))?;

    let mut frames: Vec<PathBuf> = std::fs::read_dir(&fin)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    frames.sort();
    ensure!(!frames.is_empty(), "no frames extracted");

    let scene = SceneConfig { background: parse_bg(&opts.bg), padding: opts.padding, out_w: info.width, out_h: info.height, ..Default::default() };
    let layout = frame_layout(info.width, info.height, &scene); // constant src size → constant content rect
    let background = render_background(&scene);

    for (i, f) in frames.iter().enumerate() {
        let kf = track.get(i).or_else(|| track.last()).copied().context("empty zoom track")?;
        let img = image::open(f)?.to_rgba8();
        let (w, h) = img.dimensions();
        let r = crop_rect(&kf, w, h);
        let sub = imageops::crop_imm(&img, r.x, r.y, r.w, r.h).to_image();
        let zoomed = imageops::resize(&sub, layout.content_w, layout.content_h, imageops::FilterType::Lanczos3);
        let mut canvas = background.clone();
        imageops::overlay(&mut canvas, &zoomed, layout.content_x as i64, layout.content_y as i64);
        canvas.save(fout.join(format!("{:05}.png", i + 1)))?;
    }

    run(Command::new("ffmpeg")
        .args(["-y", "-framerate", &info.fps.to_string(), "-i"])
        .arg(fout.join("%05d.png"))
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(out))?;
    Ok(())
}

fn load_events(p: Option<&Path>) -> Result<(Vec<CursorSample>, Vec<Click>)> {
    match p {
        None => Ok((vec![], vec![])),
        Some(p) => {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p)?)?;
            Ok((
                serde_json::from_value(v["cursors"].clone()).unwrap_or_default(),
                serde_json::from_value(v["clicks"].clone()).unwrap_or_default(),
            ))
        }
    }
}

fn parse_bg(s: &str) -> Background {
    if s.is_empty() || s == "gradient" {
        Background::Linear { angle: 135.0, stops: vec!["#1f6feb".into(), "#0d1117".into()] }
    } else {
        Background::Solid(s.to_string())
    }
}

fn hex(s: &str) -> [u8; 3] {
    let n = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0x0d_1117);
    [(n >> 16) as u8, (n >> 8) as u8, n as u8]
}

fn lerp8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

fn render_background(scene: &SceneConfig) -> RgbaImage {
    let (w, h) = (scene.out_w.max(1), scene.out_h.max(1));
    let mut img = RgbaImage::new(w, h);
    match &scene.background {
        Background::Solid(c) => {
            let [r, g, b] = hex(c);
            for px in img.pixels_mut() {
                *px = Rgba([r, g, b, 255]);
            }
        }
        Background::Linear { stops, .. } => {
            let a = hex(stops.first().map(String::as_str).unwrap_or("#1f6feb"));
            let b = hex(stops.last().map(String::as_str).unwrap_or("#0d1117"));
            for y in 0..h {
                for x in 0..w {
                    let t = (x as f32 / w as f32 + y as f32 / h as f32) / 2.0; // diagonal
                    img.put_pixel(x, y, Rgba([lerp8(a[0], b[0], t), lerp8(a[1], b[1], t), lerp8(a[2], b[2], t), 255]));
                }
            }
        }
        Background::Image(_) => {
            for px in img.pixels_mut() {
                *px = Rgba([13, 17, 23, 255]);
            }
        }
    }
    img
}

fn run(c: &mut Command) -> Result<()> {
    ensure!(c.stdout(Stdio::null()).stderr(Stdio::null()).status()?.success(), "ffmpeg failed");
    Ok(())
}
