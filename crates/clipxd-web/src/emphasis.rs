//! Indexing-time caption-emphasis pass — decides *which words to focus on* for the styled
//! subtitle designs (Karaoke highlight, Bold weighting).
//!
//! Runs **only inside `spawn_phase2`** (Phase 2 enrichment), never on the request path, right
//! after the auto-title pass. It sends the transcript to the shared [`crate::llm`] primitive
//! (Ollama Cloud first, NVIDIA/Gemini fallback — same BYOK path as the deep pass, so a user's
//! analysis bills to their own Ollama key) and writes a new optional
//! [`clipxd_index::SubtitleEmphasis`] into `index.json`. The pass is gated on
//! `llm::any_backend_configured()` exactly like auto-title: off locally with no key, on with a
//! server- or owner-supplied one. Every failure is logged-and-swallowed — the clip is already
//! complete without it, so a failed/degenerate emphasis reply never degrades the index.
//!
//! This is the literal answer to "the LLM analysis should happen during indexing, not before":
//! indexing = `spawn_phase2`; this is one more log-and-swallow step inside it.

use crate::llm;
use anyhow::{bail, Context, Result};
use clipxd_index::{Emphasis, EmphasisSegment, EmphasisWord, Index, SubtitleEmphasis};
use std::path::Path;

const EMPHASIS_PROMPT_PREFIX: &str = "You are preparing styled subtitles for a screen recording. Below is the recording's transcribed speech, split into timestamped segments. For each segment, mark which words should be EMPHASIZED when shown as a caption so a viewer's eye is drawn to the meaning, not the filler. Reply with ONLY JSON (no markdown fences, no commentary), shaped exactly as {\"segments\": [{\"start\": number, \"end\": number, \"words\": [{\"text\": string, \"emphasis\": \"primary\"|\"secondary\"|\"none\"}]}]}. Rules: keep the SAME number of segments as the input. `words` must cover the segment's spoken text, split on whitespace, preserving the original words verbatim. Mark at most ~2 `primary` words per segment (the ones carrying the key meaning — nouns, verbs, errors, numbers); mark a few more `secondary`; everything else `none`. Do not invent words that are not in the input. Do not add commentary.\n\nTRANSCRIPT:\n";

/// A parsed emphasis reply — one `{segments:[…]}` object.
#[derive(serde::Deserialize)]
struct EmphasisResult {
    #[serde(default)]
    segments: Vec<EmphasisSegmentIn>,
}

#[derive(serde::Deserialize)]
struct EmphasisSegmentIn {
    #[serde(default)]
    start: f64,
    #[serde(default)]
    end: f64,
    #[serde(default)]
    words: Vec<EmphasisWordIn>,
}

#[derive(serde::Deserialize)]
struct EmphasisWordIn {
    #[serde(default)]
    text: String,
    #[serde(default)]
    emphasis: String,
}

/// Run the emphasis pass for the clip in `clip_dir` and merge the result into its
/// `index.json` as `subtitle_emphasis`.
///
/// `nvidia_key`/`gemini_key` are the clip owner's own BYOK keys (looked up by the caller via
/// `Db::llm_keys`), if any — `None` falls back to the server's env-configured keys. Ollama
/// Cloud has no BYOK override yet (server-env `OLLAMA_API_KEY` only), matching the deep pass.
pub async fn run(clip_dir: &Path, id: &str, nvidia_key: Option<&str>, gemini_key: Option<&str>) -> Result<()> {
    let index_path = clip_dir.join("index.json");
    let idx: Index = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;
    if idx.transcript.is_empty() {
        bail!("no transcript yet to emphasize");
    }
    if nvidia_key.is_none() && gemini_key.is_none() && !llm::any_backend_configured() {
        bail!("no LLM backend configured");
    }
    let prompt = format!("{EMPHASIS_PROMPT_PREFIX}{}", transcript_block(&idx));
    let (text, used) = llm::complete_with_keys(&prompt, true, nvidia_key, gemini_key).await?;
    let parsed = parse_emphasis_json(&text)?;

    // Drop anything the model returned that we can't honestly align (no words, NaN times,
    // degenerate). Capping the segment count to the transcript's own length keeps a runaway
    // reply from bloating the index.
    let n = idx.transcript.len();
    let segments: Vec<EmphasisSegment> = parsed
        .segments
        .into_iter()
        .take(n)
        .map(|s| EmphasisSegment {
            start: if s.start.is_finite() { s.start.max(0.0) } else { 0.0 },
            end: if s.end.is_finite() { s.end.max(s.start) } else { s.start },
            words: s
                .words
                .into_iter()
                .filter(|w| !w.text.trim().is_empty())
                .map(|w| EmphasisWord {
                    text: w.text.trim().to_string(),
                    emphasis: parse_emphasis(&w.emphasis),
                })
                .collect(),
        })
        .filter(|s| !s.words.is_empty())
        .collect();
    if segments.is_empty() {
        bail!("model returned no usable emphasis segments");
    }

    let mut index: Index = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;
    index.subtitle_emphasis = Some(SubtitleEmphasis {
        generated_by: used.to_string(),
        generated_at: unix_rfc3339(),
        segments,
    });
    std::fs::write(&index_path, serde_json::to_string_pretty(&index)?)?;
    eprintln!("emphasis pass ({used}): {} segments for {id}", index.subtitle_emphasis.as_ref().unwrap().segments.len());
    Ok(())
}

