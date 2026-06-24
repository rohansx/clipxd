//! Map a recording's [`EventTrack`] into clipxd index [`Event`]s — this is what makes a
//! *screen recording* agent-queryable, exactly like the browser backend's event track.
//! "When did the user click Deploy?" / "what was typed at 0:42?" become answerable from the
//! index, no pixels needed.

use crate::EventTrack;
use clipxd_index::Event;
use serde_json::json;

fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    }
}

/// Turn the interaction track into time-ordered `event_track` entries for the index.
pub fn to_index_events(track: &EventTrack) -> Vec<Event> {
    let mut events: Vec<Event> = Vec::with_capacity(track.clicks.len() + track.keys.len());

    for c in &track.clicks {
        events.push(Event {
            t: c.t,
            kind: "click".into(),
            text: Some(format!("click at ({:.2}, {:.2})", c.x, c.y)),
            data: obj(json!({ "x": c.x, "y": c.y })),
        });
    }
    for k in &track.keys {
        events.push(Event {
            t: k.t,
            kind: "key".into(),
            text: Some(k.key.clone()),
            data: serde_json::Map::new(),
        });
    }

    events.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use clipxd_cinematic::Click;
    use crate::KeyPress;

    #[test]
    fn clicks_and_keys_become_sorted_index_events() {
        let track = EventTrack {
            cursors: vec![],
            clicks: vec![Click { t: 2.0, x: 0.5, y: 0.5 }, Click { t: 0.5, x: 0.1, y: 0.9 }],
            keys: vec![KeyPress { t: 1.0, key: "Enter".into() }],
        };
        let ev = to_index_events(&track);
        assert_eq!(ev.len(), 3);
        // sorted by time
        assert!(ev[0].t <= ev[1].t && ev[1].t <= ev[2].t);
        assert_eq!(ev[0].kind, "click"); // @0.5
        assert_eq!(ev[1].kind, "key"); // @1.0
        assert_eq!(ev[1].text.as_deref(), Some("Enter"));
        assert_eq!(ev[2].t, 2.0);
        // click carries normalized coords for an agent to locate it
        assert_eq!(ev[0].data.get("x").and_then(|v| v.as_f64()), Some(0.1));
    }
}
