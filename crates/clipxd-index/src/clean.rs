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

/// A single caption longer than this after repetition-collapse is still a degenerate dump,
/// not a description — hard-truncated as a backstop for whatever the sentence-level collapse
/// didn't catch (e.g. a repeated phrase with no sentence punctuation at all).
const CAPTION_MAX: usize = 500;
/// Adjacent sentences *within one caption* whose token overlap is at or above this are the
/// same restated idea — collapsed to the first. Looser than [`CAPTION_SIM`] (which compares
/// whole captions to each other): a repetition loop's sentences vary more locally ("the
/// browser's tab bar includes X" / "...includes Y") than two honestly-different captions
/// about the same held frame do.
const SENTENCE_SIM: f64 = 0.6;

/// Clean an enriched index in place: dedup the noisy streams, tame the tldr, and populate
/// [`Index::search`](crate::Index::search). Safe to call on any index (including empty or
/// already-clean ones) and idempotent.
pub fn clean_index(index: &mut Index) {
    for m in &mut index.visual_timeline {
        m.caption = collapse_repetition(&m.caption);
    }
    dedup_visual_timeline(&mut index.visual_timeline);
    dedup_on_screen_text(&mut index.on_screen_text);
    truncate_tldr(&mut index.summary.tldr);
    index.search = Some(build_search(index));
    // A cleaned index carries the v2 shape (search + deduped streams), whether it was just
    // enriched or backfilled from a v1 clip — stamp it so consumers can trust the field.
    index.clipxd_version = CLIPXD_SCHEMA_VERSION.to_string();
}

/// Collapse a single caption's *internal* degeneration — a small local caption model
/// occasionally loops, repeating one token verbatim hundreds of times ("#include #include
/// #include …") or restating one idea with minor rewording across many sentences ("the
/// browser's tab bar includes X. The browser's tab bar includes Y. …"). Unlike
/// [`dedup_visual_timeline`] (which compares *whole captions to each other*), this looks
/// *inside* one caption.
fn collapse_repetition(caption: &str) -> String {
    // Pass 1: collapse runs of 3+ identical consecutive words ("#include #include #include")
    // down to one — catches token-level loops that have no sentence punctuation at all.
    let words: Vec<&str> = caption.split_whitespace().collect();
    let mut deworded: Vec<&str> = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let mut run = 1;
        while i + run < words.len() && words[i + run].eq_ignore_ascii_case(w) {
            run += 1;
        }
        deworded.push(w);
        i += run;
    }
    let deworded = deworded.join(" ");

    // Pass 2: collapse runs of adjacent sentences that restate the same idea (near-identical
    // wording, immediately next to each other).
    let sentences = split_sentences(&deworded);
    let mut kept_idx: Vec<usize> = Vec::with_capacity(sentences.len());
    for (i, (text, _delim)) in sentences.iter().enumerate() {
        match kept_idx.last() {
            Some(&last) if caption_similarity(sentences[last].0, text) >= SENTENCE_SIM => continue,
            _ => kept_idx.push(i),
        }
    }

    // Pass 3: a *template* loop — several sentences (not necessarily adjacent) opening with
    // the same few words but differing after that ("the browser's tab bar includes X" / "…
    // includes Y" / "… includes Z"). Cap how many sentences sharing one template survive;
    // keeping only the first two is enough to show the reader the pattern without the dump.
    const PREFIX_WORDS: usize = 5;
    const PREFIX_CAP: usize = 2;
    let mut prefix_seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let kept_idx: Vec<usize> = kept_idx
        .into_iter()
        .filter(|&i| {
            let prefix: String = sentences[i].0.split_whitespace().take(PREFIX_WORDS).collect::<Vec<_>>().join(" ").to_lowercase();
            if prefix.split_whitespace().count() < PREFIX_WORDS {
                return true; // too short a sentence for a prefix template to be meaningful
            }
            let count = prefix_seen.entry(prefix).or_insert(0);
            *count += 1;
            *count <= PREFIX_CAP
        })
        .collect();

    let collapsed = kept_idx
        .into_iter()
        .map(|i| format!("{}{}", sentences[i].0, sentences[i].1))
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();

    // Pass 3: hard backstop length cap, word-boundary truncation.
    if collapsed.chars().count() <= CAPTION_MAX {
        collapsed
    } else {
        let head: String = collapsed.chars().take(CAPTION_MAX - 1).collect();
        let cut = head.rfind(char::is_whitespace).unwrap_or(head.len());
        format!("{}…", head[..cut].trim_end())
    }
}

