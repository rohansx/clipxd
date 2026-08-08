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
use clipxd_index::{Emphasis, Index, SubtitleStyle};
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
    /// Burn styled captions into the output. Path to the clip's `index.json`; reads
    /// `transcript` + `subtitle_emphasis` + `subtitle_style` (the design the user picked on
    /// the clip page + the Ollama indexing-time focus-word pass). No-op if no transcript or
    /// no `subtitle_style` is set.
    pub captions: Option<PathBuf>,
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
    // The editor's .clipxd project: manual zoom regions override the auto-zoom. The focus point
    // comes from the region too — it used to be hardcoded to the centre, so two renders that
    // differed only in focus came out byte-identical while the editor's `FocusOverlay` happily
    // previewed a crop the renderer would never deliver.
    let project = opts.project.as_deref().and_then(load_project).unwrap_or_default();
    for kf in track.iter_mut() {
        if let Some(z) = project.zoom_regions.iter().find(|z| kf.t >= z.start && kf.t <= z.end) {
            kf.scale = z.scale;
            kf.cx = z.cx;
            kf.cy = z.cy;
        }
    }
    let pills = keystroke_pills(&ev.keys, 0.4, 1.2);
    // Styled captions: load the clip's index.json (transcript + the user's subtitle_style +
    // the indexing-time subtitle_emphasis). Only burn captions when the user actually picked
    // a design (subtitle_style present) AND there's a transcript to caption.
    let captions = opts.captions.as_deref().and_then(load_captions);
    if let Some(c) = captions.as_ref() {
        eprintln!(
            "captions: design={} emphasis={} segments={}",
            c.style.design, c.emphasis.is_some(), c.transcript.len()
        );
    }
    let font = text::load_font();
    eprintln!(
        "{}x{} @ {:.0}fps {:.1}s → {} keyframes, {} clicks, {} pills, {} blur; bg={} mockup={} format={}",
        info.width, info.height, info.fps, info.duration_s, track.len(), ev.clicks.len(), pills.len(), ev.blurs.len(),
        opts.bg, opts.mockup, opts.format
    );

    // Per-render scratch dir. It used to be a fixed `clipxd-beautify` that was wiped on entry,
    // so two renders in flight at once (the web service renders per request) deleted each
    // other's frames mid-export. `Scratch` removes it on the way out, `?`-returns included —
    // and `sweep_stale_scratch` covers the deaths `Drop` cannot (SIGKILL / OOM-kill).
    let parent = std::env::temp_dir();
    sweep_stale_scratch(&parent, SCRATCH_MAX_AGE);
    let tmp = parent.join(format!(
        "clipxd-beautify-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    ));
    let _scratch = Scratch(tmp.clone());
    let (fin, fout) = (tmp.join("in"), tmp.join("out"));
    std::fs::create_dir_all(&fin)?;
    std::fs::create_dir_all(&fout)?;
    run(Command::new("ffmpeg").args(["-y", "-i"]).arg(video).args(["-vf", &format!("fps={}", info.fps)]).arg(fin.join("%05d.png")))?;

    let mut frames: Vec<PathBuf> = std::fs::read_dir(&fin)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    frames.sort();
    ensure!(!frames.is_empty(), "no frames extracted");

    // Apply the project's trim (drop spans) + speed (rate spans) → the output frame order.
    // The timeline runs to the *extracted frame count*, not the probed duration: a WebM
    // assembled from MediaRecorder chunks probes short, and a timeline that ended early would
    // silently drop the tail frames of an unedited render.
    let timeline = spans(&project.edit_regions, frames.len() as f64 / info.fps as f64);
    let cuts = cuts(&timeline, frames.len(), info.fps as f64);
    let emit = emit_order(&cuts);
    ensure!(!emit.is_empty(), "every frame was trimmed");
    frame_budget(emit.len())?;
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

        // styled captions (burn the user's chosen subtitle design + the indexing-time focus
        // words into the export). Drawn on top of everything but the cursor FX.
        if let (Some(caps), Some(f)) = (captions.as_ref(), font.as_ref()) {
            draw_caption(&mut canvas, &layout, f, caps, t);
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

    encode(&fout, info.fps, &opts.format, out, video, &cuts)
}

/// Deletes a render's scratch dir when it drops — including on the `?` early-returns in
/// `beautify`, each of which would otherwise leak a full PNG dump of the clip.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// How long a scratch dir may exist before a later render treats it as abandoned. See
/// `sweep_stale_scratch` for why it is this generous.
const SCRATCH_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// Reclaim scratch dirs left behind by renders that died before `Scratch::drop` could run:
/// SIGKILL, OOM-kill, `docker stop`. Without this the per-render dir is an *unbounded* leak —
/// verified, a SIGKILLed render left 3.8 MB of PNGs that no later render ever reclaimed,
/// and this service does get OOM-killed. (The fixed dir it replaced was wiped on entry, so it
/// held at most one clip's frames however many renders had crashed; this restores that bound
/// without giving up the per-render isolation.)
///
/// Age is the discriminator, not PID liveness: a recycled PID would make a dead render's dir
/// look alive forever, and there is no portable way to ask "is that render still going". The
/// dir's own mtime is fixed at creation (frames are written into its `in`/`out` children), so
/// this means "started more than six hours ago" — orders of magnitude longer than any render
/// of any clip, and therefore never a *concurrent* render's dir.
fn sweep_stale_scratch(parent: &Path, older_than: std::time::Duration) {
    let Ok(rd) = std::fs::read_dir(parent) else { return };
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().starts_with("clipxd-beautify-") {
            continue;
        }
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > older_than);
        if stale {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// A kept span of *source* time and the rate it was *asked* to play back at; trimmed spans are
/// simply absent. Intermediate only: a rate is a real number but a span is emitted as a whole
/// number of frames, so what the picture actually did is a `Cut`, and that — not this — is what
/// the audio has to follow.
#[derive(Debug, Clone, PartialEq)]
struct Span {
    start: f64,
    end: f64,
    rate: f64,
}

/// Decompose the source timeline `[0, end)` into kept spans, honouring the project's `trim`
/// (drop) and `speed` (rate) regions. Cuts at every region boundary and classifies each slice
/// by its midpoint, which stays correct when the editor emits overlapping regions.
///
/// Adjacent slices at the same rate are merged, so twelve 1s regions all at 1.5× become one
/// 12s span — worth knowing when writing a fixture that means to exercise several spans.
fn spans(edits: &[EditRegionJ], end: f64) -> Vec<Span> {
    let mut bounds: Vec<f64> = vec![0.0, end];
    for e in edits {
        bounds.extend([e.start, e.end].into_iter().filter(|t| *t > 0.0 && *t < end));
    }
    bounds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let mut out: Vec<Span> = Vec::new();
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b - a < 1e-9 {
            continue;
        }
        let m = (a + b) / 2.0;
        // matches the original half-open trim / closed speed tests, so an unedited or
        // trim-only project keeps behaving exactly as it did
        if edits.iter().any(|e| e.kind == "trim" && m >= e.start && m < e.end) {
            continue;
        }
        let rate = edits
            .iter()
            .find(|e| e.kind == "speed" && m >= e.start && m <= e.end)
            // the project file is user-supplied JSON: rate 0 would divide by zero below and
            // rate 0.001 would expand one source frame into a thousand output frames
            .map(|e| e.rate.clamp(0.1, 100.0))
            .unwrap_or(1.0);
        match out.last_mut() {
            Some(l) if (l.end - a).abs() < 1e-9 && (l.rate - rate).abs() < 1e-9 => l.end = b,
            _ => out.push(Span { start: a, end: b, rate }),
        }
    }
    out
}

/// A kept span as the render actually resolved it: source frames `i0 .. i0 + n`, emitted as
/// `m` output frames. This is the single source of truth the picture *and* the sound both read.
///
/// `m` is the point. A span's requested rate is a real number, but the emitted length is a whole
/// number of frames, so the rate the video really played is `n / m`, not what was asked for.
/// Handing the audio the requested rate instead left every span up to half a frame out, always
/// in the same direction: a 12-region project measured 20 ms adrift at the first region and
/// 180 ms by the last. Driving `atempo` from `n / m` makes each span's audio exactly as long as
/// the frames that span emitted, so the error cannot accumulate.
#[derive(Debug, Clone, PartialEq)]
struct Cut {
    i0: usize,
    n: usize,
    m: usize,
}

impl Cut {
    /// The rate the picture *actually* played this span at.
    fn rate(&self) -> f64 {
        self.n as f64 / self.m as f64
    }
}

/// Resolve the requested timeline against the frames that were really extracted.
fn cuts(timeline: &[Span], frames: usize, fps: f64) -> Vec<Cut> {
    timeline
        .iter()
        .filter_map(|s| {
            // source frames whose timestamp falls in [start, end) — adjacent spans share a
            // boundary, so ceil on both ends tiles the timeline with no gap and no overlap
            let i0 = (s.start * fps).ceil().max(0.0) as usize;
            let i1 = ((s.end * fps).ceil() as usize).min(frames);
            let n = i1.saturating_sub(i0);
            // a span that rounds to zero output frames contributes nothing to either stream
            let m = (n as f64 / s.rate).round() as usize;
            (n > 0 && m > 0).then_some(Cut { i0, n, m })
        })
        .collect()
}

/// Output frame *j* ← source frame `emit_order(..)[j]`. Output fps stays constant; the rate
/// decides *which* source frame each output frame samples.
///
/// That choice is the whole fix. The old code kept every `rate.round().max(1.0)`-th source
/// frame, so 1.5× snapped to 2× and nothing below 1× could slow down at all. Sampling a
/// fractional source position instead makes 1.5× exactly 1.5× and lets rate < 1 repeat
/// frames. Integer rates still land on the same frames the modulo picked (2× → 0,2,4…), and
/// with no edits `m == n` and `j * 1.0 == j`, so an unedited render is frame-for-frame what
/// it was before this change.
///
/// Per-cut integer arithmetic, deliberately: accumulating `1.0/rate` across 180 frames at
/// 1.5× sums to 119.999…, one frame short.
fn emit_order(cuts: &[Cut]) -> Vec<usize> {
    let mut out = Vec::with_capacity(cuts.iter().map(|c| c.m).sum());
    for c in cuts {
        for j in 0..c.m {
            out.push(c.i0 + ((j as f64 * c.rate()) as usize).min(c.n - 1));
        }
    }
    out
}

/// Ceiling on how many frames ONE render may emit — 20 minutes of output at 30 fps, past any
/// clip this tool is for.
///
/// The bound has to be on the product, not on the rate. `spans` clamps `rate` at 0.1 and stops
/// there, so `m = round(n / rate)` lets a hand-written `.clipxd` ask for ten output frames per
/// source frame, and `emit_order` writes one full-resolution PNG per *output* frame. Measured at
/// 2560x1440: ~1.0 MB per frame across the two scratch dirs, so a 2 s clip at rate 0 (clamped to
/// 0.1) filled 92 MB in 45 s with 113 of its 600 frames written and no end in sight. The old
/// `rate.round().max(1.0)` could never emit more frames than it read; slow motion can.
const MAX_OUTPUT_FRAMES: usize = 36_000;

/// Refuse an over-budget timeline *before* the first output PNG is written, rather than filling
/// the disk and dying somewhere in the middle of the render.
fn frame_budget(emit: usize) -> Result<()> {
    ensure!(
        emit <= MAX_OUTPUT_FRAMES,
        "this edit would emit {emit} output frames, over the {MAX_OUTPUT_FRAMES}-frame render \
         limit ({} minutes at 30fps) — slow motion emits 1/rate frames per source frame (rate 0.1 \
         is 10x), so raise the rate or trim the clip",
        MAX_OUTPUT_FRAMES / (30 * 60)
    );
    Ok(())
}

/// `atempo` is documented for 0.5–2.0 per instance, so a rate outside that band is factored
/// into a chain of in-range steps (5× → 2 × 2 × 1.25). Rate 1.0 needs no filter at all.
fn atempo_chain(rate: f64) -> Vec<f64> {
    let (mut steps, mut r) = (Vec::new(), rate);
    while r > 2.0 {
        steps.push(2.0);
        r /= 2.0;
    }
    while r < 0.5 {
        steps.push(0.5);
        r *= 2.0;
    }
    if (r - 1.0).abs() > 1e-6 {
        steps.push(r);
    }
    steps
}

/// The `-filter_complex` that puts the source audio through the *same* cuts the picture got:
/// one `atrim` per kept cut (speed-ramped with `atempo`), concatenated back into `[aout]`.
/// Input 0 is the rendered PNG sequence, input 1 the source video — hence `[1:a]`.
///
/// The rendered video is authoritative — it is exactly `m` composited PNGs — so every cut is
/// fitted to its own `m / fps` seconds, padding with silence when the source runs out and
/// cutting when it runs long. Three shipped bugs live in that sentence:
///
///   * `-shortest` used to do the fitting, and it fits the *video* to the audio. A 6 s clip
///     with a 3 s track rendered 3 s (90 of 180 frames); with the 0.05 s track a suspended
///     `AudioContext` produces, it rendered TWO FRAMES — exit 0 both times.
///   * a cut whose source audio has already ended (trim keeping [2,6) of a 2 s track) yields an
///     *empty* `atrim`, and concat of an empty segment dropped the video stream from the muxed
///     output entirely — an audio-only file shipped as a successful render. `apad` makes it
///     silence of the right length instead.
///   * `asetpts=PTS-STARTPTS` threw away the audio stream's start offset, pulling the whole
///     soundtrack early by it (measured: 956 ms, every beep a flash early). Shifting by the
///     cut's own start instead and letting `aresample=…:first_pts=0` fill the leading gap with
///     silence keeps it — the source really was silent there. `asetpts` is still what lets
///     `concat` join cuts 2..n, it just no longer lies about where cut 1 began.
fn audio_filter(cuts: &[Cut], fps: f64) -> String {
    let single = cuts.len() == 1;
    let chains: Vec<String> = cuts
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let (a, b) = (c.i0 as f64 / fps, (c.i0 + c.n) as f64 / fps);
            let tempo: String = atempo_chain(c.rate()).iter().map(|r| format!(",atempo={r:.6}")).collect();
            let label = if single { "[aout]".to_string() } else { format!("[a{i}]") };
            format!(
                "[1:a]atrim=start={a:.6}:end={b:.6},asetpts=PTS-{a:.6}/TB,\
                 aresample=async=1:first_pts=0{tempo},apad,atrim=end={:.6}{label}",
                c.m as f64 / fps
            )
        })
        .collect();
    if single {
        return chains.join("");
    }
    let ins: String = (0..cuts.len()).map(|i| format!("[a{i}]")).collect();
    format!("{};{ins}concat=n={}:v=0:a=1[aout]", chains.join(";"), cuts.len())
}

