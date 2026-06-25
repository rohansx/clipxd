//! `clipxd beautify` — the clean-room cinematic layer: auto-zoom that follows cursor/clicks,
//! composited onto a background with padding, optionally a browser mockup, keystroke pills,
//! and blur (pixelation) over redacted regions — exported as MP4 / WebM / GIF. ffmpeg
//! decodes + encodes; the per-frame compositing is ours (`clipxd-cinematic`) and runs in
//! parallel across frames (rayon) so export isn't single-threaded-slow.

use anyhow::{ensure, Context, Result};
use clipxd_cinematic::{
    browser_in, compute_zoom_track, crop_rect, frame_layout, keystroke_pills, pill_at, Background, Click, CursorSample,
    SceneConfig, ZoomConfig,
};
use clipxd_recorder::BlurRegion;
use image::{imageops, Rgba, RgbaImage};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::text;

pub struct BeautifyOpts {
    pub padding: f64,
    pub bg: String,
    pub mockup: bool,
    pub format: String,
}

pub fn beautify(video: &Path, events: Option<&Path>, out: &Path, opts: &BeautifyOpts) -> Result<()> {
    let info = clipxd_import::media::probe(video)?;
    let ev = load_events(events)?;
    let track = compute_zoom_track(
        &ev.cursors,
        &ev.clicks,
        info.duration_s,
        &ZoomConfig { fps: info.fps as f64, spring: Some(18.0), ..Default::default() },
    );
    let pills = keystroke_pills(&ev.keys, 0.4, 1.2);
    let font = text::load_font();
    eprintln!(
        "{}x{} @ {:.0}fps {:.1}s → {} keyframes, {} clicks, {} pills, {} blur; bg={} mockup={} format={}",
        info.width, info.height, info.fps, info.duration_s, track.len(), ev.clicks.len(), pills.len(), ev.blurs.len(),
        opts.bg, opts.mockup, opts.format
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

    // Frames are independent → render in parallel.
    frames.par_iter().enumerate().try_for_each(|(i, f)| -> Result<()> {
        let t = i as f64 / info.fps as f64;
        let kf = track.get(i).or_else(|| track.last()).copied().context("empty zoom track")?;
        let mut img = image::open(f)?.to_rgba8();

        // pixelate any active redaction region on the source frame (so blur tracks the zoom)
        for b in ev.blurs.iter().filter(|b| t >= b.start && t <= b.end) {
            pixelate(&mut img, b);
        }

        let (w, h) = img.dimensions();
        let r = crop_rect(&kf, w, h);
        let sub = imageops::crop_imm(&img, r.x, r.y, r.w, r.h).to_image();

        let mut canvas = background.clone();
        if opts.mockup {
            let m = browser_in(layout.content_w, layout.content_h);
            fill_rect(&mut canvas, layout.content_x, layout.content_y, layout.content_w, m.bar_h, [22, 28, 39, 255]);
            let dot_col = [[248, 81, 73, 255], [210, 153, 34, 255], [63, 185, 80, 255]];
            for (k, dx) in m.dot_x.iter().enumerate() {
                fill_circle(&mut canvas, (layout.content_x + dx) as i64, (layout.content_y + m.dot_y) as i64, m.dot_r as i64, dot_col[k]);
            }
            let vid = imageops::resize(&sub, m.video_w, m.video_h, imageops::FilterType::Lanczos3);
            imageops::overlay(&mut canvas, &vid, (layout.content_x + m.video_x) as i64, (layout.content_y + m.video_y) as i64);
        } else {
            let zoomed = imageops::resize(&sub, layout.content_w, layout.content_h, imageops::FilterType::Lanczos3);
            imageops::overlay(&mut canvas, &zoomed, layout.content_x as i64, layout.content_y as i64);
        }

        // keystroke pill, bottom-center of the content area
        if let (Some(font), Some(pill)) = (font.as_ref(), pill_at(&pills, t)) {
            draw_pill(&mut canvas, &layout, font, &pill.text);
        }

        canvas.save(fout.join(format!("{:05}.png", i + 1)))?;
        Ok(())
    })?;

    encode(&fout, info.fps, &opts.format, out)
}

fn draw_pill(canvas: &mut RgbaImage, layout: &clipxd_cinematic::FrameLayout, font: &ab_glyph::FontVec, txt: &str) {
    let px = (layout.content_h as f32 * 0.035).clamp(16.0, 30.0);
    let tw = text::text_width(font, px, txt);
    let (padx, pady) = (px * 0.7, px * 0.45);
    let (pw, ph) = (tw + padx * 2.0, px + pady * 2.0);
    let cx = layout.content_x as f32 + layout.content_w as f32 / 2.0;
    let by = (layout.content_y + layout.content_h) as f32 - ph - px;
    let bx = cx - pw / 2.0;
    fill_rect(canvas, bx.max(0.0) as u32, by.max(0.0) as u32, pw as u32, ph as u32, [12, 16, 23, 235]);
    text::draw_text(canvas, bx + padx, by + pady, txt, px, font, [230, 237, 243]);
}

fn pixelate(img: &mut RgbaImage, b: &BlurRegion) {
    let (w, h) = img.dimensions();
    let rx = (b.x * w as f64).clamp(0.0, w as f64 - 1.0) as u32;
    let ry = (b.y * h as f64).clamp(0.0, h as f64 - 1.0) as u32;
    let rw = ((b.w * w as f64) as u32).min(w - rx).max(1);
    let rh = ((b.h * h as f64) as u32).min(h - ry).max(1);
    let sub = imageops::crop_imm(img, rx, ry, rw, rh).to_image();
    let small = imageops::resize(&sub, (rw / 16).max(1), (rh / 16).max(1), imageops::FilterType::Triangle);
    let mosaic = imageops::resize(&small, rw, rh, imageops::FilterType::Nearest);
    imageops::overlay(img, &mosaic, rx as i64, ry as i64);
}

fn encode(frames_dir: &Path, fps: f32, format: &str, out: &Path) -> Result<()> {
    let pattern = frames_dir.join("%05d.png");
    let mut c = Command::new("ffmpeg");
    c.args(["-y", "-framerate", &fps.to_string(), "-i"]).arg(&pattern);
    match format {
        "gif" => {
            c.args(["-vf", "fps=15,scale=900:-1:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse"]);
        }
        "webm" => {
            c.args(["-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "32", "-pix_fmt", "yuv420p"]);
        }
        _ => {
            c.args(["-c:v", "libx264", "-pix_fmt", "yuv420p"]);
        }
    }
    run(c.arg(out))
}

fn fill_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: [u8; 4]) {
    let (iw, ih) = img.dimensions();
    for yy in y..(y + h).min(ih) {
        for xx in x..(x + w).min(iw) {
            img.put_pixel(xx, yy, Rgba(c));
        }
    }
}

fn fill_circle(img: &mut RgbaImage, cx: i64, cy: i64, r: i64, c: [u8; 4]) {
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    for yy in (cy - r).max(0)..(cy + r + 1).min(ih) {
        for xx in (cx - r).max(0)..(cx + r + 1).min(iw) {
            let (dx, dy) = (xx - cx, yy - cy);
            if dx * dx + dy * dy <= r * r {
                img.put_pixel(xx as u32, yy as u32, Rgba(c));
            }
        }
    }
}

struct Events {
    cursors: Vec<CursorSample>,
    clicks: Vec<Click>,
    keys: Vec<(f64, String)>,
    blurs: Vec<BlurRegion>,
}

fn load_events(p: Option<&Path>) -> Result<Events> {
    let Some(p) = p else {
        return Ok(Events { cursors: vec![], clicks: vec![], keys: vec![], blurs: vec![] });
    };
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p)?)?;
    let keys = v["keys"]
        .as_array()
        .map(|a| a.iter().filter_map(|k| Some((k["t"].as_f64()?, k["key"].as_str()?.to_string()))).collect())
        .unwrap_or_default();
    Ok(Events {
        cursors: serde_json::from_value(v["cursors"].clone()).unwrap_or_default(),
        clicks: serde_json::from_value(v["clicks"].clone()).unwrap_or_default(),
        keys,
        blurs: serde_json::from_value(v["blur"].clone()).unwrap_or_default(),
    })
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
                    let t = (x as f32 / w as f32 + y as f32 / h as f32) / 2.0;
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