/// Split on sentence-ending punctuation, returning `(trimmed sentence text, its delimiter)` —
/// e.g. `"Hi there. Bye!"` -> `[("Hi there", "."), ("Bye", "!")]`. The last sentence gets `"."`
/// if it had no terminal punctuation at all (degenerate captions often just trail off).
///
/// Only treats `.`/`!`/`?` as a sentence boundary when followed by whitespace or end-of-
/// string — a bare `.find` would also split `"github.com"` and `"clone_code.dev-win.html"`
/// into meaningless fragments, since on-screen captions are full of URLs and filenames.
fn split_sentences(s: &str) -> Vec<(&str, &'static str)> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if matches!(c, b'.' | b'!' | b'?') {
            let next_is_boundary = i + 1 >= bytes.len() || bytes[i + 1].is_ascii_whitespace();
            if next_is_boundary {
                let delim = match c {
                    b'!' => "!",
                    b'?' => "?",
                    _ => ".",
                };
                let text = s[start..i].trim();
                if !text.is_empty() {
                    out.push((text, delim));
                }
                start = i + 1;
            }
        }
        i += 1;
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        out.push((tail, "."));
    }
    out
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
            description: String::new(),
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
    fn collapses_verbatim_token_repetition_loop() {
        // The exact failure mode reported live: a caption that's just one token repeated
        // hundreds of times ("#include #include #include …"), no sentence punctuation at all.
        let degenerate = format!("A code editor showing {}", "#include ".repeat(300));
        let mut idx = Index::new("clp_1", Source::Screen, meta());
        idx.visual_timeline = vec![moment(0.0, 0.5, &degenerate)];
        clean_index(&mut idx);
        let caption = &idx.visual_timeline[0].caption;
        assert!(caption.len() < 100, "expected the repetition collapsed, got {} chars", caption.len());
        assert_eq!(caption.matches("#include").count(), 1);
    }

    #[test]
    fn collapses_reworded_sentence_repetition_loop() {
        // The real reported case: a small caption model restating one idea with minor
        // rewording across many sentences instead of verbatim repetition.
        let degenerate = "A screenshot of a web browser displaying a GitHub repository page. \
            The browser's tab bar includes standard browser controls like back, forward, and reload buttons. \
            The browser's address bar includes \"github.com\" and a GitHub icon. \
            The browser's tab bar includes \"worksflows\". \
            The browser's tab bar includes standard GitHub navigation elements. \
            The browser's tab bar includes \"clone_code.dev-win.html\" and a GitHub icon. \
            The browser's tab bar includes standard GitHub navigation elements.";
        let mut idx = Index::new("clp_1", Source::Screen, meta());
        idx.visual_timeline = vec![moment(0.0, 0.5, degenerate)];
        clean_index(&mut idx);
        let caption = &idx.visual_timeline[0].caption;
        // Collapsed from 7 sentences to 4: the two genuinely distinct ones ("repository page",
        // "address bar") plus at most 2 examples of the repeated "tab bar includes" template
        // (not all 5) — enough to show the pattern without the dump.
        let kept = split_sentences(caption);
        assert_eq!(kept.len(), 4, "expected collapse, kept: {caption:?}");
        assert!(caption.contains("GitHub repository page"));
        assert_eq!(caption.matches("tab bar includes").count(), 2, "expected only 2 of 5 template repeats kept: {caption:?}");
    }

    #[test]
    fn caption_repetition_collapse_is_idempotent() {
        let degenerate = format!("Loading {}", "spinner spinner spinner ".repeat(50));
        assert_eq!(collapse_repetition(&degenerate), collapse_repetition(&collapse_repetition(&degenerate)));
    }

    #[test]
    fn leaves_a_normal_caption_untouched() {
        let normal = "A profile settings page with name and email input fields, a save button in the corner.";
        assert_eq!(collapse_repetition(normal), normal);
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