/// Does the source carry an audio stream? A screen recording captured without a mic has
/// none, and pointing the filtergraph at `[1:a]` there makes ffmpeg abort the *whole* export
/// — turning a working (if silent) render into a failed one.
///
/// A `Result`, not a `bool`, deliberately. The `.is_ok_and(…)` this replaces folded three
/// different outcomes into `false`: no audio track, no `ffprobe` on PATH, and a container ffprobe
/// could not read (half-written, truncated). The last two then rendered the clip *silently* and
/// exited 0 — the same "succeeded, but the output is wrong and nothing said so" shape as the
/// `-shortest` truncation this file exists to have fixed. Absence is an answer; failing to ask
/// is not.
fn has_audio(video: &Path) -> Result<bool> {
    let o = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=index", "-of", "csv=p=0"])
        .arg(video)
        .output()
        .context("running ffprobe (is ffmpeg installed?)")?;
    ensure!(o.status.success(), "ffprobe could not read {} ({}): {}", video.display(), o.status, tail(&o.stderr, 500));
    Ok(!o.stdout.trim_ascii().is_empty())
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

/// Mux the rendered PNG sequence with the source's audio, put through the same edits.
///
/// Every export used to be silent: only the PNG pattern was ever handed to ffmpeg. GIF still
/// is (the container carries no audio), and so is a source with no audio track — see `has_audio`.
fn encode(frames_dir: &Path, fps: f32, format: &str, out: &Path, source: &Path, cuts: &[Cut]) -> Result<()> {
    let pattern = frames_dir.join("%05d.png");
    let mut c = Command::new("ffmpeg");
    c.args(["-y", "-framerate", &fps.to_string(), "-i"]).arg(&pattern);

    let audio = format != "gif" && !cuts.is_empty() && has_audio(source)?;
    if audio {
        c.arg("-i").arg(source);
        c.args(["-filter_complex", &audio_filter(cuts, fps as f64)]);
        c.args(["-map", "0:v", "-map", "[aout]"]);
    }
    match format {
        "gif" => {
            c.args(["-vf", "fps=15,scale=900:-1:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse"]);
        }
        "webm" => {
            c.args(["-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "32", "-pix_fmt", "yuv420p"]);
            if audio {
                c.args(["-c:a", "libopus"]);
            }
        }
        _ => {
            c.args(["-c:v", "libx264", "-pix_fmt", "yuv420p"]);
            if audio {
                c.args(["-c:a", "aac"]);
            }
        }
    }
    // Deliberately NO `-shortest`: it truncates whichever stream is longer, and the audio track
    // is the one that can be short (a 6 s clip with a 3 s track rendered 3 s, and the 0.05 s
    // track a suspended AudioContext produces rendered two frames). `audio_filter` already fits
    // the sound to the picture, so there is nothing left for `-shortest` to do but damage.
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
    /// Focus point in normalised source-frame coordinates (0..1, top-left origin) — the same
    /// pair `ZoomKeyframe` carries, and what `ZoomRegion` in `app/src/regions.ts` writes.
    cx: f64,
    cy: f64,
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
                .filter_map(|z| {
                    Some(ZoomRegionJ {
                        start: z["start"].as_f64()?,
                        end: z["end"].as_f64()?,
                        scale: z["scale"].as_f64()?,
                        // Optional and clamped: the project file is user-supplied JSON, and a
                        // region written before the editor had a focus control has neither key.
                        // 0.5 is the centre the renderer used to hardcode, so those still render
                        // exactly as they did.
                        cx: z["cx"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0),
                        cy: z["cy"].as_f64().unwrap_or(0.5).clamp(0.0, 1.0),
                    })
                })
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
/// Named presets — "aurora" (default), "dusk", "ocean", "violet", "noir", "mint" — or a hex
/// colour. Painted once per render, not per frame.
///
/// The base + blob colours mirror `CAMERA_BG_PRESETS` in `app/src/CameraConfig.ts` so the
/// swatch the user picks matches what gets baked in. A preset added there must be added here
/// *and* to `safe_bg` in `crates/clipxd-web/src/lib.rs`, which is what
/// `every_wallpaper_the_ui_offers_survives_safe_bg` guards — it reads the arms below by name,
/// so every preset the UI offers must appear as a named arm even when it shares the fallback's
/// palette. A preset that only reaches `_` renders Aurora and says nothing about it.
fn wallpaper(bg: &str, w: u32, h: u32) -> RgbaImage {
    if bg.starts_with('#') {
        return solid(w, h, hex(bg));
    }
    // base colour + radial colour blobs (fx, fy, radius_frac, rgb)
    type Palette = ([u8; 3], &'static [(f32, f32, f32, [u8; 3])]);
    // the signature look, and what anything unrecognised falls back to (matches the app's backdrop)
    const AURORA: Palette = (
        [8, 11, 22],
        &[(0.12, 0.1, 0.72, [60, 110, 230]), (0.88, 0.14, 0.66, [120, 80, 230]), (0.8, 0.92, 0.72, [40, 200, 160]), (0.1, 0.95, 0.66, [230, 90, 150])],
    );
    let (base, blobs): Palette = match bg {
        "noir" => ([10, 12, 16], &[(0.2, 0.1, 0.7, [40, 46, 60]), (0.85, 0.95, 0.7, [22, 26, 36])]),
        "dusk" => ([20, 16, 34], &[(0.15, 0.1, 0.75, [90, 70, 200]), (0.88, 0.18, 0.65, [205, 80, 150]), (0.7, 0.95, 0.7, [60, 90, 185])]),
        "ocean" => ([7, 18, 30], &[(0.18, 0.15, 0.75, [40, 120, 205]), (0.86, 0.9, 0.7, [30, 185, 175]), (0.6, 0.25, 0.55, [80, 90, 220])]),
        "violet" => ([16, 10, 28], &[(0.2, 0.2, 0.75, [130, 80, 235]), (0.85, 0.15, 0.65, [210, 90, 200]), (0.5, 0.95, 0.7, [90, 70, 215])]),
        // #06201c + #2bbf9a / #cdecc0 / #8ad7d0 — the "mint" entry in CameraConfig.ts
        "mint" => ([6, 32, 28], &[(0.22, 0.2, 0.72, [43, 191, 154]), (0.8, 0.82, 0.66, [205, 236, 192]), (0.7, 0.18, 0.55, [138, 215, 208])]),
        "aurora" | "gradient" => AURORA,
        _ => AURORA,
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

/// Run an ffmpeg command and, when it fails, say what it said.
///
/// This used to be `Stdio::null()` on both streams and `ensure!(…, "ffmpeg failed")`, so those two
/// words were the entire production signal for a broken `-filter_complex` — a string this file
/// builds by machine out of user-supplied JSON, whose shape nothing else records. The stderr tail
/// is where ffmpeg puts the actual complaint, and the command line is what makes it reproducible.
fn run(c: &mut Command) -> Result<()> {
    let cmd = format!("{c:?}");
    let o = c.stdout(Stdio::null()).output().with_context(|| format!("spawning {cmd}"))?;
    ensure!(o.status.success(), "ffmpeg failed ({}): {}\ncommand: {cmd}", o.status, tail(&o.stderr, 1500));
    Ok(())
}

/// The last `n` bytes of a child's stderr as text — ffmpeg prints the reason it died at the end,
/// after however many lines of banner.
fn tail(bytes: &[u8], n: usize) -> String {
    let s = String::from_utf8_lossy(bytes).trim().to_string();
    if s.len() <= n {
        return s;
    }
    let cut = (s.len() - n..s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(s.len());
    format!("…{}", &s[cut..])
}


// ---- styled captions (burned into the export) ---------------------------------
// Mirrors app/src/SubtitleStyle.tsx: 6 designs, per-word emphasis from the indexing-time
// Ollama pass (subtitle_emphasis), and the user's chosen style (subtitle_style). Drawn with
// the same single-color ab_glyph rasterizer the keystroke pills use, one word at a time so
// each word can carry its own emphasis colour.

struct Captions {
    transcript: Vec<clipxd_index::TranscriptSegment>,
    emphasis: Option<clipxd_index::SubtitleEmphasis>,
    style: SubtitleStyle,
}

/// Load `index.json` and pull out exactly what the caption layer needs. Returns `None` when
/// the user hasn't picked a design yet (`subtitle_style` absent) or there's no transcript —
/// in either case there's nothing to burn in.
fn load_captions(p: &Path) -> Option<Captions> {
    let idx: Index = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
    let style = idx.subtitle_style?;
    if idx.transcript.is_empty() {
        return None;
    }
    Some(Captions { transcript: idx.transcript, emphasis: idx.subtitle_emphasis, style })
}

/// The transcript segment active at `t` — the one that contains it. A segment with `end == 0`
/// is treated as open-ended (still active from its start). During a silence gap between
/// segments this returns `None`, so the burn path draws no caption rather than lingering
/// the previous one over dead air.
fn active_transcript<'a>(tx: &'a [clipxd_index::TranscriptSegment], t: f64) -> Option<&'a clipxd_index::TranscriptSegment> {
    for s in tx {
        if s.start <= t && (s.end == 0.0 || t <= s.end) {
            return Some(s);
        }
    }
    None
}

/// The emphasis segment matching `seg` — exact-ish start match first, then overlapping.
fn emphasis_seg<'a>(em: &'a clipxd_index::SubtitleEmphasis, seg: &clipxd_index::TranscriptSegment) -> Option<&'a clipxd_index::EmphasisSegment> {
    em.segments.iter().find(|s| (s.start - seg.start).abs() < 0.4).or_else(|| {
        em.segments
            .iter()
            .filter(|s| s.start <= seg.end && s.end >= seg.start)
            .min_by_key(|s| ((s.start - seg.start).abs() * 1000.0) as i64)
    })
}

fn draw_caption(canvas: &mut RgbaImage, layout: &FrameLayout, font: &ab_glyph::FontVec, caps: &Captions, t: f64) {
    let Some(seg) = active_transcript(&caps.transcript, t) else { return };
    let px = (layout.content_h as f32 * 0.05 * caps.style.font_scale).clamp(18.0, 56.0);
    let max_w = layout.content_w as f32 * 0.86;
    let space = text::text_width(font, px, " ");
    let em_seg = caps.emphasis.as_ref().and_then(|e| emphasis_seg(e, seg));
    let progress = if seg.end > seg.start {
        ((t - seg.start) / (seg.end - seg.start)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let words: Vec<&str> = seg.text.split_whitespace().collect();
    if words.is_empty() {
        return;
    }
    let n = words.len() as f64;

    // Decide each word's colour + (for boxed) whether to draw the bar.
    let word_color = |w: &str, i: usize| -> ([u8; 3], Option<[u8; 3]>) {
        let em = em_seg.and_then(|s| {
            let clean = w.to_lowercase();
            s.words.iter().find(|x| {
                let xc = x.text.to_lowercase();
                xc == clean || xc == clean.trim_matches(|c: char| c.is_ascii_punctuation())
            })
        });
        let kind = if caps.style.emphasis { em.map(|w| w.emphasis).unwrap_or(Emphasis::None) } else { Emphasis::None };
        let lit = caps.style.design == "karaoke" && (i as f64 / n) <= progress;
        match caps.style.design.as_str() {
            "bold" => match kind {
                Emphasis::Primary => ([255, 213, 74], None),
                Emphasis::Secondary => ([255, 255, 255], None),
                _ => ([236, 240, 246], None),
            },
            "karaoke" => match kind {
                Emphasis::Primary => ([255, 213, 74], None),
                _ if lit => ([255, 255, 255], None),
                _ => ([120, 120, 120], None),
            },
            "minimal" => match kind {
                Emphasis::Primary => ([10, 10, 10], None),
                _ => ([30, 30, 30], None),
            },
            "glow" => match kind {
                Emphasis::Primary => ([124, 249, 255], Some([24, 168, 255])),
                Emphasis::Secondary => ([255, 255, 255], Some([255, 255, 255])),
                _ => ([255, 255, 255], None),
            },
            // classic / boxed
            _ => match kind {
                Emphasis::Primary => ([255, 213, 74], None),
                Emphasis::Secondary => ([255, 255, 255], None),
                _ => ([255, 255, 255], None),
            },
        }
    };

    // Greedy wrap into lines (each a vec of word indices + widths), centered later.
    let mut lines: Vec<Vec<(usize, f32)>> = Vec::new();
    let mut cur: Vec<(usize, f32)> = Vec::new();
    let mut cur_w = 0.0_f32;
    for (i, w) in words.iter().enumerate() {
        let ww = text::text_width(font, px, w);
        let add = if cur.is_empty() { ww } else { space + ww };
        if cur_w + add > max_w && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            cur_w = ww;
            cur.push((i, ww));
        } else {
            cur_w += add;
            cur.push((i, ww));
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }

    let line_h = px * 1.28;
    let block_h = line_h * lines.len() as f32;
    let (cx_full, top_full) = (layout.content_x as f32 + layout.content_w as f32 / 2.0, layout.content_y as f32);
    let bottom_full = (layout.content_y + layout.content_h) as f32;
    let (mut y, _anchor_bottom) = match caps.style.position.as_str() {
        "top" => (top_full + px * 0.6, false),
        "center" => (top_full + layout.content_h as f32 / 2.0 - block_h / 2.0, false),
        _ => (bottom_full - block_h - px * 0.8, true),
    };
    // keep the band inside the content area
    y = y.max(top_full + 2.0).min(bottom_full - block_h - 2.0);

    let boxed = caps.style.design == "boxed";
    for line in &lines {
        let total: f32 = line.iter().map(|(_, w)| *w).sum::<f32>() + space * (line.len().saturating_sub(1)) as f32;
        let x0 = cx_full - total / 2.0;
        if boxed {
            let (bx, by, bw, bh) = (x0 - px * 0.5, y - px * 0.18, total + px, line_h);
            blend_rect(canvas, bx as i64, by as i64, bw as i64, bh as i64, [0, 0, 0, 150]);
        }
        let mut caret = x0;
        for &(i, ww) in line {
            let (color, glow) = word_color(words[i], i);
            if let Some(gc) = glow {
                for &(dx, dy) in &[(2.0_f32, 0.0), (-2.0, 0.0), (0.0, 2.0), (0.0, -2.0), (2.0, 2.0), (-2.0, -2.0), (2.0, -2.0), (-2.0, 2.0)] {
                    text::draw_text(canvas, caret + dx, y + dy, words[i], px, font, gc);
                }
            } else if caps.style.design == "classic" || caps.style.design == "bold" || caps.style.design == "karaoke" {
                // a soft shadow so the caption reads over any background
                text::draw_text(canvas, caret + 2.0, y + 2.0, words[i], px, font, [0, 0, 0]);
            }
            text::draw_text(canvas, caret, y, words[i], px, font, color);
            caret += ww + space;
        }
        y += line_h;
    }
}

#[cfg(test)]
mod timeline_tests {
    use super::*;

    fn edit(kind: &str, start: f64, end: f64, rate: f64) -> EditRegionJ {
        EditRegionJ { kind: kind.into(), start, end, rate }
    }

    /// 180 frames at 30fps — the 6.0s fixture `tools/render-audio-check.sh` renders.
    fn resolve(edits: &[EditRegionJ]) -> Vec<Cut> {
        cuts(&spans(edits, 6.0), 180, 30.0)
    }

    /// The regression that matters most: adding trim/speed support must not perturb the
    /// ordinary render. No edits ⇒ every source frame emitted once, in order.
    #[test]
    fn an_empty_edit_list_emits_every_frame_exactly_once() {
        assert_eq!(spans(&[], 6.0), vec![Span { start: 0.0, end: 6.0, rate: 1.0 }]);
        assert_eq!(resolve(&[]), vec![Cut { i0: 0, n: 180, m: 180 }]);
        assert_eq!(emit_order(&resolve(&[])), (0..180).collect::<Vec<_>>());
    }

    /// The bug: `rate.round().max(1.0)` turned 1.5 into 2, so a 1.5× ramp ran at double speed.
    /// 1.5× must emit two output frames per three source frames — no more, no less.
    #[test]
    fn a_1_5x_ramp_runs_at_exactly_1_5x() {
        let c = resolve(&[edit("speed", 0.0, 6.0, 1.5)]);
        let e = emit_order(&c);
        assert_eq!(e.len(), 120, "180 frames at 1.5x is 120 frames, not 90 (2x)");
        assert!(e.windows(2).all(|w| w[0] < w[1]), "frames must stay in order and unique");
    }

    /// The other half of the same bug: `.max(1.0)` meant nothing could ever slow down.
    #[test]
    fn a_half_speed_ramp_slows_down_by_repeating_frames() {
        let e = emit_order(&resolve(&[edit("speed", 0.0, 6.0, 0.5)]));
        assert_eq!(e.len(), 360);
        assert_eq!(&e[..4], &[0, 0, 1, 1]);
    }

    #[test]
    fn a_trim_drops_its_span_and_leaves_the_rest_untouched() {
        let t = spans(&[edit("trim", 2.0, 4.0, 1.0)], 6.0);
        assert_eq!(t, vec![Span { start: 0.0, end: 2.0, rate: 1.0 }, Span { start: 4.0, end: 6.0, rate: 1.0 }]);
        let e = emit_order(&cuts(&t, 180, 30.0));
        assert_eq!(e.len(), 120);
        assert!(!e.iter().any(|&i| (60..120).contains(&i)), "the trimmed frames must not appear");
    }

    #[test]
    fn a_speed_region_inside_a_trim_still_resolves_to_a_cut() {
        // overlapping regions are reachable from the editor; trim must win
        let t = spans(&[edit("trim", 1.0, 3.0, 1.0), edit("speed", 2.0, 4.0, 2.0)], 6.0);
        assert_eq!(t, vec![
            Span { start: 0.0, end: 1.0, rate: 1.0 },
            Span { start: 3.0, end: 4.0, rate: 2.0 },
            Span { start: 4.0, end: 6.0, rate: 1.0 },
        ]);
    }

    #[test]
    fn a_nonsense_rate_cannot_divide_by_zero_or_explode_the_frame_count() {
        // rate comes from a user-supplied .clipxd file, not from the editor's clamp
        assert_eq!(spans(&[edit("speed", 0.0, 6.0, 0.0)], 6.0)[0].rate, 0.1);
        assert_eq!(spans(&[edit("speed", 0.0, 6.0, -4.0)], 6.0)[0].rate, 0.1);

        // The clamp bounds the multiplier, not the product, and the product is what fills the
        // disk: rate 0.1 is ten full-resolution output PNGs per source frame. This assertion used
        // to stop at the 1800 below and call it correct — 10x, pinned as the intended behaviour.
        let boom = emit_order(&resolve(&[edit("speed", 0.0, 6.0, 0.0)]));
        assert_eq!(boom.len(), 1800, "180 source frames at the 0.1 floor is 10x");
        assert!(frame_budget(boom.len()).is_ok(), "6 s of slow motion is not what we mean to refuse");

        // the same edit on a real clip — 10 minutes at 30fps — is 180_000 output frames, and at
        // 1440p roughly 1 MB each. It must be refused, by a message that says what the limit is.
        let real = emit_order(&cuts(&spans(&[edit("speed", 0.0, 600.0, 0.0)], 600.0), 18_000, 30.0));
        assert!(real.len() > MAX_OUTPUT_FRAMES, "{} frames should be over budget", real.len());
        let err = frame_budget(real.len()).unwrap_err().to_string();
        assert!(err.contains(&MAX_OUTPUT_FRAMES.to_string()), "the refusal must name the limit: {err}");
    }

    /// `has_audio` used to be `.is_ok_and(…)`, which answered "no audio track" for a missing
    /// ffprobe and for a container it could not read — and a wrong "no" there renders the clip
    /// silent and exits 0. Absence must be distinguishable from failure to ask.
    #[test]
    fn a_probe_that_fails_is_an_error_not_an_answer_of_no_audio() {
        let p = std::env::temp_dir().join(format!("clipxd-hasaudio-{}.mp4", std::process::id()));
        std::fs::write(&p, b"this is not a container").unwrap();
        let r = has_audio(&p);
        let _ = std::fs::remove_file(&p);
        assert!(r.is_err(), "a file ffprobe cannot read must not answer \"no audio\": {r:?}");
    }

    #[test]
    fn atempo_chains_only_outside_0_5_to_2_0() {
        assert_eq!(atempo_chain(1.0), Vec::<f64>::new());
        assert_eq!(atempo_chain(1.5), vec![1.5]);
        assert_eq!(atempo_chain(2.0), vec![2.0]);
        assert_eq!(atempo_chain(0.5), vec![0.5]);
        assert_eq!(atempo_chain(4.0), vec![2.0, 2.0]);
        assert_eq!(atempo_chain(5.0), vec![2.0, 2.0, 1.25]);
        assert_eq!(atempo_chain(0.25), vec![0.5, 0.5]);
        // and the chain must multiply back to the requested rate, every step in ffmpeg's range
        for r in [0.1, 0.3, 0.75, 1.0, 1.5, 3.0, 7.0, 100.0] {
            let steps = atempo_chain(r);
            let got: f64 = steps.iter().product();
            assert!((got - r).abs() < 1e-9, "atempo_chain({r}) = {steps:?} multiplies to {got}");
            assert!(steps.iter().all(|s| (0.5..=2.0).contains(s)), "step out of ffmpeg's range for {r}: {steps:?}");
        }
    }

    #[test]
    fn the_audio_filter_follows_the_same_cuts_the_video_does() {
        // one cut → no concat wrapper needed
        let one = audio_filter(&resolve(&[]), 30.0);
        assert_eq!(
            one,
            "[1:a]atrim=start=0.000000:end=6.000000,asetpts=PTS-0.000000/TB,\
             aresample=async=1:first_pts=0,apad,atrim=end=6.000000[aout]"
        );

        // a trim → two atrims concatenated, so the audio loses exactly what the video lost
        let cut = audio_filter(&resolve(&[edit("trim", 2.0, 4.0, 1.0)]), 30.0);
        assert_eq!(
            cut,
            "[1:a]atrim=start=0.000000:end=2.000000,asetpts=PTS-0.000000/TB,\
             aresample=async=1:first_pts=0,apad,atrim=end=2.000000[a0];\
             [1:a]atrim=start=4.000000:end=6.000000,asetpts=PTS-4.000000/TB,\
             aresample=async=1:first_pts=0,apad,atrim=end=2.000000[a1];\
             [a0][a1]concat=n=2:v=0:a=1[aout]"
        );

        // a speed ramp → the same cut, sped up by atempo so it stays in sync
        let fast = audio_filter(&resolve(&[edit("speed", 0.0, 6.0, 1.5)]), 30.0);
        assert!(fast.contains("atempo=1.500000"), "{fast}");
        assert!(fast.ends_with("[aout]"), "{fast}");

        // out-of-range rate → chained atempo, never a single illegal one
        let vfast = audio_filter(&resolve(&[edit("speed", 0.0, 6.0, 4.0)]), 30.0);
        assert_eq!(vfast.matches("atempo=2.000000").count(), 2, "{vfast}");

        // atempo runs at the rate the picture ACTUALLY played, not the one that was asked for:
        // 180 frames at a requested 1.7 emit round(180/1.7) = 106 frames, i.e. 180/106 = 1.698113.
        // Handing atempo the raw 1.7 leaves this cut's audio 6ms short of its own frames, and
        // that shortfall is what accumulated into 180ms across a 12-region project.
        let odd = resolve(&[edit("speed", 0.0, 6.0, 1.7)]);
        assert_eq!(odd[0].m, 106);
        let f = audio_filter(&odd, 30.0);
        assert!(f.contains("atempo=1.698113"), "{f}");
        assert!(!f.contains("atempo=1.700000"), "atempo got the requested rate, not the emitted one: {f}");
    }

    /// `asetpts=PTS-STARTPTS` on the first cut discarded the audio stream's start offset, so a
    /// source whose audio begins at 0.978s played its whole soundtrack ~1s early — every beep
    /// one flash ahead of the flash it belonged to. The shift has to be the cut's own start.
    #[test]
    fn the_first_cut_shifts_by_its_own_start_not_by_the_first_sample() {
        let f = audio_filter(&resolve(&[]), 30.0);
        assert!(!f.contains("PTS-STARTPTS"), "STARTPTS throws away the stream's start offset: {f}");
        assert!(f.contains("asetpts=PTS-0.000000/TB"), "{f}");
        // and the leading gap it preserves must become real silence, not a hole
        assert!(f.contains("aresample=async=1:first_pts=0"), "{f}");
    }

    /// Every cut is fitted to its own emitted length — padded with silence when the source
    /// audio ran out, cut when it ran long. This is what replaced `-shortest`, which fitted the
    /// *video* to the audio and rendered a 6s clip with a 3s track as 3s.
    #[test]
    fn every_cut_is_padded_and_trimmed_to_the_frames_it_emitted() {
        for edits in [vec![], vec![edit("trim", 0.0, 2.0, 1.0)], vec![edit("speed", 0.0, 6.0, 1.7)]] {
            let c = resolve(&edits);
            let f = audio_filter(&c, 30.0);
            assert_eq!(f.matches("apad").count(), c.len(), "{f}");
            for cut in &c {
                assert!(f.contains(&format!("apad,atrim=end={:.6}", cut.m as f64 / 30.0)), "{f}");
            }
        }
    }

    /// Read back what `audio_filter` *actually emitted*, cut by cut: the source span it takes
    /// (`atrim=start=a:end=b`), the product of its `atempo` steps, and the length it fits that
    /// span to (`apad,atrim=end=fit`).
    ///
    /// Parsing the production string is the whole point. The two tests below used to recompute
    /// `atempo_chain(cut.rate())` and `cut.m` themselves, which makes the "audio" side
    /// `(n/fps) / (n/m) = m/fps` — exactly the video length it was being compared against. Both
    /// stayed green with the bug they are named for put back.
    ///
    /// Everything in the graph is formatted to 6 decimals, so comparisons against it get [`TOL`],
    /// not equality.
    fn chains(graph: &str) -> Vec<(f64, f64, f64, f64)> {
        fn num(s: &str) -> f64 {
            s.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect::<String>().parse().unwrap()
        }
        graph
            .split(';')
            .filter(|c| c.starts_with("[1:a]"))
            .map(|c| {
                let a = num(c.split("atrim=start=").nth(1).expect("no atrim start"));
                let b = num(c.split(":end=").nth(1).expect("no atrim end"));
                // no atempo at all (rate 1.0) is an empty product, i.e. 1.0 — the right identity
                let tempo: f64 = c.split("atempo=").skip(1).map(num).product();
                let fit = num(c.split("apad,atrim=end=").nth(1).expect("no apad,atrim fit"));
                (a, b, tempo, fit)
            })
            .collect()
    }

    /// 0.1 ms. Slack for the filtergraph's 6-decimal formatting only — the drift these tests
    /// exist for was 20 ms at the FIRST region and 180 ms by the last, i.e. 200x this and up.
    const TOL: f64 = 1e-4;

    /// The drift that shipped: `emit_order` quantised each span to whole frames while the audio
    /// got `atempo` at the *raw* requested rate, so every region contributed up to half a frame
    /// of permanent offset in the same direction. Measured on a 12-region render: audio led
    /// video by 20 ms at the first region and 180 ms by the last.
    ///
    /// Twelve regions with *distinct* rates — equal adjacent rates merge in `spans()`, which is
    /// how the first attempt at this fixture accidentally measured a single span and drifted 8ms.
    ///
    /// The sound comes out of the `audio_filter` string, the picture out of `emit_order`. Neither
    /// side is recomputed here, so driving `atempo` from anything but `n/m` goes red at cut 0.
    #[test]
    fn twelve_speed_regions_do_not_drift_audio_away_from_video() {
        let edits: Vec<EditRegionJ> =
            (0..12).map(|k| edit("speed", k as f64, k as f64 + 1.0, 1.70 + 0.01 * k as f64)).collect();
        let c = cuts(&spans(&edits, 12.0), 360, 30.0);
        assert_eq!(c.len(), 12, "adjacent regions must not merge here, or the test proves nothing");
        let g = chains(&audio_filter(&c, 30.0));
        assert_eq!(g.len(), 12, "one filter chain per cut");

        let (mut video, mut audio) = (0.0_f64, 0.0_f64);
        for (i, (&(a, b, tempo, fit), cut)) in g.iter().zip(&c).enumerate() {
            // what ffmpeg will produce for this chain: (b-a) seconds of source played at `tempo`
            audio += (b - a) / tempo;
            // what the renderer will write for this cut: the frames `emit_order` really pushes
            let frames = emit_order(std::slice::from_ref(cut)).len() as f64 / 30.0;
            video += frames;
            assert!((fit - frames).abs() < TOL, "cut {i} fits its audio to {fit}s but emits {frames}s of frames");
            assert!((audio - video).abs() < TOL, "drifted {:.0}ms by cut {i}", (audio - video) * 1000.0);
        }
    }

    /// Audio and video must come out the length the EDIT LIST asked for — a number worked out by
    /// hand here, not recomputed from the code under test. The picture is measured with
    /// `emit_order`, the sound with the `audio_filter` string.
    ///
    /// What this replaces compared `emit_order(&c).len()` against `sum(c.m)`; `emit_order` pushes
    /// exactly `c.m` entries per cut, so both sides were the video length and the "two
    /// independently-computed durations" in its doc comment did not exist. It stayed green with
    /// every cut's audio fitted to the wrong stream length.
    #[test]
    fn audio_and_video_timelines_agree_with_the_length_the_edits_ask_for() {
        // (edits, output frames they ask for) against the 180-frame / 6.0s / 30fps fixture
        let cases: Vec<(Vec<EditRegionJ>, usize)> = vec![
            (vec![], 180),                             // untouched
            (vec![edit("speed", 0.0, 6.0, 1.5)], 120), // 180 / 1.5
            (vec![edit("trim", 1.0, 2.0, 1.0)], 150),  // 180 - 30
            // 30 kept + 30 kept + 60 at 2x + 30 kept
            (vec![edit("trim", 1.0, 2.0, 1.0), edit("speed", 3.0, 5.0, 2.0)], 120),
            (vec![edit("speed", 0.0, 3.0, 0.5), edit("speed", 3.0, 6.0, 3.0)], 210), // 90/0.5 + 90/3
            // round(60/1.7) + round(60/2.3) + round(60/0.7) = 35 + 26 + 86
            (vec![edit("speed", 0.0, 2.0, 1.7), edit("speed", 2.0, 4.0, 2.3), edit("speed", 4.0, 6.0, 0.7)], 147),
        ];
        for (edits, want) in cases {
            let c = resolve(&edits);
            assert_eq!(emit_order(&c).len(), want, "picture: {c:?}");
            let sound: f64 = chains(&audio_filter(&c, 30.0)).iter().map(|&(_, _, _, fit)| fit).sum();
            assert!((sound - want as f64 / 30.0).abs() < TOL, "sound {sound}s vs {want} frames of picture");
        }
    }

    /// The focus point is user-supplied JSON and used to be ignored entirely (`kf.cx = 0.5`
    /// hardcoded), so two renders differing only in focus came out byte-identical.
    #[test]
    fn a_zoom_regions_focus_point_is_read_and_clamped() {
        let write = |json: &str| {
            let p = std::env::temp_dir().join(format!("clipxd-zoomtest-{}.json", std::process::id()));
            std::fs::write(&p, json).unwrap();
            let z = load_project(&p).unwrap().zoom_regions;
            let _ = std::fs::remove_file(&p);
            z
        };

        let z = write(r#"{"zoom_regions":[{"start":0,"end":1,"scale":2,"cx":0.1,"cy":0.9}]}"#);
        assert_eq!((z[0].cx, z[0].cy), (0.1, 0.9));

        // a project written before the editor had a focus control keeps the old centred crop
        let old = write(r#"{"zoom_regions":[{"start":0,"end":1,"scale":2}]}"#);
        assert_eq!((old[0].cx, old[0].cy), (0.5, 0.5));

        // out-of-range values would sample off-frame
        let wild = write(r#"{"zoom_regions":[{"start":0,"end":1,"scale":2,"cx":-3,"cy":42}]}"#);
        assert_eq!((wild[0].cx, wild[0].cy), (0.0, 1.0));
    }

    /// The per-render scratch dir is only reclaimed by `Scratch::drop`, which SIGKILL and
    /// OOM-kill skip; `sweep_stale_scratch` is what bounds the leak. It must not touch a
    /// concurrent render's dir, which is why the discriminator is age.
    #[test]
    fn the_scratch_sweep_reclaims_old_dirs_and_spares_live_ones() {
        use std::time::Duration;
        let root = std::env::temp_dir().join(format!("clipxd-sweeptest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (mine, theirs, other) =
            (root.join("clipxd-beautify-1-1"), root.join("clipxd-beautify-2-2"), root.join("not-ours"));
        for d in [&mine, &theirs, &other] {
            std::fs::create_dir_all(d.join("in")).unwrap();
        }

        // a concurrent render's dir is younger than the cutoff and must survive
        sweep_stale_scratch(&root, Duration::from_secs(6 * 3600));
        assert!(mine.exists() && theirs.exists(), "the sweep ate a live render's scratch dir");

        // an abandoned one is older than the cutoff and must go — with its contents
        sweep_stale_scratch(&root, Duration::ZERO);
        assert!(!mine.exists() && !theirs.exists(), "the sweep left a dead render's frames behind");
        assert!(other.exists(), "the sweep must only touch clipxd-beautify-* dirs");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod caption_tests {
    use super::*;
    use clipxd_index::{EmphasisSegment, EmphasisWord, Index, Metadata, Source, SubtitleEmphasis, SubtitleStyle, TranscriptSegment};

    fn meta() -> Metadata {
        Metadata {
            duration: 10.0, resolution: [100, 100], fps: 30.0, created_at: "0".into(),
            title: "t".into(), description: String::new(), app_focus: vec![], url_context: None, has_video: true,
        }
    }

    fn seg(start: f64, end: f64, text: &str) -> TranscriptSegment {
        TranscriptSegment { start, end, speaker: None, text: text.into() }
    }

    #[test]
    fn active_transcript_returns_the_containing_segment() {
        let tx = [seg(0.0, 1.0, "first"), seg(2.0, 4.0, "second"), seg(5.0, 7.0, "third")];
        assert_eq!(active_transcript(&tx, 3.0).unwrap().text, "second");
        assert_eq!(active_transcript(&tx, 6.5).unwrap().text, "third");
        assert_eq!(active_transcript(&tx, 1.5), None); // gap — no active segment
    }

    #[test]
    fn active_transcript_falls_back_to_last_started_when_no_end() {
        let tx = [seg(0.0, 0.0, "open ended")];
        assert_eq!(active_transcript(&tx, 5.0).unwrap().text, "open ended");
    }

    #[test]
    fn emphasis_seg_matches_by_start_then_overlap() {
        let em = SubtitleEmphasis {
            generated_by: "ollama".into(), generated_at: "0".into(),
            segments: vec![
                EmphasisSegment { start: 0.0, end: 1.0, words: vec![EmphasisWord { text: "first".into(), emphasis: Emphasis::Primary }] },
                EmphasisSegment { start: 2.2, end: 4.2, words: vec![EmphasisWord { text: "second".into(), emphasis: Emphasis::Primary }] },
            ],
        };
        // exact-ish start
        let s = seg(2.0, 4.0, "second segment");
        assert_eq!(emphasis_seg(&em, &s).unwrap().start, 2.2);
        // overlap fallback when no start is close
        let s2 = seg(3.5, 5.0, "drifted");
        assert_eq!(emphasis_seg(&em, &s2).unwrap().start, 2.2);
    }

    #[test]
    fn load_captions_requires_a_style_and_a_transcript() {
        let tmp = std::env::temp_dir().join(format!("clipxd-cap-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        // no style + no transcript -> None
        let mut idx = Index::new("clp_1", Source::Screen, meta());
        std::fs::write(tmp.join("a.json"), serde_json::to_string(&idx).unwrap()).unwrap();
        assert!(load_captions(&tmp.join("a.json")).is_none());

        // style set but no transcript -> None
        idx.subtitle_style = Some(SubtitleStyle::default());
        std::fs::write(tmp.join("b.json"), serde_json::to_string(&idx).unwrap()).unwrap();
        assert!(load_captions(&tmp.join("b.json")).is_none());

        // style + transcript -> Some
        idx.transcript.push(seg(0.0, 1.0, "hello world"));
        std::fs::write(tmp.join("c.json"), serde_json::to_string(&idx).unwrap()).unwrap();
        let c = load_captions(&tmp.join("c.json")).unwrap();
        assert_eq!(c.transcript.len(), 1);
        assert_eq!(c.style.design, "classic");
        assert!(c.emphasis.is_none());

        // with emphasis too
        idx.subtitle_emphasis = Some(SubtitleEmphasis {
            generated_by: "ollama".into(), generated_at: "0".into(),
            segments: vec![EmphasisSegment { start: 0.0, end: 1.0, words: vec![EmphasisWord { text: "hello".into(), emphasis: Emphasis::Primary }] }],
        });
        std::fs::write(tmp.join("d.json"), serde_json::to_string(&idx).unwrap()).unwrap();
        let c2 = load_captions(&tmp.join("d.json")).unwrap();
        assert!(c2.emphasis.is_some());

        std::fs::remove_dir_all(&tmp).ok();
    }
}
