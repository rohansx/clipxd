//! `clipxd-import` — Phase 1: turn an existing video (URL or local file) into a clipxd
//! [`Index`](clipxd_index::Index) with **no capture code**.
//!
//! It demuxes frames + audio, runs them through the **veyo-core** salience gate (which
//! moments matter), enriches the salient moments via **veyo-enrich** (transcript · OCR ·
//! caption), and assembles `index.json`. This is the smallest thing that proves the
//! headline — *paste a link, query it from text* — and it doubles as the generator of the
//! real sessions veyo's codec needs for tuning. See `docs/phases.md` (Phase 1).

pub mod downscale;
pub mod gate;
pub mod map;
pub mod media;
pub mod pipeline;

pub use pipeline::{import, ImportOptions, ImportOutput};
