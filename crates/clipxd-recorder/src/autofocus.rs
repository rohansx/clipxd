//! Content-aware auto-zoom focus — derive a zoom focus track from veyo deltas when there is
//! no input-device track (e.g. a browser screen recording, where the page can't see the OS
//! cursor). veyo already localizes *where* the change is (`Delta::region.bounds`) and *how
//! much* it matters (`salience`); we turn that into a synthetic cursor path + zoom triggers,
//! which the cinematic engine replays as the same 3-phase eased zoom it uses for real cursor
//! data (the OpenVid/Screen-Studio behavior, reproduced clean-room).
//!
//! This is the veyo-native answer to "auto-zoom toward the action": no cursor capture, no
//! per-tool hooks — the codec that already understands the screen tells the camera where to
//! look. The derived track drives **beautify only**; the queryable `event_track` still holds
//! real input events (here, none), so the index never claims synthetic clicks happened.

use crate::{Click, CursorSample, EventTrack};
use veyo_core::Delta;

/// Build a focus track (cursor path + zoom triggers) from veyo `deltas` over a
/// `width`×`height` frame. Empty in → empty out.
pub fn focus_track_from_deltas(deltas: &[Delta], width: u32, height: u32) -> EventTrack {
    if deltas.is_empty() {
        return EventTrack::default();
    }
    let (w, h) = (width.max(1) as f64, height.max(1) as f64);

    // (t_seconds, cx, cy, salience) for each delta, at its region centre, normalized 0..1
    let mut pts: Vec<(f64, f64, f64, f32)> = deltas
        .iter()
        .map(|d| {
            let b = d.region.bounds;
            let cx = ((b.x as f64 + b.w as f64 / 2.0) / w).clamp(0.0, 1.0);
            let cy = ((b.y as f64 + b.h as f64 / 2.0) / h).clamp(0.0, 1.0);
            (d.t_event as f64 / 1000.0, cx, cy, d.salience)
        })
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // cursor path = the activity centroid over time (the cinematic spring/EMA smooths it)
    let cursors: Vec<CursorSample> = pts.iter().map(|&(t, x, y, _)| CursorSample { t, x, y }).collect();

    // zoom triggers = the most salient, well-spaced moments — content "clicks"
    let mut ranked = pts.clone();
    ranked.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
    let mut clicks: Vec<Click> = Vec::new();
    for (t, x, y, _) in ranked {
        if clicks.len() >= 8 {
            break;
        }
        if clicks.iter().all(|c| (c.t - t).abs() > 0.8) {
            clicks.push(Click { t, x, y });
        }
    }
    clicks.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));

    EventTrack { cursors, clicks, keys: vec![] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veyo_core::{Delta, EventId, EventKind, Evidence, Rect, RegionRef, SurfaceRef};

    fn delta(t_ms: u64, x: i32, y: i32, sal: f32) -> Delta {
        Delta {
            v: 1,
            id: EventId("ev".into()),
            t_event: t_ms,
            t_observed: t_ms,
            source: "screen:0".into(),
            kind: EventKind::RegionChange,
            surface: SurfaceRef { id: "s".into(), app: "a".into(), title: "t".into(), focused: true },
            region: RegionRef { id: "r".into(), grid: [0, 0], bounds: Rect { x, y, w: 200, h: 200 } },
            summary: String::new(),
            salience: sal,
            novelty: 0.0,
            duration_ms: None,
            evidence: Evidence::default(),
        }
    }

    #[test]
    fn derives_a_centroid_cursor_and_salient_zoom_triggers() {
        // a 1000×1000 frame; a low-salience change top-left, a high-salience one bottom-right
        let deltas = [delta(500, 100, 100, 0.2), delta(2000, 700, 700, 0.9)];
        let track = focus_track_from_deltas(&deltas, 1000, 1000);

        assert_eq!(track.cursors.len(), 2);
        // centre of a 200×200 region at (700,700) → (0.8, 0.8)
        assert!((track.cursors[1].x - 0.8).abs() < 1e-9 && (track.cursors[1].y - 0.8).abs() < 1e-9);
        // the most salient moment becomes the first zoom trigger
        assert!(!track.clicks.is_empty());
        assert!((track.clicks.iter().map(|c| c.x).fold(0.0_f64, f64::max) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn empty_deltas_yield_an_empty_track() {
        assert!(focus_track_from_deltas(&[], 1920, 1080).is_empty());
    }
}
