//! Per-clip "ask an agent" synthesis — the LLM layer in front of
//! [`query::query_clip`](clipxd_index::query::query_clip).
//!
//! The deterministic path is keyword retrieval, and when nothing matches it hands back a
//! grounded overview built by concatenating the two most *salient* captions. On a clip whose
//! captions are a dozen rewordings of one held shot, that means the ask box answers "what
//! happens and what's the key moment?" with the same sentence about someone's hair, twice, and
//! never reads the question. This module actually answers it.
//!
//! Generated **on request** (`GET /clip/:id/query`), not on the enrichment path — an ask is a
//! per-ask output, the same tradeoff [`crate::docgen`] already makes. `query_clip` stays
//! exactly as it is underneath: it is the local-first, no-key, zero-latency fallback, MCP sits
//! on it, and every failure here falls back to it.
//!
//! Transcript-first on purpose: the narration is what a person *said happens*; the captions are
//! a vision model guessing at pixels with no memory across frames. Ranking them equally (as a
//! flat timestamp interleave would) is how the captions drown the answer.

use crate::llm;
use anyhow::{bail, Context, Result};
use clipxd_index::Index;

/// Cap each stream's contribution to the prompt. A long clip's transcript is the thing worth
/// spending tokens on, so it gets the largest budget; OCR is the noisiest per token, so the
/// least.
const MAX_TRANSCRIPT_LINES: usize = 120;
const MAX_MOMENT_LINES: usize = 40;
const MAX_OST_LINES: usize = 40;
/// Beyond this an "answer" is an essay — the ask box renders a short paragraph.
const MAX_ANSWER_CHARS: usize = 900;
const MAX_CITATIONS: usize = 6;

const ASK_PROMPT_PREFIX: &str = "Answer the user's question about this recording using the sources below. The \
SPOKEN NARRATION is the primary source — prefer it. The VISUAL MOMENTS come from a vision model that looked at \
each keyframe with no memory of the others, so it repeats itself and describes appearance rather than meaning — \
never quote it back as an answer. Reply with ONLY JSON (no markdown fences, no commentary), shaped exactly as \
{\"answer\": string, \"key_moment_t\": number, \"citations\": [number]}. `answer`: 1-3 sentences that DIRECTLY \
answer the question. `key_moment_t`: the timestamp in seconds of the single most important moment. `citations`: \
timestamps in seconds supporting the answer. If the sources genuinely do not answer it, say so plainly in \
`answer` — do not pad with descriptions.\n\n";

#[derive(serde::Deserialize)]
struct AskResult {
    #[serde(default)]
    answer: String,
    #[serde(default)]
    key_moment_t: Option<f64>,
    #[serde(default)]
    citations: Vec<f64>,
}

/// Answer `question` about `idx`, returning the prose plus the timestamps it is grounded in.
///
/// Citations are what the UI renders as click-to-seek chips, so they are clamped to the clip's
/// own duration — a hallucinated timestamp would seek nowhere.
pub async fn answer(idx: &Index, question: &str, nvidia_key: Option<&str>, gemini_key: Option<&str>) -> Result<(String, Vec<f64>)> {
    let q = question.trim();
    if q.is_empty() {
        bail!("no question asked");
    }
    let context = build_context(idx);
    if context.trim().is_empty() {
        bail!("no transcript/captions/OCR yet to answer from");
    }
    let prompt = format!("{ASK_PROMPT_PREFIX}QUESTION: {q}\n\nSOURCES:\n{context}");
    let (text, used) = llm::complete_with_keys(&prompt, true, nvidia_key, gemini_key).await?;
    let cleaned = llm::strip_fence(&text);
    let parsed: AskResult = serde_json::from_str(cleaned).with_context(|| format!("ask JSON parse: {cleaned:.200}"))?;

    let answer = parsed.answer.trim().chars().take(MAX_ANSWER_CHARS).collect::<String>();
    if answer.is_empty() {
        bail!("model returned an empty answer");
    }
    eprintln!("ask ({used}): answered {:?}", q.chars().take(60).collect::<String>());
    Ok((answer, citations(&parsed, idx.metadata.duration)))
}

/// The key moment leads (the UI seeks to the first citation), then the supporting timestamps.
/// Drops anything non-finite or outside the clip, and de-dupes to the tenth of a second — the
/// resolution the seek bar and `fmt_duration` actually distinguish.
fn citations(parsed: &AskResult, duration: f64) -> Vec<f64> {
    let limit = if duration.is_finite() && duration > 0.0 { duration } else { f64::MAX };
    let mut out: Vec<f64> = Vec::new();
    for t in parsed.key_moment_t.iter().chain(parsed.citations.iter()) {
        if !t.is_finite() || *t < 0.0 || *t > limit {
            continue;
        }
        if out.iter().any(|k| (k - t).abs() < 0.1) {
            continue;
        }
        out.push(*t);
        if out.len() == MAX_CITATIONS {
            break;
        }
    }
    out
}

