//! Optional Tier-2 "deep pass" over a finished clip — **off by default, opt-in by env**.
//!
//! The default pipeline is local-first (oar-ocr/PaddleOCR + Moondream on the box) and stays
//! that way. This pass exists to synthesize what per-frame enrichment can't: a narrative
//! title, a real tl;dr, and timestamped chapters. It runs on the **text** the pipeline
//! already produced — timestamped OCR spans, scene captions, and transcript — not the raw
//! video. (An earlier version uploaded the whole video to Gemini's Files API; that worked
//! but cost a second video transfer + a 300s upload-and-poll cycle for information the index
//! had *already extracted*. Measured: text-context calls land in 5-20s vs. video-upload's
//! 17-39s, and the JSON output was equivalent quality on the recording it was tested against.)
//! It never runs on the request path: `spawn_phase2` fires it after enrichment, and every
//! failure is logged-and-swallowed (the clip is already complete without it).
//!
//! **Two backends, primary + fallback — not a choice you make:**
//! 1. **NVIDIA NIM** (`NVIDIA_API_KEY`) — free-tier hosted inference (Kimi K2 by default; see
//!    `CLIPXD_NVIDIA_MODEL`). No published per-token price as of 2026-07 (confirmed against
//!    `docs.nvidia.com/nim` — no pricing page exists for the hosted endpoint), so it is
//!    explicitly *not* the thing to depend on for guaranteed uptime or cost. Tried first
//!    because it's free right now and was faster/equal quality in testing.
//! 2. **Gemini** (`GEMINI_API_KEY`, model `CLIPXD_GEMINI_MODEL`, default
//!    `gemini-3.1-flash-lite`) — a real, published, stable price
//!    ($0.25/M in, $1.50/M out — ai.google.dev/gemini-api/docs/pricing, confirmed 2026-07).
//!    Used whenever NVIDIA is unset, or fails for *any* reason (down, rate-limited, pricing
//!    changed, model retired) — the fallback exists so a free tier disappearing overnight
//!    doesn't silently turn this feature off.
//!
//! Enable with `CLIPXD_DEEP_PASS=1` and at least one of `NVIDIA_API_KEY` / `GEMINI_API_KEY`.

use anyhow::{anyhow, bail, Context, Result};
use clipxd_index::Index;
use std::path::Path;

pub fn enabled() -> bool {
    let on = std::env::var("CLIPXD_DEEP_PASS").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false);
    on && (has_env("NVIDIA_API_KEY") || has_env("GEMINI_API_KEY"))
}

fn has_env(key: &str) -> bool {
    std::env::var(key).map(|v| !v.is_empty()).unwrap_or(false)
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
/// Tries NVIDIA first (if configured), then Gemini on any NVIDIA failure or absence. Merge
/// rules are conservative: the title is only set while it's still the recorder's default
/// (never stomp a user edit), tl;dr/chapters only when the model returned something.
pub async fn run(clip_dir: &Path, id: &str) -> Result<()> {
    let index_path = clip_dir.join("index.json");
    let idx: Index = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;
    let context = build_context(&idx);
    if context.trim().is_empty() {
        bail!("no transcript/OCR/captions yet to summarize");
    }
    let prompt = format!("{PROMPT_PREFIX}{context}");

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build()?;

    let mut used = "none";
    let mut result: Option<Result<DeepResult>> = None;
    if has_env("NVIDIA_API_KEY") {
        let r = call_nvidia(&client, &prompt).await;
        if let Err(e) = &r {
            eprintln!("deep pass: NVIDIA backend failed for {id}, falling back to Gemini: {e:#}");
        } else {
            used = "nvidia";
        }
        result = Some(r);
    }
    if result.as_ref().is_none_or(|r| r.is_err()) && has_env("GEMINI_API_KEY") {
        let r = call_gemini(&client, &prompt).await;
        if r.is_ok() {
            used = "gemini";
        }
        result = Some(r);
    }
    let deep = result.ok_or_else(|| anyhow!("no deep-pass backend configured"))??;

    merge_into_index(clip_dir, &deep)?;
    eprintln!("deep pass ({used}): merged title/tldr/{} chapters for {id}", deep.chapters.len());
    Ok(())
}

/// Flatten the index's transcript + scene captions + OCR spans into one timestamp-ordered
/// text block — the same shape proved out in manual testing (transcript/captions/OCR
/// interleaved by `t`, one line each). Deliberately excludes raw event_track/search fields:
/// those are noisy relative to their token cost for this specific synthesis task.
fn build_context(idx: &Index) -> String {
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

fn nvidia_model() -> String {
    // kimi-k2.6 was fastest and highest quality of the three NVIDIA-hosted Chinese frontier
    // models tested (kimi-k2.6, minimax-m3, qwen3.5-122b-a10b) on this task — see project memory.
    std::env::var("CLIPXD_NVIDIA_MODEL").ok().filter(|m| !m.is_empty()).unwrap_or_else(|| "moonshotai/kimi-k2.6".into())
}

async fn call_nvidia(client: &reqwest::Client, prompt: &str) -> Result<DeepResult> {
    let key = std::env::var("NVIDIA_API_KEY").context("NVIDIA_API_KEY")?;
    let body = serde_json::json!({
        "model": nvidia_model(),
        "messages": [{ "role": "user", "content": prompt }],
        "temperature": 0.3,
        "max_tokens": 1024,
    });
    let resp = client
        .post("https://integrate.api.nvidia.com/v1/chat/completions")
        .bearer_auth(&key)
        .json(&body)
        .send()
        .await
        .context("nvidia request")?;
    let status = resp.status();
    let out: serde_json::Value = resp.json().await.context("nvidia response")?;
    if !status.is_success() {
        bail!("nvidia {status}: {}", out["error"]["message"].as_str().or_else(|| out["detail"].as_str()).unwrap_or("?"));
    }
    let text = out["choices"][0]["message"]["content"].as_str().ok_or_else(|| anyhow!("nvidia response had no message content"))?;
    parse_deep_json(text)
}

fn gemini_model() -> String {
    // gemini-2.5-flash-lite returned repeated 503 "high demand" errors in testing (2026-07) --
    // likely deprioritized capacity now that newer generations exist. gemini-3.1-flash-lite is
    // the current stable cheap/fast tier and has a real published price.
    std::env::var("CLIPXD_GEMINI_MODEL").ok().filter(|m| !m.is_empty()).unwrap_or_else(|| "gemini-3.1-flash-lite".into())
}

async fn call_gemini(client: &reqwest::Client, prompt: &str) -> Result<DeepResult> {
    let key = std::env::var("GEMINI_API_KEY").context("GEMINI_API_KEY")?;
    let body = serde_json::json!({
        "contents": [{ "parts": [{ "text": prompt }] }],
        "generationConfig": { "responseMimeType": "application/json" },
    });
    let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", gemini_model());
    let resp = client.post(&url).header("x-goog-api-key", &key).json(&body).send().await.context("gemini request")?;
    let status = resp.status();
    let out: serde_json::Value = resp.json().await.context("gemini response")?;
    if !status.is_success() {
        bail!("gemini {status}: {}", out["error"]["message"].as_str().unwrap_or("?"));
    }
    let text = out["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or_else(|| anyhow!("gemini response had no text part"))?;
    parse_deep_json(text)
}

/// Both backends are asked for JSON-only output, but chat-completion-style models sometimes
/// wrap it in a ```json fence anyway — strip that before parsing rather than failing on it.
fn parse_deep_json(text: &str) -> Result<DeepResult> {
    let cleaned = text.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
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
