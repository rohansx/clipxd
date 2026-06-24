//! The recorder's interaction **event track** — cursor path, clicks, keystrokes — captured
//! alongside the video. It does double duty: it drives the [cinematic](clipxd_cinematic)
//! auto-zoom *and* becomes part of the agent-queryable index (the moat: a recording you can
//! ask questions of). Spatial coords are normalized `0..1` (see [`clipxd_cinematic`]).

pub use clipxd_cinematic::{Click, CursorSample};
use serde::{Deserialize, Serialize};

/// A keystroke (or chord) at time `t` (seconds). Subject to redaction before it reaches a
/// shared index (passwords/secrets — Phase 4 CloakPipe).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyPress {
    pub t: f64,
    pub key: String,
}

/// Everything the user *did* during a recording.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EventTrack {
    #[serde(default)]
    pub cursors: Vec<CursorSample>,
    #[serde(default)]
    pub clicks: Vec<Click>,
    #[serde(default)]
    pub keys: Vec<KeyPress>,
}

impl EventTrack {
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(s)?)
    }

    /// True if nothing was captured (a screenshot, or a source with no input track).
    pub fn is_empty(&self) -> bool {
        self.cursors.is_empty() && self.clicks.is_empty() && self.keys.is_empty()
    }
}

/// Probed facts about a capture source.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_s: f64,
}
