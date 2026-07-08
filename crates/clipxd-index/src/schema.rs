//! The clip-index schema (`clipxd_version = "2"`).
//!
//! A clip resolves, from one URL, to this object. It is a **time-indexed bundle of
//! streams**: transcript, visual timeline, on-screen text, event track — plus a derived
//! summary and a redaction manifest. All timestamps are **seconds from clip start**
//! (`f64`). See `docs/index-schema.md` for the prose spec.

use serde::{Deserialize, Serialize};

/// Wire schema version. `"2"` adds the additive [`SearchCorpus`] (`search`) field and the
/// post-enrichment [`clean`](crate::clean) pass; every `"1"` field keeps its shape.
pub const CLIPXD_SCHEMA_VERSION: &str = "2";

/// Which backend produced a clip. The agent surface is identical across all three;
/// only which streams are populated differs (import has an empty [`Index::event_track`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Screen,
    Browser,
    Import,
}

/// Honest completeness signal. `Enriching`/`Partial` mean some streams are still filling
/// in or an enricher failed — consumers must not treat a partial index as complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Complete,
    Partial,
    Enriching,
    /// Instant-link staged upload: the share URL already resolves but the user is still
    /// recording — chunks are landing and the video/metadata aren't final yet.
    Recording,
}

/// The top-level clip index — the artifact an agent queries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub clipxd_version: String,
    pub id: String,
    pub source: Source,
    pub status: Status,
    pub metadata: Metadata,
    pub transcript: Vec<TranscriptSegment>,
    pub visual_timeline: Vec<VisualMoment>,
    pub on_screen_text: Vec<OnScreenText>,
    pub event_track: Vec<Event>,
    pub summary: Summary,
    pub redaction: Redaction,
    /// v2: consolidated, lowercase, deduped text corpus for agent retrieval.
    /// Always present post-clean_index. Old consumers (who never read
    /// `search`) ignore it via serde's default behaviour. None of the v1
    /// fields change shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchCorpus>,

    /// v2.1: per-word emphasis for styled captions, produced at indexing time by the
    /// Ollama-Cloud-first LLM pass (`clipxd-web::emphasis`). Absent when no LLM backend
    /// is configured or the pass failed — never blocks the clip completing. Consumed by
    /// the Karaoke/Bold subtitle designs and the burned-in caption render.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_emphasis: Option<SubtitleEmphasis>,

    /// v2.1: the user-chosen caption design + knobs, set via `POST /clip/:id/subtitle-style`.
    /// Absent until the user picks a design on the clip page. Pure presentation metadata —
    /// does not affect agent queryability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_style: Option<SubtitleStyle>,
}

/// Per-word emphasis for one transcript segment, produced by the indexing-time LLM pass.
/// `text` is the word verbatim; `emphasis` is how strongly a caption should highlight it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmphasisWord {
    pub text: String,
    pub emphasis: Emphasis,
}

/// How strongly a word should be highlighted in a styled caption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Emphasis {
    /// The key word to focus on — largest/brightest in Karaoke, bold in Bold.
    Primary,
    /// A supporting term — secondary highlight.
    Secondary,
    /// No special emphasis.
    None,
}

impl Default for Emphasis {
    fn default() -> Self {
        Self::None
    }
}

/// A transcript span with per-word emphasis, aligned to the transcript's own timestamps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmphasisSegment {
    pub start: f64,
    pub end: f64,
    pub words: Vec<EmphasisWord>,
}

/// The indexing-time LLM emphasis pass output, stored on the index as `subtitle_emphasis`.
/// `generated_by` records which backend answered (for logging/debugging), matching the
/// `used` string the deep pass returns from `llm::complete_with_keys`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtitleEmphasis {
    pub generated_by: String,
    pub generated_at: String,
    pub segments: Vec<EmphasisSegment>,
}

/// The user-chosen caption design. Pure presentation — saved from the clip page, consumed by
/// the render path and the live caption preview. `design` is the preset name; the rest are
/// knobs the preset defaults but the user can override.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtitleStyle {
    /// One of the preset names: `classic`, `bold`, `karaoke`, `minimal`, `boxed`, `glow`.
    pub design: String,
    /// Caption font scale, 0.8 .. 1.6. 1.0 = preset default.
    #[serde(default = "one_f32")]
    pub font_scale: f32,
    /// Vertical anchor for the caption band.
    #[serde(default = "pos_bottom")]
    pub position: String,
    /// Whether to honor `subtitle_emphasis` (Karaoke highlight / Bold weighting).
    #[serde(default = "default_true")]
    pub emphasis: bool,
}

fn one_f32() -> f32 {
    1.0
}
fn pos_bottom() -> String {
    "bottom".to_string()
}
fn default_true() -> bool {
    true
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            design: "classic".to_string(),
            font_scale: 1.0,
            position: "bottom".to_string(),
            emphasis: true,
        }
    }
}

