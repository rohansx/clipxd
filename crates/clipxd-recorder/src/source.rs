//! The capture-source abstraction. The recorder is agnostic to *where* frames + the event
//! track come from:
//!
//! - a **live screen capture** (the MIT `scap` crate + PipeWire/ScreenCaptureKit/D3D11),
//!   behind a feature flag — see `docs/phase3-recorder-plan.md`; not buildable on this
//!   Wayland / PipeWire-1.6 box yet, but a drop-in on Mac/Win/compatible Linux;
//! - a **video file** (for development + testing where live capture isn't available);
//! - or an **in-memory** source (for unit tests).
//!
//! Whatever the source, it yields a [`SourceInfo`] and an [`EventTrack`], which the rest of
//! the recorder turns into a beautified video + an agent-queryable index.

use crate::types::{EventTrack, SourceInfo};

/// A source of capture frames + an interaction event track.
pub trait CaptureSource {
    fn info(&self) -> SourceInfo;
    fn event_track(&self) -> EventTrack;
}

/// A fully in-memory source — the testing/seam implementation.
#[derive(Clone, Debug)]
pub struct InMemorySource {
    pub info: SourceInfo,
    pub events: EventTrack,
}

impl CaptureSource for InMemorySource {
    fn info(&self) -> SourceInfo {
        self.info
    }
    fn event_track(&self) -> EventTrack {
        self.events.clone()
    }
}
