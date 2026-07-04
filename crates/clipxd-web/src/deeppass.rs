//! Optional Tier-2 "deep pass" over a finished clip — **off by default, opt-in by env**.
//!
//! The default pipeline is local-first (oar-ocr/PaddleOCR + Moondream on the box) and stays
//! that way. This pass exists to synthesize what per-frame enrichment can't: a narrative
//! title, a real tl;dr, and timestamped chapters. It runs on the **text** the pipeline
//! already produced — timestamped OCR spans, scene captions, and transcript — not the raw
//! video, via the shared [`crate::llm`] NVIDIA/Gemini-fallback primitive. It never runs on the
//! request path: `spawn_phase2` fires it after enrichment, and every failure is
//! logged-and-swallowed (the clip is already complete without it).
//!
//! Enable with `CLIPXD_DEEP_PASS=1` and at least one of `NVIDIA_API_KEY` / `GEMINI_API_KEY`.

use crate::llm;
use anyhow::{bail, Context, Result};
use clipxd_index::Index;
use std::path::Path;

pub fn enabled() -> bool {
    let on = std::env::var("CLIPXD_DEEP_PASS").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false);
    on && llm::any_backend_configured()
}

/// Auto-title generation, unlike the rest of the deep pass, runs unconditionally (no
/// `CLIPXD_DEEP_PASS` gate) — it's genuinely cheap: one short prompt, plain-text answer, no
/// JSON round trip. Every clip deserves a real name instead of sitting in the library as
/// "Screen recording" forever; the fuller tldr/chapters synthesis stays opt-in since it costs
/// more (bigger prompt, structured output) for something the library list doesn't show anyway.
const TITLE_PROMPT_PREFIX: &str = "You are naming a screen recording so it's identifiable in a list of clips, \
without watching the video. Below is the recording's already-extracted index: timestamped scene captions, \
on-screen OCR text, and any transcribed speech. Reply with ONLY the title — one specific, concrete line (6-10 \
words) naming what actually happens, no quotes, no markdown, no trailing period, no commentary before or after.\
\n\nINDEX DATA:\n";

/// Generate a title for the clip in `clip_dir` and merge it in, but only while the title is
/// still the recorder's generic default (never stomp a user rename, and never re-spend a call
/// on a clip that already got one — e.g. from a later, opt-in full deep pass).
pub async fn generate_title(clip_dir: &Path, id: &str) -> Result<()> {
    let index_path = clip_dir.join("index.json");
    let idx: Index = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;
    if idx.metadata.title != "Screen recording" {
        return Ok(()); // already renamed (by a user or an earlier pass) — nothing to do
    }
    if !llm::any_backend_configured() {
        bail!("no LLM backend configured");
    }
    let context = build_context(&idx);
    if context.trim().is_empty() {
        bail!("no transcript/OCR/captions yet to title from");
    }
    let prompt = format!("{TITLE_PROMPT_PREFIX}{context}");
    let (text, used) = llm::complete(&prompt, false).await?;
    let title = llm::strip_fence(&text).trim().trim_matches('"').to_string();
    if title.is_empty() {
        bail!("model returned an empty title");
    }

    let mut index: Index = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;
    if index.metadata.title == "Screen recording" {
        index.metadata.title = title.clone();
        std::fs::write(&index_path, serde_json::to_string_pretty(&index)?)?;
    }
    eprintln!("auto-title ({used}): \"{title}\" for {id}");
    Ok(())
}

/// What the deep pass asks for — matches the fields it is allowed to merge into the index.
#[derive(serde::Deserialize)]
struct DeepResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    tldr: String,
    #[serde(default)]
    chapters: Vec<DeepChapter>,
}

#[derive(serde::Deserialize)]
struct DeepChapter {
    #[serde(default)]
    start: f64,
    #[serde(default)]
    title: String,
}

const PROMPT_PREFIX: &str = "You are indexing a screen recording so software agents can answer questions about it \
without watching it. Below is the recording's already-extracted index: timestamped scene captions (from a \
vision model looking at keyframes), timestamped on-screen OCR text, and any transcribed speech. Synthesize \
this into JSON only (no markdown fences, no commentary), shaped exactly as \
{\"title\": string, \"tldr\": string, \"chapters\": [{\"start\": number, \"title\": string}]}. \
`title`: one specific, concrete line naming what the recording is about. `tldr`: 2-4 sentences narrating \
what happens in order, naming visible apps, actions, and any errors verbatim. `chapters`: 3-8 entries; \
`start` is the chapter's first moment in seconds from the beginning.\n\nINDEX DATA:\n";

/// Run the deep pass for the clip in `clip_dir` and merge the result into its `index.json`.
/// Merge rules are conservative: the title is only set while it's still the recorder's
/// default (never stomp a user edit), tl;dr/chapters only when the model returned something.
pub async fn run(clip_dir: &Path, id: &str) -> Result<()> {
    let index_path = clip_dir.join("index.json");
    let idx: Index = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;
    let context = build_context(&idx);
    if context.trim().is_empty() {
        bail!("no transcript/OCR/captions yet to summarize");
    }
    let prompt = format!("{PROMPT_PREFIX}{context}");

    let (text, used) = llm::complete(&prompt, true).await?;
    let deep = parse_deep_json(&text)?;

    merge_into_index(clip_dir, &deep)?;
    eprintln!("deep pass ({used}): merged title/tldr/{} chapters for {id}", deep.chapters.len());
    Ok(())
}

