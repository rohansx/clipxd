//! On-frame annotations — arrows, boxes, text labels, highlights — authored in the produced
//! frame's content space (normalized `0..1`) and drawn as an overlay on top of the composited
//! video. Clean-room: these are the standard annotation primitives every screen tool has;
//! the shapes themselves are uncopyrightable. The renderer (`beautify`) draws them.

use serde::{Deserialize, Serialize};

/// One annotation, visible during `[start, end]`. `(x, y)` is the anchor; `(x2, y2)` is the
/// far point for `arrow`/`box`/`highlight` (ignored for `text`). All coords are `0..1` of the
/// content area.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub start: f64,
    pub end: f64,
    pub kind: String, // "arrow" | "box" | "text" | "highlight"
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub x2: f64,
    #[serde(default)]
    pub y2: f64,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub color: String, // "#rrggbb"; empty → renderer's default
}

impl Annotation {
    pub fn active(&self, t: f64) -> bool {
        t >= self.start && t <= self.end
    }
}

/// The annotations visible at time `t`.
pub fn annotations_at(anns: &[Annotation], t: f64) -> Vec<&Annotation> {
    anns.iter().filter(|a| a.active(t)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(kind: &str, t0: f64, t1: f64) -> Annotation {
        Annotation { start: t0, end: t1, kind: kind.into(), x: 0.5, y: 0.5, x2: 0.7, y2: 0.7, text: "x".into(), color: String::new() }
    }

    #[test]
    fn only_active_annotations_are_returned() {
        let anns = [ann("arrow", 1.0, 3.0), ann("text", 5.0, 6.0)];
        assert_eq!(annotations_at(&anns, 2.0).len(), 1);
        assert_eq!(annotations_at(&anns, 2.0)[0].kind, "arrow");
        assert!(annotations_at(&anns, 4.0).is_empty());
        assert_eq!(annotations_at(&anns, 5.5).len(), 1);
    }
}