/// A labelled, *ranked* source block — deliberately not
/// [`deeppass::build_context`](crate::deeppass), which interleaves all three streams flat by
/// timestamp with no priority. That shape is right for summarizing the whole clip and wrong for
/// answering a question: it is exactly what lets a dozen near-identical captions outweigh the
/// one sentence of narration that holds the answer.
fn build_context(idx: &Index) -> String {
    let mut s = String::new();
    if !idx.transcript.is_empty() {
        s.push_str("SPOKEN NARRATION (primary source — trust this first):\n");
        for seg in idx.transcript.iter().take(MAX_TRANSCRIPT_LINES) {
            s.push_str(&format!("[{:.1}s] {}\n", seg.start, seg.text.trim()));
        }
        s.push('\n');
    }
    if !idx.visual_timeline.is_empty() {
        s.push_str(
            "VISUAL MOMENTS (secondary — the vision model described keyframes independently and repeats itself; use only to locate beats):\n",
        );
        for m in idx.visual_timeline.iter().take(MAX_MOMENT_LINES) {
            // The label, when the label pass has run, is the beat's name; the caption is the
            // evidence behind it. Both help here, so show both when they exist.
            match m.label.as_deref().filter(|l| !l.trim().is_empty()) {
                Some(label) => s.push_str(&format!("[{:.1}s] {label} — {}\n", m.t, m.caption.trim())),
                None => s.push_str(&format!("[{:.1}s] {}\n", m.t, m.caption.trim())),
            }
        }
        s.push('\n');
    }
    if !idx.on_screen_text.is_empty() {
        s.push_str("ON-SCREEN TEXT (tertiary):\n");
        for t in idx.on_screen_text.iter().take(MAX_OST_LINES) {
            s.push_str(&format!("[{:.1}s] {}\n", t.start, t.text.trim()));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use clipxd_index::{Metadata, OnScreenText, Source, TextKind, TranscriptSegment, VisualMoment};

    fn idx() -> Index {
        let mut idx = Index::new(
            "clp_1",
            Source::Import,
            Metadata {
                duration: 20.0,
                resolution: [100, 100],
                fps: 30.0,
                created_at: "0".into(),
                title: "t".into(),
                description: String::new(),
                app_focus: vec![],
                url_context: None,
                has_video: true,
            },
        );
        idx.transcript.push(TranscriptSegment { start: 1.0, end: 2.0, speaker: None, text: "these are the elephants".into() });
        idx.visual_timeline.push(VisualMoment {
            t: 5.0,
            salience: 1.0,
            caption: "a young man in front of a fence".into(),
            delta: "d".into(),
            frame_ref: None,
            label: Some("Introduces the elephants".into()),
        });
        idx.on_screen_text.push(OnScreenText { start: 3.0, end: 3.0, text: "ZOO MAP".into(), source: TextKind::Ocr, bbox: None });
        idx
    }

    #[test]
    fn context_ranks_narration_above_moments_above_ocr() {
        let c = build_context(&idx());
        let (speech, moment, ocr) = (c.find("elephants").unwrap(), c.find("young man").unwrap(), c.find("ZOO MAP").unwrap());
        assert!(speech < moment && moment < ocr, "narration must lead, OCR must trail:\n{c}");
        assert!(c.contains("primary source"), "the ranking has to be stated, not just implied:\n{c}");
        // The label names the beat; the caption stays as the evidence behind it.
        assert!(c.contains("Introduces the elephants — a young man in front of a fence"), "{c}");
    }

    #[test]
    fn key_moment_leads_the_citations() {
        let p = AskResult { answer: "a".into(), key_moment_t: Some(12.0), citations: vec![1.0, 5.0] };
        assert_eq!(citations(&p, 20.0), vec![12.0, 1.0, 5.0]);
    }

    #[test]
    fn drops_hallucinated_and_duplicate_timestamps() {
        // 99.0 is past the end of a 20s clip, -1.0 is impossible, NaN is nonsense, and 12.04
        // is 12.0 again at seek-bar resolution.
        let p = AskResult {
            answer: "a".into(),
            key_moment_t: Some(12.0),
            citations: vec![99.0, -1.0, f64::NAN, 12.04, 5.0],
        };
        assert_eq!(citations(&p, 20.0), vec![12.0, 5.0]);
    }

    #[test]
    fn missing_key_moment_still_cites() {
        let p = AskResult { answer: "a".into(), key_moment_t: None, citations: vec![3.0] };
        assert_eq!(citations(&p, 20.0), vec![3.0]);
    }
}
