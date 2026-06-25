//! Blur regions → the index's **redaction manifest**. A recording can mark spatial +
//! temporal regions to hide (a password field, a token on screen). Each becomes a
//! [`RedactionItem`] so the index is *honest* about what's obscured — auditable, not silent
//! (the privacy thesis) — and the renderer pixelates the region before any frame is stored.
//!
//! This is the explicit/manual-region path; automatic text-secret scanning is CloakPipe's
//! job in Phase 4. See `docs/privacy-and-redaction.md`.

use clipxd_index::{Index, Redaction, RedactionItem};
use serde::{Deserialize, Serialize};

/// A region to blur, in normalized `0..1` space, over `[start, end]` seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlurRegion {
    pub start: f64,
    pub end: f64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub label: String,
}

/// Build the auditable redaction manifest for a set of blur regions.
pub fn redaction_for(blurs: &[BlurRegion]) -> Redaction {
    if blurs.is_empty() {
        return Redaction::default();
    }
    let items = blurs
        .iter()
        .map(|b| RedactionItem {
            stream: "frame".into(),
            t: b.start,
            entity: if b.label.trim().is_empty() { "manual_region".into() } else { b.label.clone() },
            action: "blurred".into(),
        })
        .collect();
    Redaction {
        ran: true,
        engine: Some("manual-region".into()),
        items,
        policy: "explicit-blur-regions".into(),
    }
}

/// Record the blur regions in `index.redaction` (overwrites the stub).
pub fn apply_redaction(index: &mut Index, blurs: &[BlurRegion]) {
    if !blurs.is_empty() {
        index.redaction = redaction_for(blurs);
    }
}

/// The blur region active at time `t` (for the renderer to pixelate).
pub fn blur_at(blurs: &[BlurRegion], t: f64) -> Option<&BlurRegion> {
    blurs.iter().find(|b| t >= b.start && t <= b.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(t0: f64, t1: f64, label: &str) -> BlurRegion {
        BlurRegion { start: t0, end: t1, x: 0.1, y: 0.1, w: 0.3, h: 0.1, label: label.into() }
    }

    #[test]
    fn blur_regions_become_an_auditable_manifest() {
        let r = redaction_for(&[region(2.0, 5.0, "card number"), region(8.0, 9.0, "")]);
        assert!(r.ran);
        assert_eq!(r.engine.as_deref(), Some("manual-region"));
        assert_eq!(r.items.len(), 2);
        assert_eq!(r.items[0].entity, "card number");
        assert_eq!(r.items[0].action, "blurred");
        assert_eq!(r.items[1].entity, "manual_region"); // default label
    }

    #[test]
    fn empty_blurs_leave_the_stub() {
        assert!(!redaction_for(&[]).ran);
    }

    #[test]
    fn blur_at_finds_the_active_region() {
        let bs = [region(2.0, 5.0, "a")];
        assert_eq!(blur_at(&bs, 3.0).map(|b| b.label.as_str()), Some("a"));
        assert!(blur_at(&bs, 6.0).is_none());
    }
}
