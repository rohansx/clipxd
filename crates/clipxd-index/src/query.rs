//! The read surface over an [`Index`](crate::Index): the primitives an agent composes to
//! answer a question about a clip **without downloading the video**. The MCP server and
//! the JSON API are thin wrappers over these.
//!
//! - [`search_text`] — rank text hits across transcript + on-screen text + captions
//! - [`get_frame_context`] — everything true at time `t`
//! - [`query_clip`] — a grounded, cited answer (deterministic retrieval; a real LLM would
//!   compose the primitives itself, but this proves the index *is* queryable from text)

use crate::schema::Index;

/// A ranked text match somewhere in the index.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TextHit {
    pub t: f64,
    pub text: String,
    /// Which stream it came from: `transcript` | `on_screen_text` | `caption`.
    pub stream: &'static str,
    pub score: f32,
}

/// Everything true at one instant — the answer to "what was on screen at `t`".
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct FrameContext {
    pub t: f64,
    pub caption: Option<String>,
    pub on_screen_text: Vec<String>,
    pub transcript: Option<String>,
    pub app: Option<String>,
    pub events: Vec<String>,
    pub frame_ref: Option<String>,
}

/// A grounded, cited answer to a natural-language question about the clip.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Answer {
    pub text: String,
    /// Timestamps (seconds) the answer is grounded in — the citations.
    pub citations: Vec<f64>,
}

/// Lowercased content terms of length ≥ 3 that aren't stopwords.
fn keywords(q: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "and", "was", "what", "did", "show", "you", "did", "are", "for", "that", "this",
        "then", "there", "with", "his", "her", "its", "user", "users", "right", "before", "after",
        "doing", "were", "how", "why", "when", "where", "who", "happen", "happened", "happens",
        "happening", "screen", "video", "clip", "thing", "things", "something", "anything",
        "see", "saw",
    ];
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(|w| w.to_lowercase())
        .filter(|w| !STOP.contains(&w.as_str()))
        .collect()
}

/// Score `text` against query `terms`: one point per distinct term that appears as a
/// substring, plus a small bonus when the whole query phrase appears.
fn score_text(text: &str, terms: &[String], phrase: &str) -> f32 {
    let lc = text.to_lowercase();
    let mut s = 0.0;
    for t in terms {
        if lc.contains(t.as_str()) {
            s += 1.0;
        }
    }
    if !phrase.is_empty() && lc.contains(phrase) {
        s += 0.5;
    }
    s
}

/// Rank text hits across transcript, on-screen text, and captions for query `q`.
pub fn search_text(index: &Index, q: &str) -> Vec<TextHit> {
    let terms = keywords(q);
    let phrase = q.to_lowercase();
    let phrase = phrase.trim();
    let mut hits = Vec::new();

    for seg in &index.transcript {
        let sc = score_text(&seg.text, &terms, phrase);
        if sc > 0.0 {
            hits.push(TextHit { t: seg.start, text: seg.text.clone(), stream: "transcript", score: sc });
        }
    }
    for ost in &index.on_screen_text {
        let sc = score_text(&ost.text, &terms, phrase);
        if sc > 0.0 {
            hits.push(TextHit { t: ost.start, text: ost.text.clone(), stream: "on_screen_text", score: sc });
        }
    }
    for m in &index.visual_timeline {
        let sc = score_text(&m.caption, &terms, phrase);
        if sc > 0.0 {
            hits.push(TextHit { t: m.t, text: m.caption.clone(), stream: "caption", score: sc });
        }
    }

    // Highest score first; ties broken by earliest timestamp (stable, deterministic).
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal))
    });
    hits
}

const WINDOW_S: f64 = 1.5;

