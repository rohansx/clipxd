//! Device-mockup geometry + keystroke-pill timing — two small clean-room pieces of the
//! "produced look". Pure layout/timing math (the renderer draws them); the browser-frame
//! shape (a titlebar above the content with traffic-light dots) and the pill merge/hold
//! behavior are observable, uncopyrightable layout, re-derived here
//! (`docs/recorder-feature-catalog.md` §B/§E).

use crate::types::ZoomKeyframe;
use serde::{Deserialize, Serialize};

/// Which device frame to wrap the video in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mockup {
    None,
    Browser,
}

/// Where the titlebar + video sit inside a `box_w`×`box_h` area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MockupLayout {
    pub bar_h: u32,
    pub video_x: u32,
    pub video_y: u32,
    pub video_w: u32,
    pub video_h: u32,
    /// Center of the three traffic-light dots, left to right.
    pub dot_r: u32,
    pub dot_y: u32,
    pub dot_x: [u32; 3],
}

/// Fit a browser mockup into a `box_w`×`box_h` area: titlebar on top, video below.
pub fn browser_in(box_w: u32, box_h: u32) -> MockupLayout {
    let bar_h = ((box_h as f64 * 0.07).round() as u32).clamp(28, 72);
    let video_h = box_h.saturating_sub(bar_h).max(2);
    let dot_r = (bar_h / 6).clamp(4, 9);
    let dot_y = bar_h / 2;
    let gap = dot_r * 3;
    let x0 = dot_r * 4;
    MockupLayout {
        bar_h,
        video_x: 0,
        video_y: bar_h,
        video_w: box_w,
        video_h,
        dot_r,
        dot_y,
        dot_x: [x0, x0 + gap, x0 + 2 * gap],
    }
}

/// A keystroke overlay pill, visible during `[t_start, t_end]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pill {
    pub t_start: f64,
    pub t_end: f64,
    pub text: String,
}

/// Merge a keystroke stream into pills: keys within `merge_s` of each other join into one
/// pill (so typing "git push" shows one pill, not 8), each held for `hold_s` after the last
/// key. `keys` is `(t, label)`.
pub fn keystroke_pills(keys: &[(f64, String)], merge_s: f64, hold_s: f64) -> Vec<Pill> {
    if keys.is_empty() {
        return Vec::new();
    }
    let mut ks = keys.to_vec();
    ks.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut pills = Vec::new();
    let mut start = ks[0].0;
    let mut last = ks[0].0;
    let mut text = ks[0].1.clone();
    for (t, label) in ks.iter().skip(1) {
        if t - last <= merge_s {
            text.push_str(label);
            last = *t;
        } else {
            pills.push(Pill { t_start: start, t_end: last + hold_s, text: std::mem::take(&mut text) });
            start = *t;
            last = *t;
            text = label.clone();
        }
    }
    pills.push(Pill { t_start: start, t_end: last + hold_s, text });
    pills
}

/// The pill visible at time `t`, if any.
pub fn pill_at(pills: &[Pill], t: f64) -> Option<&Pill> {
    pills.iter().find(|p| t >= p.t_start && t <= p.t_end)
}

/// Convenience: the zoom keyframe nearest `t` (used by renderers that index by time).
pub fn keyframe_at(track: &[ZoomKeyframe], t: f64) -> Option<&ZoomKeyframe> {
    track
        .iter()
        .min_by(|a, b| (a.t - t).abs().partial_cmp(&(b.t - t).abs()).unwrap_or(std::cmp::Ordering::Equal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_mockup_reserves_a_titlebar_above_the_video() {
        let m = browser_in(1280, 720);
        assert!(m.bar_h >= 28 && m.bar_h <= 72);
        assert_eq!(m.video_y, m.bar_h);
        assert_eq!(m.video_h, 720 - m.bar_h);
        assert!(m.dot_x[0] < m.dot_x[1] && m.dot_x[1] < m.dot_x[2], "three dots, left to right");
    }

    #[test]
    fn keystrokes_merge_into_pills_then_split_on_a_gap() {
        let keys = vec![
            (1.0, "g".into()), (1.1, "i".into()), (1.2, "t".into()), // "git"
            (5.0, "Enter".into()),                                    // gap → new pill
        ];
        let pills = keystroke_pills(&keys, 0.5, 1.0);
        assert_eq!(pills.len(), 2);
        assert_eq!(pills[0].text, "git");
        assert!((pills[0].t_end - (1.2 + 1.0)).abs() < 1e-9);
        assert_eq!(pills[1].text, "Enter");
        assert_eq!(pill_at(&pills, 1.5).map(|p| p.text.as_str()), Some("git"));
        assert!(pill_at(&pills, 3.0).is_none(), "gap between pills is empty");
    }
}
