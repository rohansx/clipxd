//! Indexing-time moment-label pass — turns per-keyframe caption soup into the short ACTION
//! labels a viewer actually navigates by ("Introduces the elephants", "The long-thumbs bit").
//!
//! The captioner looks at each keyframe **independently**, with no memory across frames. On a
//! single-shot clip that means one held scene gets re-described a dozen times with slight
//! rewording — every row of the outline saying "a young man stands in front of a metal fence"
//! in a marginally different way. [`crate::clean`](clipxd_index::clean_index)'s similarity
//! bars catch the obvious repeats locally; this pass is the one that can actually read the
//! clip and say which moments are *distinct beats*, because it sees them all at once.
//!
//! Runs **only inside `spawn_phase2`** (and on re-enrich), never on the request path — the same
//! indexing-time, log-and-swallow contract as [`crate::emphasis`] and auto-title. Gated on any
//! LLM backend being configured (server- or owner-supplied BYOK), so a local-first box with no
//! key is unaffected: `label` simply stays `None` and every consumer falls back to `caption`.
//!
//! Non-destructive by design: survivors keep their original caption/salience/delta/frame_ref,
//! so the grounding, the retrieval surface, and the frame thumbnails are all preserved.

use crate::llm;
use anyhow::{bail, Context, Result};
use clipxd_index::{Index, VisualMoment};
use std::collections::HashSet;
use std::path::Path;

/// Most moments the pass may keep — an outline longer than this is a scroll, not a glance.
const MAX_LABELLED: usize = 8;
/// A "label" longer than this is a caption again. Enforced here rather than trusted from the
/// model, which treats word limits as a suggestion.
const MAX_LABEL_WORDS: usize = 6;
/// How much of a caption to show the model. Long enough to carry the scene, short enough that a
/// degenerate 2000-char caption can't dominate the prompt.
const CAPTION_BUDGET: usize = 240;

const LABEL_PROMPT_PREFIX: &str = "You are indexing a recording so a viewer can jump straight to the moment they \
want. Below are timestamped scene captions from a vision model that looked at each keyframe INDEPENDENTLY — it \
has no memory across frames, so one continuous shot is often described many times with slight rewording. \
Collapse those into the distinct MOMENTS a viewer would actually navigate to. Reply with ONLY JSON (no markdown \
fences, no commentary), shaped exactly as {\"keep\":[{\"i\":number,\"label\":string}]}. `i` is the input index of \
the moment to KEEP (drop every index you omit — omit near-duplicates of a moment you already kept, keeping the \
EARLIEST). `label`: at most 6 words, an ACTION or beat naming what happens (\"Introduces the elephants\", \"The \
long-thumbs bit\") — never a description of appearance or clothing. Prefer the spoken narration for naming the \
beat when it is present. Keep at most 8. Never invent a moment that is not in the input.\n\n";

/// A parsed `{keep:[…]}` reply.
#[derive(serde::Deserialize)]
struct LabelResult {
    #[serde(default)]
    keep: Vec<KeepIn>,
}

#[derive(serde::Deserialize)]
struct KeepIn {
    /// Signed on purpose: a model that emits `-1` should cost us that one entry, not the whole
    /// reply (a `usize` here would fail the entire parse and waste the call).
    #[serde(default)]
    i: i64,
    #[serde(default)]
    label: String,
}

/// Run the label pass for the clip in `clip_dir`, collapsing near-duplicate moments and naming
/// the survivors, then merge the result into its `index.json`.
///
/// `nvidia_key`/`gemini_key` are the clip owner's own BYOK keys (looked up by the caller via
/// `Db::llm_keys`), if any — `None` falls back to the server's env-configured keys, matching
/// [`crate::emphasis`].
pub async fn run(clip_dir: &Path, id: &str, nvidia_key: Option<&str>, gemini_key: Option<&str>) -> Result<()> {
    let index_path = clip_dir.join("index.json");
    let idx: Index = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;
    if idx.visual_timeline.is_empty() {
        bail!("no visual timeline yet to label");
    }
    // Idempotency: already labelled — never re-spend the call. A re-enrich rebuilds the
    // timeline with `label: None`, which correctly re-opens the gate for the new captions.
    if idx.visual_timeline.iter().any(|m| m.label.is_some()) {
        return Ok(());
    }
    if nvidia_key.is_none() && gemini_key.is_none() && !llm::any_backend_configured() {
        bail!("no LLM backend configured");
    }

    let prompt = format!("{LABEL_PROMPT_PREFIX}{}", build_moment_context(&idx));
    let (text, used) = llm::complete_with_keys(&prompt, true, nvidia_key, gemini_key).await?;
    let parsed = parse_label_json(&text)?;
    let keep = validate(parsed, idx.visual_timeline.len());
    if keep.is_empty() {
        // Leaves visual_timeline untouched — a degenerate reply must never destroy the index.
        bail!("model returned no usable moments");
    }

    // Re-read before writing: enrichment and the sibling passes write this file too, and this
    // pass has been awaiting a network round-trip.
    let mut index: Index = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;
    let orig = index.visual_timeline.clone();
    if orig.len() != idx.visual_timeline.len() {
        bail!("visual_timeline changed under the label pass — skipping rather than mislabelling");
    }
    let mut labelled: Vec<VisualMoment> = keep
        .into_iter()
        .map(|(i, label)| VisualMoment { label: Some(label), ..orig[i].clone() })
        .collect();
    labelled.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    let n = labelled.len();
    index.visual_timeline = labelled;
    std::fs::write(&index_path, serde_json::to_string_pretty(&index)?)?;
    eprintln!("label pass ({used}): {n} moments for {id} (from {})", orig.len());
    Ok(())
}