/// Everything the index knows about the instant `t`.
pub fn get_frame_context(index: &Index, t: f64) -> FrameContext {
    let near = |a: f64| (a - t).abs() <= WINDOW_S;

    // Caption and frame both come from the salient moment nearest in time to `t`.
    let nearest_moment = index
        .visual_timeline
        .iter()
        .filter(|m| near(m.t))
        .min_by(|a, b| {
            (a.t - t)
                .abs()
                .partial_cmp(&(b.t - t).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    let caption = nearest_moment.map(|m| m.caption.clone());
    let frame_ref = nearest_moment.and_then(|m| m.frame_ref.clone());

    let on_screen_text = index
        .on_screen_text
        .iter()
        .filter(|o| (o.start - t).abs() <= WINDOW_S || (o.start <= t && t <= o.end))
        .map(|o| o.text.clone())
        .collect();

    let transcript = index
        .transcript
        .iter()
        .find(|s| s.start <= t && t <= s.end)
        .or_else(|| index.transcript.iter().find(|s| near(s.start)))
        .map(|s| s.text.clone());

    let app = index
        .metadata
        .app_focus
        .iter()
        .find(|a| a.start <= t && t <= a.end)
        .map(|a| format!("{} — {}", a.app, a.window));

    let events = index
        .event_track
        .iter()
        .filter(|e| (e.t - t).abs() <= WINDOW_S)
        .map(|e| match &e.text {
            Some(tx) => format!("{}: {}", e.kind, tx),
            None => e.kind.clone(),
        })
        .collect();

    FrameContext { t, caption, on_screen_text, transcript, app, events, frame_ref }
}

/// Whether a question is asking about what came *before* its main subject.
fn asks_for_before(q: &str) -> bool {
    let lc = q.to_lowercase();
    ["before", "prior", "leading up", "preceding", "beforehand"]
        .iter()
        .any(|k| lc.contains(k))
}

/// A grounded, cited answer. First routes by **intent** (a transcript question → the
/// narration; a summary question → the salient overview), then falls back to deterministic
/// keyword retrieval, and — when nothing matches — returns a grounded summary instead of a
/// dead end, so an agent always gets something to work with.
pub fn query_clip(index: &Index, question: &str) -> Answer {
    let lc = question.to_lowercase();

    // intent: "what did they say" / narration → the transcript
    if mentions(&lc, &["said", "say", "saying", "spoke", "speak", "talk", "narrat", "transcript", "voice", "audio", "mention"]) {
        if let Some(a) = transcript_answer(index) {
            return a;
        }
    }
    // intent: summarize / overview / what's this clip about
    if mentions(&lc, &["summar", "overview", "tldr", "recap", "gist", "what is this", "about this", "what's this"]) {
        return summary_answer(index);
    }

    // default: keyword retrieval over transcript + on-screen text + captions
    let hits = search_text(index, question);
    let Some(best) = hits.first() else {
        // no keyword match → hand the agent a grounded summary rather than "no match"
        return summary_answer(index);
    };

    let mut text = format!(
        "At {:.1}s, the {} shows: \"{}\".",
        best.t,
        best.stream.replace('_', " "),
        best.text.trim()
    );
    let mut citations = vec![best.t];

    if asks_for_before(question) {
        // The nearest salient moment / transcript line strictly before `best.t`.
        if let Some((t, what)) = before_context(index, best.t) {
            text.push_str(&format!(" Just before, at {t:.1}s: {what}"));
            citations.push(t);
        }
    }

    Answer { text, citations }
}

fn mentions(lc: &str, keys: &[&str]) -> bool {
    keys.iter().any(|k| lc.contains(k))
}

/// The narration as a cited answer (for "what did they say").
fn transcript_answer(index: &Index) -> Option<Answer> {
    if index.transcript.is_empty() {
        return None;
    }
    let segs: Vec<_> = index.transcript.iter().take(5).collect();
    let text = segs.iter().map(|s| s.text.trim()).collect::<Vec<_>>().join(" ");
    let citations = segs.iter().take(3).map(|s| s.start).collect();
    Some(Answer { text: format!("They said: \"{}\"", text.trim()), citations })
}

/// A grounded overview built from the most salient moments + the opening narration.
fn summary_answer(index: &Index) -> Answer {
    let mut parts = Vec::new();
    let mut citations = Vec::new();
    let mut moments: Vec<_> = index.visual_timeline.iter().collect();
    moments.sort_by(|a, b| b.salience.partial_cmp(&a.salience).unwrap_or(std::cmp::Ordering::Equal));
    for m in moments.iter().take(2) {
        parts.push(m.caption.trim().to_string());
        citations.push(m.t);
    }
    if let Some(s) = index.transcript.first() {
        parts.push(format!("Narration: \"{}\"", s.text.trim()));
        citations.push(s.start);
    }
    let text = if parts.is_empty() {
        format!("\"{}\" — a {:.0}s clip; no extracted text yet.", index.metadata.title, index.metadata.duration)
    } else {
        parts.join(" ")
    };
    Answer { text, citations }
}

/// What was happening just before `t`. Prefers the most recent **user action** — a click,
/// a gesture→request cause→effect moment, a navigation — the real answer to "what was the
/// user *doing*". Everything is strictly **before** `t` (never after — answering "what came
/// before" with a later event would be backwards causality); a triggering action is still
/// captured because it is recorded a hair before the symptom it causes. Falls back to the
/// latest passive moment / transcript line when there is no action.
fn before_context(index: &Index, t: f64) -> Option<(f64, String)> {
    const ACTION_KINDS: &[&str] = &[
        "click", "context_menu", "key", "input", "form_submit", "scroll", "navigate", "navigation",
    ];
    // cause→effect summaries (the gesture→request join) ARE "what the user did".
    const ACTION_DELTAS: &[&str] = &["gesture_request"];
    const LOOKBACK: f64 = 12.0;

    let mut actions: Vec<(f64, String)> = Vec::new();
    let mut others: Vec<(f64, String)> = Vec::new();

    for e in &index.event_track {
        if ACTION_KINDS.contains(&e.kind.as_str()) {
            if e.t < t && e.t >= t - LOOKBACK {
                let what = if e.kind == "click" || e.kind == "form_submit" {
                    format!("clicked {}", quote_or(&e.text, "an element"))
                } else {
                    e.text.clone().unwrap_or_else(|| e.kind.clone())
                };
                actions.push((e.t, what));
            }
        } else if e.t < t - 0.05 {
            others.push((e.t, e.text.clone().unwrap_or_else(|| e.kind.clone())));
        }
    }
    for m in &index.visual_timeline {
        if ACTION_DELTAS.contains(&m.delta.as_str()) && m.t < t && m.t >= t - LOOKBACK {
            actions.push((m.t, m.caption.clone()));
        } else if m.t < t - 0.05 {
            others.push((m.t, m.caption.clone()));
        }
    }
    for s in &index.transcript {
        if s.start < t - 0.05 {
            others.push((s.start, s.text.clone()));
        }
    }

    let latest = |v: Vec<(f64, String)>| {
        v.into_iter()
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
    };
    latest(actions).or_else(|| latest(others))
}

fn quote_or(text: &Option<String>, fallback: &str) -> String {
    match text {
        Some(t) if !t.trim().is_empty() => format!("\"{}\"", t.trim()),
        _ => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::*;

    fn checkout_index() -> Index {
        let mut idx = Index::new(
            "clp_demo",
            Source::Import,
            Metadata {
                duration: 20.0,
                resolution: [1920, 1080],
                fps: 30.0,
                created_at: "0".into(),
                title: "Checkout 500".into(),
                description: String::new(),
                app_focus: vec![],
                url_context: None,
                has_video: true,
            },
        );
        idx.visual_timeline.push(VisualMoment {
            t: 12.4,
            salience: 0.8,
            caption: "Cursor clicks the 'Place order' button; a spinner appears.".into(),
            delta: "region_change".into(),
            frame_ref: Some("frames/00050.png".into()),
            label: None,
        });
        idx.visual_timeline.push(VisualMoment {
            t: 13.0,
            salience: 0.93,
            caption: "A red toast appears. On screen: \"ERROR: Payment failed (500)\"".into(),
            delta: "state_settle".into(),
            frame_ref: Some("frames/00052.png".into()),
            label: None,
        });
        idx.on_screen_text.push(OnScreenText {
            start: 13.0,
            end: 13.0,
            text: "ERROR: Payment failed (500)".into(),
            source: TextKind::Ocr,
            bbox: Some([320, 210, 600, 40]),
        });
        idx
    }

    #[test]
    fn search_finds_the_error_ranked_first() {
        let idx = checkout_index();
        let hits = search_text(&idx, "what error showed up");
        assert!(!hits.is_empty(), "should find the error");
        assert!(
            hits[0].text.to_lowercase().contains("error"),
            "top hit should be the error, got {:?}",
            hits[0]
        );
    }

    #[test]
    fn frame_context_assembles_everything_at_t() {
        let idx = checkout_index();
        let ctx = get_frame_context(&idx, 13.0);
        assert!(ctx.caption.as_deref().unwrap().contains("red toast"));
        assert!(ctx.on_screen_text.iter().any(|s| s.contains("Payment failed (500)")));
        assert_eq!(ctx.frame_ref.as_deref(), Some("frames/00052.png"));
    }

    #[test]
    fn query_clip_answers_the_headline_with_before_context_and_citations() {
        let idx = checkout_index();
        let ans = query_clip(&idx, "what error showed up and what was the user doing right before it");
        // grounds the error...
        assert!(ans.text.contains("Payment failed (500)"), "{}", ans.text);
        assert!(ans.text.contains("13.0s"), "{}", ans.text);
        // ...and what was happening just before (the click)
        assert!(ans.text.to_lowercase().contains("place order"), "{}", ans.text);
        assert!(ans.text.contains("12.4s"), "{}", ans.text);
        // cited both moments
        assert_eq!(ans.citations, vec![13.0, 12.4]);
    }

    #[test]
    fn query_clip_falls_back_to_a_grounded_summary() {
        let idx = checkout_index();
        let ans = query_clip(&idx, "elephant giraffe");
        // no keyword match → a grounded overview (salient moments), not a dead end
        assert!(!ans.citations.is_empty(), "summary should cite moments: {}", ans.text);
        assert!(
            ans.text.to_lowercase().contains("error") || ans.text.to_lowercase().contains("place order"),
            "{}",
            ans.text
        );
    }

    #[test]
    fn query_clip_returns_the_transcript_for_what_did_they_say() {
        let mut idx = checkout_index();
        idx.transcript.push(TranscriptSegment { start: 1.0, end: 3.0, speaker: None, text: "Let me place this order".into() });
        let ans = query_clip(&idx, "what did they say");
        assert!(ans.text.to_lowercase().contains("place this order"), "{}", ans.text);
        assert!(!ans.citations.is_empty());
    }

    #[test]
    fn query_clip_summarizes_on_request() {
        let idx = checkout_index();
        let ans = query_clip(&idx, "summarize this clip");
        assert!(!ans.citations.is_empty());
        assert!(ans.text.to_lowercase().contains("error") || ans.text.to_lowercase().contains("place order"), "{}", ans.text);
    }
}
