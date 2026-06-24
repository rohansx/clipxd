//! The browser-trace JSON format (`docs/phase2-browser-spec.md` §1).
//!
//! A single object: a small header + a flat, time-ordered `events[]` array, each a tagged
//! union on `type`. Clean-room, rrweb/CDP-compatible *in spirit*. `t_ms` is wall-clock
//! ms-since-epoch (the capture script reconciles all timebases); `started_at_ms` is the
//! `t = 0` anchor, so clip-relative seconds are `(t_ms - started_at_ms) / 1000`.

use serde::Deserialize;

fn default_viewport() -> Viewport {
    Viewport { w: 1280, h: 800 }
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Viewport {
    pub w: u32,
    pub h: u32,
}

/// One captured browser session.
#[derive(Debug, Clone, Deserialize)]
pub struct BrowserTrace {
    #[serde(default)]
    pub clipxd_trace_version: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub captured_by: String,
    #[serde(default)]
    pub started_at_ms: u64,
    #[serde(default = "default_viewport")]
    pub viewport: Viewport,
    #[serde(default)]
    pub url: String,
    pub events: Vec<TraceEvent>,
}

/// A single trace event; `type` selects the variant. Unknown types are ignored
/// (forward-compat) via [`TraceEvent::Unknown`].
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraceEvent {
    Navigate {
        t_ms: u64,
        url: String,
        #[serde(default)]
        from: Option<String>,
        #[serde(default)]
        nav_kind: String,
        #[serde(default)]
        title: Option<String>,
    },
    DomSnapshot {
        t_ms: u64,
        #[serde(default)]
        url: String,
        #[serde(default)]
        node_count: u64,
        #[serde(default)]
        text: Option<String>,
    },
    DomMutation {
        t_ms: u64,
        #[serde(default)]
        target: String,
        #[serde(default)]
        op: String,
        #[serde(default)]
        added: i64,
        #[serde(default)]
        removed: i64,
        #[serde(default)]
        text_delta: i64,
        #[serde(default)]
        role: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        attr: Option<String>,
    },
    Console {
        t_ms: u64,
        level: String,
        text: String,
        #[serde(default)]
        stack: Option<Vec<String>>,
        #[serde(default)]
        source: String,
        #[serde(default)]
        uncaught: bool,
    },
    Network {
        t_ms: u64,
        method: String,
        url: String,
        #[serde(default)]
        status: Option<u16>,
        #[serde(default)]
        status_text: Option<String>,
        #[serde(default)]
        resource_type: String,
        #[serde(default)]
        duration_ms: Option<u64>,
        #[serde(default)]
        error_text: Option<String>,
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        initiator: String,
    },
    Click {
        t_ms: u64,
        #[serde(default)]
        click_kind: String,
        #[serde(default)]
        target: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        x: Option<i64>,
        #[serde(default)]
        y: Option<i64>,
    },
    Input {
        t_ms: u64,
        #[serde(default)]
        target: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        value: String,
        #[serde(default)]
        checked: Option<bool>,
        #[serde(default)]
        masked: bool,
        #[serde(default)]
        submit: bool,
    },
    Scroll {
        t_ms: u64,
        #[serde(default)]
        target: String,
        #[serde(default)]
        x: i64,
        #[serde(default)]
        y: i64,
    },
    A11yText {
        t_ms: u64,
        #[serde(default)]
        selector: String,
        #[serde(default)]
        role: Option<String>,
        text: String,
        #[serde(default)]
        valid_until_ms: Option<u64>,
        #[serde(default)]
        sensitive: bool,
    },
    Screenshot {
        t_ms: u64,
        path: String,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        redacted: bool,
    },
    /// Forward-compat: any unrecognized `type` deserializes here and is ignored.
    #[serde(other)]
    Unknown,
}

impl TraceEvent {
    /// The event's wall-clock timestamp (ms). `Unknown` has none → `None`.
    pub fn t_ms(&self) -> Option<u64> {
        use TraceEvent::*;
        Some(match self {
            Navigate { t_ms, .. }
            | DomSnapshot { t_ms, .. }
            | DomMutation { t_ms, .. }
            | Console { t_ms, .. }
            | Network { t_ms, .. }
            | Click { t_ms, .. }
            | Input { t_ms, .. }
            | Scroll { t_ms, .. }
            | A11yText { t_ms, .. }
            | Screenshot { t_ms, .. } => *t_ms,
            Unknown => return None,
        })
    }
}

/// Did a network response indicate an error? `status >= 400`, an explicit `status == 0`
/// (failed/opaque), or a transport `error_text`. A *missing* status (e.g. a resource-timing
/// entry that exposes no HTTP code) is **not** an error by itself — only `error_text` makes
/// it one (spec §1.3).
pub fn network_is_error(status: Option<u16>, error_text: &Option<String>) -> bool {
    error_text.is_some() || matches!(status, Some(s) if s >= 400 || s == 0)
}

impl BrowserTrace {
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(s)?)
    }

    /// Clip-relative seconds for a wall-clock `t_ms`.
    pub fn rel_s(&self, t_ms: u64) -> f64 {
        (t_ms.saturating_sub(self.started_at_ms)) as f64 / 1000.0
    }

    /// Duration in seconds (last event relative to start).
    pub fn duration_s(&self) -> f64 {
        let last = self.events.iter().filter_map(|e| e.t_ms()).max().unwrap_or(self.started_at_ms);
        self.rel_s(last)
    }
}