/// Keep only what we can honestly honor: in-range indices, first mention of each index, a
/// non-empty label, at most [`MAX_LABEL_WORDS`] words, at most [`MAX_LABELLED`] moments.
fn validate(parsed: LabelResult, timeline_len: usize) -> Vec<(usize, String)> {
    let mut seen: HashSet<usize> = HashSet::new();
    parsed
        .keep
        .into_iter()
        .filter_map(|k| {
            let i = usize::try_from(k.i).ok().filter(|i| *i < timeline_len)?;
            if !seen.insert(i) {
                return None;
            }
            let label = truncate_words(k.label.trim().trim_matches('"'), MAX_LABEL_WORDS);
            (!label.is_empty()).then_some((i, label))
        })
        .take(MAX_LABELLED)
        .collect()
}

fn truncate_words(s: &str, max: usize) -> String {
    s.split_whitespace().take(max).collect::<Vec<_>>().join(" ")
}

fn parse_label_json(text: &str) -> Result<LabelResult> {
    let cleaned = llm::strip_fence(text);
    serde_json::from_str(cleaned).with_context(|| format!("label JSON parse: {cleaned:.200}"))
}

/// The index-numbered moment block the prompt refers to, plus the narration when there is any.
///
/// Indices, not float timestamps: the model only has to echo back a small integer, and matching
/// a float round-trip (`18.0` vs `18.000001`) against the timeline would be fragile. Narration
/// leads because it is what names a beat — "introduces the elephants" is in the speech, never in
/// a description of someone's hair.
fn build_moment_context(idx: &Index) -> String {
    let mut s = String::new();
    if !idx.transcript.is_empty() {
        s.push_str("SPOKEN NARRATION (use this to name the beats):\n");
        for seg in &idx.transcript {
            s.push_str(&format!("[{:.1}s] {}\n", seg.start, seg.text.trim()));
        }
        s.push('\n');
    }
    s.push_str("SCENE CAPTIONS (index, timestamp, caption):\n");
    for (i, m) in idx.visual_timeline.iter().enumerate() {
        let caption: String = m.caption.trim().chars().take(CAPTION_BUDGET).collect();
        s.push_str(&format!("{i} [{:.1}s] {caption}\n", m.t));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(json: &str) -> LabelResult {
        parse_label_json(json).unwrap()
    }

    #[test]
    fn parse_label_json_strips_fence() {
        let r = result("```json\n{\"keep\":[{\"i\":0,\"label\":\"Introduces the elephants\"}]}\n```");
        assert_eq!(r.keep.len(), 1);
        assert_eq!(r.keep[0].label, "Introduces the elephants");
    }

    #[test]
    fn drops_out_of_range_and_duplicate_indices() {
        // 9 is past the end, -1 is nonsense, 0 repeats — all must fall out without taking the
        // usable entries with them.
        let r = result(r#"{"keep":[{"i":0,"label":"a b"},{"i":9,"label":"past the end"},{"i":-1,"label":"negative"},{"i":0,"label":"dupe"},{"i":2,"label":"c d"}]}"#);
        let keep = validate(r, 3);
        assert_eq!(keep, vec![(0, "a b".to_string()), (2, "c d".to_string())]);
    }

    #[test]
    fn truncates_a_runaway_label_to_six_words() {
        let r = result(r#"{"keep":[{"i":0,"label":"one two three four five six seven eight nine ten eleven twelve"}]}"#);
        let keep = validate(r, 1);
        assert_eq!(keep[0].1, "one two three four five six");
    }

    #[test]
    fn drops_empty_labels_and_caps_the_kept_set() {
        let entries: Vec<String> = (0..12).map(|i| format!(r#"{{"i":{i},"label":"beat {i}"}}"#)).collect();
        let r = result(&format!(r#"{{"keep":[{}]}}"#, entries.join(",")));
        assert_eq!(validate(r, 12).len(), MAX_LABELLED);

        let r = result(r#"{"keep":[{"i":0,"label":"   "},{"i":1,"label":"real beat"}]}"#);
        assert_eq!(validate(r, 2), vec![(1, "real beat".to_string())]);
    }

    #[test]
    fn a_garbage_reply_yields_nothing_to_merge() {
        // The load-bearing degenerate case: `run` bails on an empty keep set, which leaves the
        // existing visual_timeline untouched rather than wiping it.
        let r = result(r#"{"keep":[{"i":42,"label":"invented"}]}"#);
        assert!(validate(r, 3).is_empty());
    }
}
