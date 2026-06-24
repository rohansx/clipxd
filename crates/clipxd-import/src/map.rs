//! Map veyo-enrich's [`Enrichment`] into the clipxd [`Index`]. This is the one place the
//! engine's output types are translated into the product's agent-facing schema; the
//! dependency arrow is clipxd → veyo-enrich, never the reverse.

use crate::media::MediaInfo;
use clipxd_index::{
    ms_to_s, Chapter, Index, Metadata, OnScreenText, Source, Summary, TextKind, TranscriptSegment,
    VisualMoment,
};
use std::path::Path;
use veyo_enrich::{Enrichment, TextSource};

/// Build the clip [`Index`] from probed media + the enrichment streams.
pub fn to_index(
    id: &str,
    source: Source,
    media: &MediaInfo,
    title: &str,
    created_at: &str,
    enrichment: &Enrichment,
) -> Index {
    let mut idx = Index::new(
        id,
        source,
        Metadata {
            duration: media.duration_s,
            resolution: [media.width, media.height],
            fps: media.fps,
            created_at: created_at.to_string(),
            title: title.to_string(),
            app_focus: Vec::new(),
            url_context: None,
            has_video: true,
        },
    );

    idx.transcript = enrichment
        .transcript
        .iter()
        .map(|s| TranscriptSegment {
            start: ms_to_s(s.start_ms),
            end: ms_to_s(s.end_ms),
            speaker: s.speaker.clone(),
            text: s.text.clone(),
        })
        .collect();

    idx.on_screen_text = enrichment
        .on_screen_text
        .iter()
        .map(|o| OnScreenText {
            start: ms_to_s(o.t_ms),
            end: ms_to_s(o.t_ms),
            text: o.text.clone(),
            source: match o.source {
                TextSource::Ocr => TextKind::Ocr,
                TextSource::Dom => TextKind::Dom,
            },
            bbox: o.bbox.map(|r| [r.x, r.y, r.w, r.h]),
        })
        .collect();

    idx.visual_timeline = enrichment
        .visual_timeline
        .iter()
        .map(|m| VisualMoment {
            t: ms_to_s(m.t_ms),
            salience: m.salience,
            caption: m.caption.clone(),
            delta: m.delta_kind.clone(),
            frame_ref: m.frame_ref.as_deref().map(rel_frame),
        })
        .collect();

    // event_track stays empty for import (no input/DOM events exist for a finished video).
    idx.summary = derive_summary(&idx);
    idx
}

/// Rewrite an absolute salient-frame path to a clip-relative `frames/<name>` reference.
fn rel_frame(abs: &str) -> String {
    Path::new(abs)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| format!("frames/{n}"))
        .unwrap_or_else(|| abs.to_string())
}

/// Derive the (non-authoritative) summary: a one-line TL;DR from the most salient moment,
/// and chapters from the top moments in time order.
fn derive_summary(idx: &Index) -> Summary {
    let tldr = idx
        .visual_timeline
        .iter()
        .max_by(|a, b| a.salience.partial_cmp(&b.salience).unwrap_or(std::cmp::Ordering::Equal))
        .map(|m| m.caption.clone())
        .unwrap_or_else(|| {
            format!(
                "Imported {:.0}s clip with {} salient moment(s).",
                idx.metadata.duration,
                idx.visual_timeline.len()
            )
        });

    let mut moments: Vec<&VisualMoment> = idx.visual_timeline.iter().collect();
    moments.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    let chapters = moments
        .iter()
        .take(8)
        .map(|m| Chapter {
            start: m.t,
            title: truncate(&m.caption, 60),
        })
        .collect();

    Summary { tldr, chapters }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('…');
        t
    }
}
