//! Post-enrichment noise reduction — the pass that turns a raw enricher dump into
//! something an agent can actually read.
//!
//! The local captioner emits one caption *per sampled keyframe* and one OCR bag *per
//! frame*, with no cross-frame memory. On a static screen that means the same "collage
//! of fragmented text … Kernel Or, Kernel Or, Kernel Or …" caption repeated dozens of
//! times, hundreds of near-duplicate OCR snapshots, and a `summary.tldr` that is one of
//! those multi-KB captions verbatim. [`clean_index`] collapses that:
//!
//! 1. **`visual_timeline`** — merge runs of near-identical captions (token-overlap ≥
//!    [`CAPTION_SIM`]), then keep only the [`MAX_MOMENTS`] most salient, back in time order.
//! 2. **`on_screen_text`** — drop OCR garbage and exact repeats, keeping the first sighting.
//! 3. **`summary.tldr`** — cap at [`TLDR_MAX`] chars so it reads like a summary, not a dump.
//! 4. **`search`** — build the flat [`SearchCorpus`](crate::SearchCorpus) an agent greps.
//!
//! Pure `Index -> Index`, idempotent (running it twice is a no-op after the first), and
//! dependency-free beyond `serde_json` — it stays inside "the product" crate on purpose.

use crate::schema::{Index, OnScreenText, SearchCorpus, VisualMoment, CLIPXD_SCHEMA_VERSION};
use std::collections::HashSet;

/// Most moments kept in `visual_timeline` after dedup — enough to skim, not a scroll.
const MAX_MOMENTS: usize = 16;
/// Two captions whose token sets overlap at or above this Jaccard score are the same moment.
const CAPTION_SIM: f64 = 0.85;
/// `summary.tldr` longer than this (chars, incl. the ellipsis) is truncated at a word boundary.
const TLDR_MAX: usize = 280;

/// Clean an enriched index in place: dedup the noisy streams, tame the tldr, and populate
/// [`Index::search`](crate::Index::search). Safe to call on any index (including empty or
/// already-clean ones) and idempotent.
pub fn clean_index(index: &mut Index) {
    dedup_visual_timeline(&mut index.visual_timeline);
    dedup_on_screen_text(&mut index.on_screen_text);
    truncate_tldr(&mut index.summary.tldr);
    index.search = Some(build_search(index));
    // A cleaned index carries the v2 shape (search + deduped streams), whether it was just
    // enriched or backfilled from a v1 clip — stamp it so consumers can trust the field.
    index.clipxd_version = CLIPXD_SCHEMA_VERSION.to_string();
}

/// Collapse runs of near-identical captions (keeping the earliest sighting, promoted to the
/// run's peak salience), then keep the [`MAX_MOMENTS`] most salient moments in time order.
fn dedup_visual_timeline(items: &mut Vec<VisualMoment>) {
    if items.is_empty() {
        return;
    }
    items.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));

    let mut kept: Vec<VisualMoment> = Vec::new();
    for m in items.drain(..) {
        match kept.last_mut() {
            Some(last) if caption_similarity(&last.caption, &m.caption) >= CAPTION_SIM => {
                // Same moment, restated — anchor on the earliest t but keep the peak salience.
                last.salience = last.salience.max(m.salience);
            }
            _ => kept.push(m),
        }
    }

    if kept.len() > MAX_MOMENTS {
        kept.sort_by(|a, b| b.salience.partial_cmp(&a.salience).unwrap_or(std::cmp::Ordering::Equal));
        kept.truncate(MAX_MOMENTS);
        kept.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    }
    *items = kept;
}