fn parse_emphasis(s: &str) -> Emphasis {
    match s.trim().to_ascii_lowercase().as_str() {
        "primary" => Emphasis::Primary,
        "secondary" => Emphasis::Secondary,
        _ => Emphasis::None,
    }
}

fn parse_emphasis_json(text: &str) -> Result<EmphasisResult> {
    let cleaned = llm::strip_fence(text);
    serde_json::from_str(cleaned).with_context(|| format!("emphasis JSON parse: {cleaned:.200}"))
}

/// Flatten the transcript into the per-segment block the prompt expects. Kept separate from
/// `deeppass::build_context` because emphasis cares ONLY about speech (captions/OCR are not
/// spoken and would mislead the word-splitting), and the shape it needs is segment-oriented.
fn transcript_block(idx: &Index) -> String {
    idx.transcript
        .iter()
        .map(|s| format!("[{:.1}s .. {:.1}s] {}", s.start, s.end, s.text))
        .collect::<Vec<_>>()
        .join("\n")
}

fn unix_rfc3339() -> String {
    // Good enough for a "generated_at" stamp — the deep pass uses unix-secs strings
    // elsewhere; an RFC3339-ish stamp here is fine and human-readable.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_primary_secondary_none() {
        assert_eq!(parse_emphasis("primary"), Emphasis::Primary);
        assert_eq!(parse_emphasis("Secondary"), Emphasis::Secondary);
        assert_eq!(parse_emphasis(""), Emphasis::None);
        assert_eq!(parse_emphasis("garbage"), Emphasis::None);
    }

    #[test]
    fn parse_emphasis_json_strips_fence() {
        let fenced = "```json\n{\"segments\":[{\"start\":0,\"end\":1,\"words\":[{\"text\":\"hi\",\"emphasis\":\"primary\"}]}]}\n```";
        let r = parse_emphasis_json(fenced).unwrap();
        assert_eq!(r.segments.len(), 1);
        assert_eq!(r.segments[0].words[0].text, "hi");
    }

    #[test]
    fn transcript_block_only_uses_speech() {
        use clipxd_index::{Metadata, OnScreenText, Source, TextKind, TranscriptSegment, VisualMoment};
        let mut idx = Index::new(
            "clp_1",
            Source::Screen,
            Metadata {
                duration: 10.0, resolution: [100, 100], fps: 30.0, created_at: "0".into(),
                title: "t".into(), description: String::new(), app_focus: vec![], url_context: None,
                has_video: true,
            },
        );
        idx.transcript.push(TranscriptSegment { start: 0.0, end: 1.0, speaker: None, text: "hello world".into() });
        idx.visual_timeline.push(VisualMoment { t: 5.0, salience: 1.0, caption: "not speech".into(), delta: "d".into(), frame_ref: None });
        idx.on_screen_text.push(OnScreenText { start: 1.0, end: 1.0, text: "also not speech".into(), source: TextKind::Ocr, bbox: None });
        let block = transcript_block(&idx);
        assert!(block.contains("hello world"));
        assert!(!block.contains("not speech"));
    }
}