/// A single string per kind — easy to grep, easy to embed, easy to score
/// against.  An agent that gets this blob can answer retrieval-style
/// questions without parsing the structured tree.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchCorpus {
    /// Full transcript text (no timestamps — just the running text).
    #[serde(default)]
    pub transcript: String,
    /// All OCR text spans, joined with a single space, deduped of
    /// single-frame noise.
    #[serde(default)]
    pub screen_text: String,
    /// Event labels — "click at (23%, 50%)", "press 'a'", "GET /foo" …
    #[serde(default)]
    pub events: String,
}

impl Index {
    /// A new, empty index for `source`, stamped with the current schema version.
    pub fn new(id: impl Into<String>, source: Source, metadata: Metadata) -> Self {
        Self {
            clipxd_version: CLIPXD_SCHEMA_VERSION.to_string(),
            id: id.into(),
            source,
            status: Status::Complete,
            metadata,
            transcript: Vec::new(),
            visual_timeline: Vec::new(),
            on_screen_text: Vec::new(),
            event_track: Vec::new(),
            summary: Summary::default(),
            redaction: Redaction::default(),
            search: None,
            subtitle_emphasis: None,
            subtitle_style: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    /// Clip duration in seconds.
    pub duration: f64,
    /// `[width, height]` in pixels.
    pub resolution: [u32; 2],
    pub fps: f32,
    /// When the clip was created (RFC3339, or unix-seconds string in Phase 1).
    pub created_at: String,
    /// AI-derived, human-editable title.
    pub title: String,
    /// AI-derived one-sentence description, for library cards. Empty until the auto-title
    /// pass runs (or for clips recorded before this field existed).
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub app_focus: Vec<AppFocus>,
    /// Browser mode: the page(s) involved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_context: Option<String>,
    pub has_video: bool,
}

/// Which app/window was foreground over a time span (screen mode).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppFocus {
    pub start: f64,
    pub end: f64,
    pub app: String,
    pub window: String,
}

/// A time-aligned span of transcribed speech.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub start: f64,
    pub end: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub text: String,
}

/// A veyo-gated salient moment, captioned. The heart of the index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisualMoment {
    pub t: f64,
    pub salience: f32,
    pub caption: String,
    /// veyo's structured delta kind (`region_change`, `state_settle`, …).
    pub delta: String,
    /// Path/URL to the retained, redacted salient frame, if one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_ref: Option<String>,
}

/// Where on-screen text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextKind {
    Ocr,
    Dom,
}

/// Searchable, timestamped text that appeared on screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OnScreenText {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub source: TextKind,
    /// `[x, y, w, h]` in pixels, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[i32; 4]>,
}

/// An interaction-stream entry. Empty for import; rich in browser mode (console, network,
/// DOM); input events in screen mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub t: f64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Free-form structured payload (status, url, target, …).
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub data: serde_json::Map<String, serde_json::Value>,
}

/// Derived convenience — explicitly *not* the source of truth.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub tldr: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chapters: Vec<Chapter>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub start: f64,
    pub title: String,
}

/// The redaction receipt: what CloakPipe masked, where. Every other stream is already
/// post-redaction; the manifest is the audit trail, not a to-do list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Redaction {
    pub ran: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<RedactionItem>,
    pub policy: String,
}

impl Default for Redaction {
    fn default() -> Self {
        // Phase 1 stubs CloakPipe (the field exists so turning it on is a swap, not a
        // schema change — see docs/privacy-and-redaction.md §6).
        Self {
            ran: false,
            engine: None,
            items: Vec::new(),
            policy: "none-phase1-stub".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedactionItem {
    pub stream: String,
    pub t: f64,
    pub entity: String,
    pub action: String,
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn roundtrips_through_json() {
        let mut idx = Index::new("clp_1", Source::Import, meta());
        idx.on_screen_text.push(OnScreenText {
            start: 13.0,
            end: 13.0,
            text: "Payment failed (500)".into(),
            source: TextKind::Ocr,
            bbox: Some([320, 210, 460, 30]),
        });
        let json = serde_json::to_string(&idx).unwrap();
        let back: Index = serde_json::from_str(&json).unwrap();
        assert_eq!(idx, back);
    }

    #[test]
    fn source_and_status_serialize_snake_case() {
        let idx = Index::new("clp_1", Source::Import, meta());
        let json = serde_json::to_string(&idx).unwrap();
        assert!(json.contains("\"source\":\"import\""), "{json}");
        assert!(json.contains("\"status\":\"complete\""), "{json}");
        assert!(json.contains("\"clipxd_version\":\"2\""), "{json}");
    }

    #[test]
    fn empty_optional_streams_are_omitted() {
        let idx = Index::new("clp_1", Source::Import, meta());
        let json = serde_json::to_string(&idx).unwrap();
        // app_focus / url_context / chapters / redaction.items are empty -> omitted
        assert!(!json.contains("app_focus"), "{json}");
        assert!(!json.contains("url_context"), "{json}");
        // but the core streams are always present (even when empty arrays)
        assert!(json.contains("\"transcript\":[]"), "{json}");
        assert!(json.contains("\"event_track\":[]"), "{json}");
    }
}