/// Drop OCR garbage and exact (normalized, case-insensitive) repeats, keeping the first
/// sighting of each distinct span in time order.
fn dedup_on_screen_text(items: &mut Vec<OnScreenText>) {
    items.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));

    let mut seen: HashSet<String> = HashSet::new();
    let mut kept: Vec<OnScreenText> = Vec::new();
    for mut o in items.drain(..) {
        let norm = normalize_ws(&o.text);
        if norm.is_empty() || is_ocr_noise(&norm) {
            continue;
        }
        if !seen.insert(norm.to_lowercase()) {
            continue;
        }
        o.text = norm;
        kept.push(o);
    }
    *items = kept;
}

/// Cap the tldr at [`TLDR_MAX`] chars, cut on a word boundary with a trailing ellipsis.
fn truncate_tldr(tldr: &mut String) {
    if tldr.chars().count() <= TLDR_MAX {
        return;
    }
    let head: String = tldr.chars().take(TLDR_MAX - 1).collect();
    let cut = head.rfind(char::is_whitespace).unwrap_or(head.len());
    *tldr = format!("{}…", head[..cut].trim_end());
}

/// Flatten the (already-cleaned) streams into the one-string-per-kind corpus an agent greps.
fn build_search(index: &Index) -> SearchCorpus {
    let transcript = join_nonempty(index.transcript.iter().map(|s| s.text.as_str()), " ");
    let screen_text = join_nonempty(index.on_screen_text.iter().map(|o| o.text.as_str()), " ").to_lowercase();
    let events = join_nonempty(index.event_track.iter().filter_map(|e| e.text.as_deref()), "; ");
    SearchCorpus { transcript, screen_text, events }
}

fn join_nonempty<'a>(parts: impl Iterator<Item = &'a str>, sep: &str) -> String {
    parts.map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(sep)
}