/// Flatten the index's transcript + scene captions + OCR spans into one timestamp-ordered
/// text block — the shared input every LLM-over-the-index feature (deep pass, doc generation)
/// builds its prompt from. Deliberately excludes raw event_track/search fields: those are
/// noisy relative to their token cost for text-synthesis tasks.
pub(crate) fn build_context(idx: &Index) -> String {
    let mut lines: Vec<(f64, String)> = Vec::new();
    for seg in &idx.transcript {
        lines.push((seg.start, format!("[{:.1}s] speech: {}", seg.start, seg.text)));
    }
    for m in &idx.visual_timeline {
        lines.push((m.t, format!("[{:.1}s] caption: {}", m.t, m.caption)));
    }
    for t in &idx.on_screen_text {
        lines.push((t.start, format!("[{:.1}s] on-screen text: {}", t.start, t.text)));
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    lines.into_iter().map(|(_, l)| l).collect::<Vec<_>>().join("\n")
}

fn parse_deep_json(text: &str) -> Result<DeepResult> {
    let cleaned = llm::strip_fence(text);
    serde_json::from_str(cleaned).with_context(|| format!("deep-pass JSON parse: {cleaned:.200}"))
}

fn merge_into_index(clip_dir: &Path, deep: &DeepResult) -> Result<()> {
    let path = clip_dir.join("index.json");
    let mut index: Index = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let title = deep.title.trim();
    if !title.is_empty() && (index.metadata.title.is_empty() || index.metadata.title == "Screen recording") {
        index.metadata.title = title.to_string();
    }
    if !deep.tldr.trim().is_empty() {
        index.summary.tldr = deep.tldr.trim().to_string();
    }
    if !deep.chapters.is_empty() {
        index.summary.chapters = deep
            .chapters
            .iter()
            .filter(|c| !c.title.trim().is_empty())
            .map(|c| clipxd_index::Chapter { start: c.start.max(0.0), title: c.title.trim().to_string() })
            .collect();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&index)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn generate_title_skips_a_clip_already_renamed() {
        use clipxd_index::{Metadata, Source};
        let tmp = std::env::temp_dir().join(format!("clipxd-titlegen-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut idx = Index::new(
            "clp_1",
            Source::Screen,
            Metadata {
                duration: 10.0,
                resolution: [100, 100],
                fps: 30.0,
                created_at: "0".into(),
                title: "Already renamed by the user".into(),
                app_focus: vec![],
                url_context: None,
                has_video: true,
            },
        );
        idx.summary.tldr = "not empty, so build_context wouldn't otherwise bail".into();
        std::fs::write(tmp.join("index.json"), serde_json::to_string(&idx).unwrap()).unwrap();

        // No NVIDIA_API_KEY/GEMINI_API_KEY needed — the title-already-set guard must return Ok
        // before any backend check, regardless of environment.
        let result = generate_title(&tmp, "clp_1").await;
        assert!(result.is_ok(), "{result:?}");
        let after: Index = serde_json::from_str(&std::fs::read_to_string(tmp.join("index.json")).unwrap()).unwrap();
        assert_eq!(after.metadata.title, "Already renamed by the user");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn parse_deep_json_strips_markdown_fence() {
        let fenced = "```json\n{\"title\":\"t\",\"tldr\":\"d\",\"chapters\":[]}\n```";
        let d = parse_deep_json(fenced).unwrap();
        assert_eq!(d.title, "t");
        assert_eq!(d.tldr, "d");
    }

    #[test]
    fn parse_deep_json_accepts_bare_json() {
        let d = parse_deep_json(r#"{"title":"a","tldr":"b","chapters":[{"start":1.0,"title":"c"}]}"#).unwrap();
        assert_eq!(d.chapters.len(), 1);
    }

    #[test]
    fn build_context_orders_by_timestamp_across_streams() {
        use clipxd_index::{Metadata, OnScreenText, Source, TextKind, VisualMoment};
        let mut idx = Index::new(
            "clp_1",
            Source::Screen,
            Metadata {
                duration: 10.0,
                resolution: [100, 100],
                fps: 30.0,
                created_at: "0".into(),
                title: "t".into(),
                app_focus: vec![],
                url_context: None,
                has_video: true,
            },
        );
        idx.visual_timeline.push(VisualMoment { t: 5.0, salience: 1.0, caption: "later".into(), delta: "d".into(), frame_ref: None });
        idx.on_screen_text.push(OnScreenText { start: 1.0, end: 1.0, text: "earlier".into(), source: TextKind::Ocr, bbox: None });
        let ctx = build_context(&idx);
        let earlier_pos = ctx.find("earlier").unwrap();
        let later_pos = ctx.find("later").unwrap();
        assert!(earlier_pos < later_pos, "context should be timestamp-ordered: {ctx}");
    }
}
