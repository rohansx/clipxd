//! `clipxd beautify` — the clean-room cinematic layer: auto-zoom that follows cursor/clicks,
//! composited onto a background with padding, optionally a browser mockup, keystroke pills,
//! and blur (pixelation) over redacted regions — exported as MP4 / WebM / GIF. ffmpeg
//! decodes + encodes; the per-frame compositing is ours (`clipxd-cinematic`) and runs in
//! parallel across frames (rayon) so export isn't single-threaded-slow.

use anyhow::{ensure, Context, Result};
use clipxd_cinematic::{
    annotations_at, browser_in, compute_zoom_track, crop_rect, frame_layout, keystroke_pills, pill_at, Annotation,
    Background, Click, CursorSample, FrameLayout, SceneConfig, ZoomConfig,
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
    /// Use this precomputed zoom track (a clip's `zoom.json`) instead of computing one from
    /// events — lets the server render the same content-aware auto-zoom the editor previews.
    pub zoom: Option<PathBuf>,
    /// Apply a `.clipxd` project: manual zoom regions (override the auto-zoom), trim cuts
    /// (drop spans), and speed ramps (decimate spans) — bake the editor's edits into output.
    pub project: Option<PathBuf>,
}

pub fn beautify(video: &Path, events: Option<&Path>, out: &Path, opts: &BeautifyOpts) -> Result<()> {
    let info = clipxd_import::media::probe(video)?;
    let ev = load_events(events)?;
    let auto = || {
        compute_zoom_track(&ev.cursors, &ev.clicks, info.duration_s, &ZoomConfig { fps: info.fps as f64, spring: Some(18.0), ..Default::default() })
    };
    let mut track = match &opts.zoom {
        Some(zp) => {
            let loaded: Vec<clipxd_cinematic::ZoomKeyframe> =
                std::fs::read_to_string(zp).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
            if loaded.is_empty() { auto() } else { loaded }
        }
        None => auto(),
    };
    // the editor's .clipxd project: manual zoom regions override the auto-zoom (centered snap)
    let project = opts.project.as_deref().and_then(load_project).unwrap_or_default();
    for kf in track.iter_mut() {
        if let Some(z) = project.zoom_regions.iter().find(|z| kf.t >= z.start && kf.t <= z.end) {
            kf.scale = z.scale;
            kf.cx = 0.5;
            kf.cy = 0.5;
        }
    }
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

    // apply the project's trim (drop spans) + speed (decimate spans) → the output frame order
    let emit: Vec<usize> = (0..frames.len())
        .filter(|&i| {
            let t = i as f64 / info.fps as f64;
            if project.edit_regions.iter().any(|e| e.kind == "trim" && t >= e.start && t < e.end) {
                return false; // cut
            }
            if let Some(s) = project.edit_regions.iter().find(|e| e.kind == "speed" && t >= e.start && t <= e.end) {
                let r = s.rate.round().max(1.0) as usize;
                if r > 1 && i % r != 0 {
                    return false; // play r× faster by keeping every r-th frame
                }
            }
            true
        })
        .collect();
    ensure!(!emit.is_empty(), "every frame was trimmed");
    eprintln!(
        "project: {} zoom-region(s), {} edit(s) → emit {}/{} frames",
        project.zoom_regions.len(), project.edit_regions.len(), emit.len(), frames.len()
    );

    let scene = SceneConfig { background: parse_bg(&opts.bg), padding: opts.padding, out_w: info.width, out_h: info.height, ..Default::default() };
    let layout = frame_layout(info.width, info.height, &scene); // constant src size → constant content rect
    let background = wallpaper(&opts.bg, info.width, info.height);

    // Frames are independent → render in parallel (output index j ← source frame emit[j]).
    emit.par_iter().enumerate().try_for_each(|(j, &src)| -> Result<()> {
        let t = src as f64 / info.fps as f64;
        let kf = track.get(src).or_else(|| track.last()).copied().context("empty zoom track")?;
        let mut img = image::open(&frames[src])?.to_rgba8();

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

        // annotation overlay (arrows / boxes / text / highlights) on top of the produced frame
        let anns = annotations_at(&ev.anns, t);
        if !anns.is_empty() {
            draw_annotations(&mut canvas, &layout, font.as_ref(), &anns);
        }

        // cursor effects — a soft spotlight + click ripples, mapped through the zoom crop so
        // they sit exactly where the cursor/click is even as the camera pushes in.
        if !ev.cursors.is_empty() || !ev.clicks.is_empty() {
            let to_out = |sx: f64, sy: f64| -> Option<(f32, f32)> {
                let (px, py) = (sx * w as f64, sy * h as f64);
                if px < r.x as f64 || px > (r.x + r.w) as f64 || py < r.y as f64 || py > (r.y + r.h) as f64 {
                    return None;
                }
                let (vx, vy, vw, vh) = if opts.mockup {
                    let m = browser_in(layout.content_w, layout.content_h);
                    ((layout.content_x + m.video_x) as f64, (layout.content_y + m.video_y) as f64, m.video_w as f64, m.video_h as f64)
                } else {
                    (layout.content_x as f64, layout.content_y as f64, layout.content_w as f64, layout.content_h as f64)
                };
                let ox = vx + (px - r.x as f64) / r.w as f64 * vw;
                let oy = vy + (py - r.y as f64) / r.h as f64 * vh;
                Some((ox as f32, oy as f32))
            };
            // styled cursor highlight at the interpolated (smooth) position: soft halo + a
            // crisp white dot with a dark outline — a "produced" pointer that reads cleanly.
            if let Some((ox, oy)) = cursor_lerp(&ev.cursors, t).and_then(|(sx, sy)| to_out(sx, sy)) {
                let s = layout.content_h as f32;
                glow(&mut canvas, ox, oy, s * 0.035, [180, 210, 255], 0.16);
                disc(&mut canvas, ox, oy, s * 0.013, [10, 15, 25], 0.55);
                disc(&mut canvas, ox, oy, s * 0.009, [255, 255, 255], 0.96);
            }
            for clk in ev.clicks.iter().filter(|c| t >= c.t && t <= c.t + 0.6) {
                if let Some((ox, oy)) = to_out(clk.x, clk.y) {
                    let p = ((t - clk.t) / 0.6) as f32;
                    let rad = layout.content_h as f32 * 0.02 * (1.0 + p * 2.6);
                    ring(&mut canvas, ox, oy, rad, 3.5, [140, 198, 255], (1.0 - p) * 0.85);
                }
            }
        }

        canvas.save(fout.join(format!("{:05}.png", j + 1)))?;
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

fn blend_px(img: &mut RgbaImage, x: u32, y: u32, c: [u8; 3], a: f32) {
    let bg = img.get_pixel(x, y).0;
    let m = |f: u8, b: u8| (f as f32 * a + b as f32 * (1.0 - a)).round() as u8;
    img.put_pixel(x, y, Rgba([m(c[0], bg[0]), m(c[1], bg[1]), m(c[2], bg[2]), 255]));
}

/// An anti-aliased alpha-blended ring (for click ripples).
fn ring(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, thick: f32, c: [u8; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let r1 = r + thick;
    for y in ((cy - r1) as i32).max(0)..((cy + r1) as i32 + 1).min(ih) {
        for x in ((cx - r1) as i32).max(0)..((cx + r1) as i32 + 1).min(iw) {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let edge = 1.0 - ((d - r).abs() / thick).min(1.0);
            if edge > 0.0 {
                blend_px(img, x as u32, y as u32, c, a * edge);
            }
        }
    }
}

/// A soft radial glow (for the cursor spotlight).
fn glow(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, c: [u8; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    for y in ((cy - r) as i32).max(0)..((cy + r) as i32 + 1).min(ih) {
        for x in ((cx - r) as i32).max(0)..((cx + r) as i32 + 1).min(iw) {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if d < r {
                blend_px(img, x as u32, y as u32, c, a * (1.0 - d / r));
            }
        }
    }
}

/// A soft alpha-blended filled disc with 1px anti-aliased edge (for the cursor dot/outline).
fn disc(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, c: [u8; 3], a: f32) {
    if a <= 0.0 || r <= 0.0 {
        return;
    }
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    for y in ((cy - r) as i32).max(0)..((cy + r) as i32 + 1).min(ih) {
        for x in ((cx - r) as i32).max(0)..((cx + r) as i32 + 1).min(iw) {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let edge = (r - d).clamp(0.0, 1.0); // 1px feather at the rim
            if edge > 0.0 {
                blend_px(img, x as u32, y as u32, c, a * edge);
            }
        }
    }
}

/// The cursor position (normalized) at time `t`, **linearly interpolated** between the two
/// bracketing samples (smooth follow, vs. a jumpy nearest-sample lookup). Samples are sorted.
fn cursor_lerp(cursors: &[CursorSample], t: f64) -> Option<(f64, f64)> {
    if cursors.is_empty() {
        return None;
    }
    let mut prev = &cursors[0];
    for c in cursors {
        if c.t >= t {
            let span = c.t - prev.t;
            if span < 1e-6 {
                return Some((c.x, c.y));
            }
            let f = ((t - prev.t) / span).clamp(0.0, 1.0);
            return Some((prev.x + (c.x - prev.x) * f, prev.y + (c.y - prev.y) * f));
        }
        prev = c;
    }
    Some((prev.x, prev.y))
}

fn draw_annotations(canvas: &mut RgbaImage, layout: &FrameLayout, font: Option<&ab_glyph::FontVec>, anns: &[&Annotation]) {
    let px = |x: f64| (layout.content_x as f64 + x * layout.content_w as f64) as f32;
    let py = |y: f64| (layout.content_y as f64 + y * layout.content_h as f64) as f32;
    for a in anns {
        let rgb = if a.color.is_empty() { [88, 166, 255] } else { hex(&a.color) };
        let c = [rgb[0], rgb[1], rgb[2], 255];
        let (x0, y0, x1, y1) = (px(a.x), py(a.y), px(a.x2), py(a.y2));
        match a.kind.as_str() {
            "highlight" => {
                let (lx, ly) = (x0.min(x1) as i64, y0.min(y1) as i64);
                let (w, h) = ((x0 - x1).abs() as i64, (y0 - y1).abs() as i64);
                blend_rect(canvas, lx, ly, w, h, [rgb[0], rgb[1], rgb[2], 96]);
            }
            "box" => stroke_rect(canvas, x0, y0, x1, y1, c, 3),
            "arrow" => draw_arrow(canvas, x0, y0, x1, y1, c, 3),
            "text" => {
                if let Some(f) = font {
                    let s = (layout.content_h as f32 * 0.04).clamp(18.0, 36.0);
                    text::draw_text(canvas, x0, y0, &a.text, s, f, rgb);
                }
            }
            _ => {}
        }
    }
}

fn fill_square(img: &mut RgbaImage, cx: i64, cy: i64, r: i64, c: [u8; 4]) {
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    for yy in (cy - r).max(0)..=(cy + r).min(ih - 1) {
        for xx in (cx - r).max(0)..=(cx + r).min(iw - 1) {
            img.put_pixel(xx as u32, yy as u32, Rgba(c));
        }
    }
}

fn stroke_line(img: &mut RgbaImage, x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 4], r: i64) {
    let steps = (x1 - x0).hypot(y1 - y0).ceil().max(1.0) as i32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        fill_square(img, (x0 + (x1 - x0) * t) as i64, (y0 + (y1 - y0) * t) as i64, r, c);
    }
}

fn stroke_rect(img: &mut RgbaImage, x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 4], r: i64) {
    stroke_line(img, x0, y0, x1, y0, c, r);
    stroke_line(img, x1, y0, x1, y1, c, r);
    stroke_line(img, x1, y1, x0, y1, c, r);
    stroke_line(img, x0, y1, x0, y0, c, r);
}

fn blend_rect(img: &mut RgbaImage, x: i64, y: i64, w: i64, h: i64, c: [u8; 4]) {
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    let a = c[3] as f32 / 255.0;
    for yy in y.max(0)..(y + h).min(ih) {
        for xx in x.max(0)..(x + w).min(iw) {
            let bg = img.get_pixel(xx as u32, yy as u32).0;
            let mix = |f: u8, b: u8| (f as f32 * a + b as f32 * (1.0 - a)) as u8;
            img.put_pixel(xx as u32, yy as u32, Rgba([mix(c[0], bg[0]), mix(c[1], bg[1]), mix(c[2], bg[2]), 255]));
        }
    }
}

fn draw_arrow(img: &mut RgbaImage, x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 4], r: i64) {
    stroke_line(img, x0, y0, x1, y1, c, r);
    let ang = (y1 - y0).atan2(x1 - x0);
    let head = (r as f32 * 6.0).max(16.0);
    for da in [2.5_f32, -2.5] {
        stroke_line(img, x1, y1, x1 + head * (ang + da).cos(), y1 + head * (ang + da).sin(), c, r);
    }
}

struct Events {
    cursors: Vec<CursorSample>,
    clicks: Vec<Click>,
    keys: Vec<(f64, String)>,
    blurs: Vec<BlurRegion>,
    anns: Vec<Annotation>,
}

#[derive(Default)]
struct Project {
    zoom_regions: Vec<ZoomRegionJ>,
    edit_regions: Vec<EditRegionJ>,
}

struct ZoomRegionJ {
    start: f64,
    end: f64,
    scale: f64,
}

struct EditRegionJ {
    kind: String,
    start: f64,
    end: f64,
    rate: f64,
}

fn load_project(p: &Path) -> Option<Project> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
    let zoom_regions = v["zoom_regions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|z| Some(ZoomRegionJ { start: z["start"].as_f64()?, end: z["end"].as_f64()?, scale: z["scale"].as_f64()? }))
                .collect()
        })
        .unwrap_or_default();
    let edit_regions = v["edit_regions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    Some(EditRegionJ {
                        kind: e["kind"].as_str()?.to_string(),
                        start: e["start"].as_f64()?,
                        end: e["end"].as_f64()?,
                        rate: e["rate"].as_f64().unwrap_or(1.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Project { zoom_regions, edit_regions })
}