/// Token-set Jaccard similarity of two captions — 1.0 identical, 0.0 disjoint.
fn caption_similarity(a: &str, b: &str) -> f64 {
    let (sa, sb) = (token_set(a), token_set(b));
    if sa.is_empty() && sb.is_empty() {
        return 1.0;
    }
    let inter = sa.iter().filter(|w| sb.contains(*w)).count();
    let union = sa.len() + sb.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

/// Meaningful (3+ char) alphanumeric tokens, lowercased.
fn token_set(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(str::to_string)
        .collect()
}

/// Collapse all whitespace runs to single spaces and trim.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A span is OCR noise if it is mostly symbols or has no real word — e.g. `@ a @ Bc Be O i]`.
fn is_ocr_noise(s: &str) -> bool {
    let total = s.chars().filter(|c| !c.is_whitespace()).count();
    if total == 0 {
        return true;
    }
    let alnum = s.chars().filter(|c| c.is_alphanumeric()).count();
    if (alnum as f64) / (total as f64) < 0.5 {
        return true;
    }
    // Needs at least one token with 3+ letters to carry meaning.
    !s.split(|c: char| !c.is_alphanumeric())
        .any(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Metadata, Source, Summary, TextKind, TranscriptSegment};

    fn meta() -> Metadata {
        Metadata {
            duration: 10.0,
            resolution: [1920, 1080],
            fps: 30.0,
            created_at: "1700000000".into(),
            title: "t".into(),
            app_focus: vec![],
            url_context: None,
            has_video: true,
        }
    }

    fn moment(t: f64, salience: f32, caption: &str) -> VisualMoment {
        VisualMoment { t, salience, caption: caption.into(), delta: "keyframe".into(), frame_ref: None }
    }

    fn ost(t: f64, text: &str) -> OnScreenText {
        OnScreenText { start: t, end: t, text: text.into(), source: TextKind::Ocr, bbox: None }
    }

    #[test]
    fn merges_near_identical_captions_keeping_earliest_and_peak_salience() {
        let mut idx = Index::new("clp_1", Source::Screen, meta());
        idx.visual_timeline = vec![
            moment(0.0, 0.2, "Kernel Optimization collage of fragmented text Kernel Or Kernel Or"),
            moment(2.0, 0.9, "Kernel Optimization collage of fragmented text Kernel Or Kernel Or Kernel"),
            moment(4.0, 0.5, "A profile settings page with name and email input fields"),
        ];
        clean_index(&mut idx);
        assert_eq!(idx.visual_timeline.len(), 2);
        assert_eq!(idx.visual_timeline[0].t, 0.0); // earliest sighting anchors the moment
        assert_eq!(idx.visual_timeline[0].salience, 0.9); // promoted to the run peak
        assert_eq!(idx.visual_timeline[1].t, 4.0);
    }

    #[test]
    fn caps_at_sixteen_most_salient_in_time_order() {
        let mut idx = Index::new("clp_1", Source::Screen, meta());
        // 20 distinct captions (unique `tagNN` token each) with increasing salience by time.
        idx.visual_timeline =
            (0..20).map(|i| moment(i as f64, i as f32 / 20.0, &format!("distinct scene tag{i:02} showing content"))).collect();
        clean_index(&mut idx);
        assert_eq!(idx.visual_timeline.len(), MAX_MOMENTS);
        // Kept the top-16 salience (t = 4..=19), still sorted by time.
        assert_eq!(idx.visual_timeline.first().unwrap().t, 4.0);
        assert_eq!(idx.visual_timeline.last().unwrap().t, 19.0);
        assert!(idx.visual_timeline.windows(2).all(|w| w[0].t <= w[1].t));
    }

    #[test]
    fn drops_ocr_noise_and_exact_repeats() {
        let mut idx = Index::new("clp_1", Source::Screen, meta());
        idx.on_screen_text = vec![
            ost(0.0, "@ a @ Bc Be O i] i o x"),   // symbol/short-token soup -> dropped
            ost(1.0, "  Library   Settings  "),    // real -> kept, whitespace-normalized
            ost(2.0, "Library Settings"),          // exact repeat -> dropped
            ost(3.0, "=== +++ >>>"),               // no alnum -> dropped
        ];
        clean_index(&mut idx);
        assert_eq!(idx.on_screen_text.len(), 1);
        assert_eq!(idx.on_screen_text[0].text, "Library Settings");
    }

    #[test]
    fn truncates_runaway_tldr_at_word_boundary() {
        let mut idx = Index::new("clp_1", Source::Screen, meta());
        idx.summary = Summary { tldr: "Kernel Or ".repeat(400), chapters: vec![] };
        clean_index(&mut idx);
        assert!(idx.summary.tldr.chars().count() <= TLDR_MAX);
        assert!(idx.summary.tldr.ends_with('…'));
    }

    #[test]
    fn builds_search_corpus_from_cleaned_streams() {
        let mut idx = Index::new("clp_1", Source::Screen, meta());
        idx.transcript = vec![TranscriptSegment { start: 0.0, end: 1.0, speaker: None, text: "hello world".into() }];
        idx.on_screen_text = vec![ost(0.0, "Deploy Button")];
        idx.event_track = vec![crate::schema::Event {
            t: 0.5,
            kind: "click".into(),
            text: Some("click at (0.23, 0.50)".into()),
            data: Default::default(),
        }];
        clean_index(&mut idx);
        let s = idx.search.as_ref().unwrap();
        assert_eq!(s.transcript, "hello world");
        assert_eq!(s.screen_text, "deploy button"); // lowercased
        assert_eq!(s.events, "click at (0.23, 0.50)");
    }

    #[test]
    fn is_idempotent() {
        let mut idx = Index::new("clp_1", Source::Screen, meta());
        idx.visual_timeline = vec![
            moment(0.0, 0.2, "Kernel Or Kernel Or collage fragmented text"),
            moment(2.0, 0.9, "Kernel Or Kernel Or collage fragmented text more"),
        ];
        idx.summary.tldr = "word ".repeat(400);
        idx.on_screen_text = vec![ost(0.0, "Library"), ost(1.0, "Library")];
        clean_index(&mut idx);
        let once = idx.clone();
        clean_index(&mut idx);
        assert_eq!(once, idx);
    }
}
