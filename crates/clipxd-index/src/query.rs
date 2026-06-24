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
        "doing", "were", "how", "why", "when", "where", "who", "happen", "happened", "screen",
        "video", "clip", "thing", "things", "something", "anything", "see", "saw",
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

/// A grounded, cited answer. Deterministic retrieval: find the best-matching moment,
/// describe it with its timestamp, and — when the question asks — describe what happened
/// just before it.
pub fn query_clip(index: &Index, question: &str) -> Answer {
    let hits = search_text(index, question);
    let Some(best) = hits.first() else {
        return Answer {
            text: format!("No matching content found in the index for: \"{question}\""),
            citations: Vec::new(),
        };
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
        let before = before_context(index, best.t);
        if let Some((t, what)) = before {
            text.push_str(&format!(" Just before, at {t:.1}s: {what}"));
            citations.push(t);
        }
    }

    Answer { text, citations }
}

/// The most recent salient caption or transcript line strictly before `t`.
fn before_context(index: &Index, t: f64) -> Option<(f64, String)> {
    let mut candidates: Vec<(f64, String)> = Vec::new();
    for m in &index.visual_timeline {
        if m.t < t - 0.05 {
            candidates.push((m.t, m.caption.clone()));
        }
    }
    for s in &index.transcript {
        if s.start < t - 0.05 {
            candidates.push((s.start, s.text.clone()));
        }
    }
    for e in &index.event_track {
        if e.t < t - 0.05 {
            let what = e.text.clone().unwrap_or_else(|| e.kind.clone());
            candidates.push((e.t, what));
        }
    }
    candidates
        .into_iter()
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
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
        });
        idx.visual_timeline.push(VisualMoment {
            t: 13.0,
            salience: 0.93,
            caption: "A red toast appears. On screen: \"ERROR: Payment failed (500)\"".into(),
            delta: "state_settle".into(),
            frame_ref: Some("frames/00052.png".into()),
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
    fn query_clip_handles_no_match() {
        let idx = checkout_index();
        let ans = query_clip(&idx, "elephant giraffe");
        assert!(ans.text.starts_with("No matching content"));
        assert!(ans.citations.is_empty());
    }
}