fn load_events(p: Option<&Path>) -> Result<Events> {
    let Some(p) = p else {
        return Ok(Events { cursors: vec![], clicks: vec![], keys: vec![], blurs: vec![], anns: vec![] });
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
        anns: serde_json::from_value(v["annotations"].clone()).unwrap_or_default(),
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

/// A premium mesh-gradient wallpaper behind the video (the "beautification" background).
/// Named presets — "aurora" (default), "dusk", "ocean", "violet", "noir" — or a hex colour.
/// Painted once per render, not per frame.
fn wallpaper(bg: &str, w: u32, h: u32) -> RgbaImage {
    if bg.starts_with('#') {
        return solid(w, h, hex(bg));
    }
    // base colour + radial colour blobs (fx, fy, radius_frac, rgb)
    let (base, blobs): ([u8; 3], &[(f32, f32, f32, [u8; 3])]) = match bg {
        "noir" => ([10, 12, 16], &[(0.2, 0.1, 0.7, [40, 46, 60]), (0.85, 0.95, 0.7, [22, 26, 36])]),
        "dusk" => ([20, 16, 34], &[(0.15, 0.1, 0.75, [90, 70, 200]), (0.88, 0.18, 0.65, [205, 80, 150]), (0.7, 0.95, 0.7, [60, 90, 185])]),
        "ocean" => ([7, 18, 30], &[(0.18, 0.15, 0.75, [40, 120, 205]), (0.86, 0.9, 0.7, [30, 185, 175]), (0.6, 0.25, 0.55, [80, 90, 220])]),
        "violet" => ([16, 10, 28], &[(0.2, 0.2, 0.75, [130, 80, 235]), (0.85, 0.15, 0.65, [210, 90, 200]), (0.5, 0.95, 0.7, [90, 70, 215])]),
        // "aurora" / "gradient" / anything else → the signature look (matches the app's backdrop)
        _ => ([8, 11, 22], &[(0.12, 0.1, 0.72, [60, 110, 230]), (0.88, 0.14, 0.66, [120, 80, 230]), (0.8, 0.92, 0.72, [40, 200, 160]), (0.1, 0.95, 0.66, [230, 90, 150])]),
    };
    mesh(w, h, base, blobs)
}

fn solid(w: u32, h: u32, c: [u8; 3]) -> RgbaImage {
    let mut img = RgbaImage::new(w.max(1), h.max(1));
    for px in img.pixels_mut() {
        *px = Rgba([c[0], c[1], c[2], 255]);
    }
    img
}

fn mesh(w: u32, h: u32, base: [u8; 3], blobs: &[(f32, f32, f32, [u8; 3])]) -> RgbaImage {
    let (w, h) = (w.max(1), h.max(1));
    let (wf, hf) = (w as f32, h as f32);
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (mut r, mut g, mut b) = (base[0] as f32, base[1] as f32, base[2] as f32);
            for &(fx, fy, rad, c) in blobs {
                let (dx, dy) = (x as f32 / wf - fx, y as f32 / hf - fy);
                let falloff = (1.0 - (dx * dx + dy * dy).sqrt() / rad).max(0.0);
                let a = falloff * falloff * 0.8; // smooth, soft blend toward the blob colour
                r += (c[0] as f32 - r) * a;
                g += (c[1] as f32 - g) * a;
                b += (c[2] as f32 - b) * a;
            }
            img.put_pixel(x, y, Rgba([r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8, 255]));
        }
    }
    img
}

fn run(c: &mut Command) -> Result<()> {
    ensure!(c.stdout(Stdio::null()).stderr(Stdio::null()).status()?.success(), "ffmpeg failed");
    Ok(())
}
