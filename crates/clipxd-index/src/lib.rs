//! `clipxd-index` — the clip-index schema and query surface. **This is the product.**
//!
//! Everything else (recorder, cinematic layer, share page) is plumbing that produces or
//! serves *this object*. The [`Index`] is what an agent queries from a clip's URL,
//! without ever downloading the video. It is defined here **once** and depended on by
//! every other crate, so changing the agent-facing contract means changing one crate.
//!
//! Governing principle: *an agent should never need the pixels. If it does, the index
//! failed.* Every field exists so a question about the video can be answered from text.
//!
//! The companion [`query`] module is the read surface ([`search_text`](query::search_text),
//! [`get_frame_context`](query::get_frame_context), [`query_clip`](query::query_clip))
//! that the MCP server and JSON API sit on top of.

pub mod clean;
pub mod query;
pub mod schema;

pub use clean::clean_index;
pub use query::{Answer, FrameContext, TextHit};
pub use schema::{
    AppFocus, Chapter, Emphasis, EmphasisSegment, EmphasisWord, Event, Index, Metadata, OnScreenText,
    Redaction, RedactionItem, SearchCorpus, Source, Status, SubtitleEmphasis, SubtitleStyle, Summary,
    TextKind, TranscriptSegment, VisualMoment, CLIPXD_SCHEMA_VERSION,
};

/// Milliseconds → seconds, the unit every timestamp in the index uses.
pub fn ms_to_s(ms: u64) -> f64 {
    ms as f64 / 1000.0
}
