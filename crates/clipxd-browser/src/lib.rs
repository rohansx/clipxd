//! `clipxd-browser` — Phase 2: ingest a **browser trace** (DOM mutations · a11y · console ·
//! network · clicks) into the *same* clipxd [`Index`](clipxd_index::Index) as Phase 1.
//!
//! For web flows the DOM is the right primitive — cheaper, exact, and more legible than
//! captioning pixels. There is no pixel codec here: `event_track` is rich and lossless,
//! `on_screen_text` is DOM-verbatim (`source:"dom"`), and `visual_timeline` is derived from
//! **salient events** (errors, failed requests, navigations, alert toasts, and the
//! gesture→request join). Spec: `docs/phase2-browser-spec.md`. Schema-identity with Phase 1
//! is the load-bearing invariant — only `source:"browser"` differs.

pub mod ingest;
pub mod salience;
pub mod trace;

pub use ingest::build_index;
pub use salience::Opts as SalienceOpts;
pub use trace::{BrowserTrace, TraceEvent};

use clipxd_index::Index;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Ingest a trace from a JSON string with an explicit id + created_at (pure; used by tests).
pub fn ingest_str(json: &str, id: &str, created_at: &str, opts: &SalienceOpts) -> anyhow::Result<Index> {
    let trace = BrowserTrace::from_json(json)?;
    Ok(build_index(&trace, id, created_at, opts))
}

/// Ingest a trace file, deriving the clip id from the trace and stamping the current time.
pub fn ingest_path(path: &Path, opts: &SalienceOpts) -> anyhow::Result<Index> {
    let json = std::fs::read_to_string(path)?;
    let trace = BrowserTrace::from_json(&json)?;
    let id = clip_id(&trace);
    Ok(build_index(&trace, &id, &unix_secs(), opts))
}

/// Stable clip id from the trace's `session_id` (or a hash of url + event count).
pub fn clip_id(trace: &BrowserTrace) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if !trace.session_id.is_empty() {
        trace.session_id.hash(&mut h);
    } else {
        trace.url.hash(&mut h);
        trace.events.len().hash(&mut h);
    }
    format!("clp_{:08x}", h.finish() as u32)
}

fn unix_secs() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clipxd_index::{query, Source, TextKind};

    /// The spec's §6 checkout-500 fixture.
    const FIXTURE: &str = r##"{
      "clipxd_trace_version": "1",
      "session_id": "fixture-checkout-500",
      "captured_by": "clipxd-capture-playwright/0.1",
      "started_at_ms": 1710854008000,
      "viewport": { "w": 1280, "h": 800 },
      "url": "https://shop.example.com/cart",
      "events": [
        {"type":"navigate","t_ms":1710854008100,"url":"https://shop.example.com/checkout","from":"https://shop.example.com/cart","nav_kind":"load","title":"Checkout"},
        {"type":"dom_snapshot","t_ms":1710854008120,"url":"https://shop.example.com/checkout","node_count":842,"text":"Checkout\nOrder summary\nPayment\nCard number\nPlace order"},
        {"type":"a11y_text","t_ms":1710854008140,"selector":"main#checkout h1","role":"heading","text":"Checkout"},
        {"type":"a11y_text","t_ms":1710854008150,"selector":"button#place-order","role":"button","text":"Place order"},
        {"type":"screenshot","t_ms":1710854008600,"path":"frames/000001.png","reason":"navigation","redacted":true},
        {"type":"input","t_ms":1710854009200,"target":"input#card-number","label":"Card number","value":"4111 1111 1111 1111","masked":true,"submit":false},
        {"type":"click","t_ms":1710854009800,"click_kind":"click","target":"button#place-order","label":"Place order","x":642,"y":511},
        {"type":"network","t_ms":1710854009900,"method":"POST","url":"https://shop.example.com/api/checkout","status":500,"status_text":"Internal Server Error","resource_type":"fetch","mime":"application/json","duration_ms":1840,"request_id":"req-7f3a","initiator":"script"},
        {"type":"console","t_ms":1710854009920,"level":"error","text":"Checkout failed: HTTP 500 at /api/checkout","stack":["at submitOrder (checkout.js:84:13)"],"source":"javascript","uncaught":false},
        {"type":"dom_mutation","t_ms":1710854009950,"target":"#notifications","op":"insert","added":1,"removed":0,"text_delta":18,"role":"alert","name":"Payment failed (500)"},
        {"type":"a11y_text","t_ms":1710854009960,"selector":"#notifications > .toast","role":"alert","text":"Payment failed (500)"},
        {"type":"screenshot","t_ms":1710854009965,"path":"frames/000002.png","reason":"error","redacted":true},
        {"type":"dom_mutation","t_ms":1710854010360,"target":"#checkout-form","op":"attr","attr":"aria-busy"},
        {"type":"screenshot","t_ms":1710854010400,"path":"frames/000003.png","reason":"state_settle","redacted":true}
      ]
    }"##;

    fn ingest() -> Index {
        ingest_str(FIXTURE, "clp_test", "0", &SalienceOpts::default()).unwrap()
    }

    #[test]
    fn event_track_is_lossless_and_on_screen_text_is_dom_verbatim() {
        let idx = ingest();
        let kinds: Vec<&str> = idx.event_track.iter().map(|e| e.kind.as_str()).collect();
        for k in ["navigation", "input", "click", "network", "console_error", "dom_mutation"] {
            assert!(kinds.contains(&k), "event_track missing {k}: {kinds:?}");
        }
        // the network row knows it's an error
        let net = idx.event_track.iter().find(|e| e.kind == "network").unwrap();
        assert_eq!(net.data.get("status").and_then(|v| v.as_u64()), Some(500));
        assert_eq!(net.data.get("is_error").and_then(|v| v.as_bool()), Some(true));

        // on_screen_text is DOM-verbatim
        assert!(idx.on_screen_text.iter().all(|o| o.source == TextKind::Dom && o.bbox.is_none()));
        let texts: Vec<&str> = idx.on_screen_text.iter().map(|o| o.text.as_str()).collect();
        assert!(texts.iter().any(|t| *t == "Payment failed (500)"));
        assert!(texts.iter().any(|t| t.contains("POST /api/checkout 500")));
        assert!(texts.iter().any(|t| *t == "Checkout"));
    }

    #[test]
    fn masked_card_value_never_reaches_the_index_but_is_audited() {
        let idx = ingest();
        let blob = serde_json::to_string(&idx).unwrap();
        assert!(!blob.contains("4111"), "masked card number leaked into the index!");
        assert!(idx.redaction.ran && !idx.redaction.items.is_empty(), "redaction marker missing");
        assert!(idx.redaction.items.iter().any(|i| i.action == "masked"));
    }

    #[test]
    fn gesture_request_join_and_frame_attach() {
        let idx = ingest();
        let g = idx.visual_timeline.iter().find(|m| m.delta == "gesture_request").expect("gesture_request");
        assert!(g.caption.contains("Place order") && g.caption.contains("POST /api/checkout") && g.caption.contains("500"), "{}", g.caption);
        assert_eq!(g.frame_ref.as_deref(), Some("frames/000002.png"));
        // standalone network_error suppressed by the join (coalescing)
        assert!(!idx.visual_timeline.iter().any(|m| m.delta == "network_error"));
        // the toast is its own salient moment
        assert!(idx.visual_timeline.iter().any(|m| m.delta == "node_inserted" && m.caption.contains("Payment failed (500)")));
    }

    #[test]
    fn schema_identity_with_phase1() {
        let idx = ingest();
        assert_eq!(idx.source, Source::Browser);
        // serializes and round-trips as the SAME Index type Phase 1 produces
        let json = serde_json::to_string(&idx).unwrap();
        let back: Index = serde_json::from_str(&json).unwrap();
        assert_eq!(idx, back);
        // browser specifics live inside payloads, not as new top-level keys
        assert!(json.contains("\"source\":\"browser\""));
        assert!(idx.event_track.iter().all(|e| !e.kind.is_empty()));
    }

    #[test]
    fn query_clip_answers_the_headline_over_a_browser_clip() {
        let idx = ingest();
        let ans = query::query_clip(&idx, "what error showed up and what was the user doing right before it");
        // the error...
        assert!(ans.text.contains("500"), "{}", ans.text);
        // ...and the user action right before it
        assert!(ans.text.to_lowercase().contains("place order"), "{}", ans.text);
        assert!(!ans.citations.is_empty());

        // search finds the 500 in both the network line and the toast
        let hits = query::search_text(&idx, "500");
        assert!(hits.iter().any(|h| h.text.contains("Payment failed (500)")));
        assert!(hits.iter().any(|h| h.text.to_lowercase().contains("checkout")));
    }

    fn from_value(v: serde_json::Value) -> Index {
        let trace: BrowserTrace = serde_json::from_value(v).unwrap();
        build_index(&trace, "c", "0", &SalienceOpts::default())
    }

    #[test]
    fn url_query_tokens_do_not_leak_into_the_index() {
        let idx = from_value(serde_json::json!({
            "started_at_ms": 0, "url": "https://x/", "events": [
                {"type":"network","t_ms":100,"method":"GET","url":"https://x/api?token=SUPERSECRET123&k=v","status":500,"resource_type":"fetch"}
            ]
        }));
        let blob = serde_json::to_string(&idx).unwrap();
        assert!(!blob.contains("SUPERSECRET123"), "URL query token leaked into the index: {blob}");
    }

    #[test]
    fn pathological_dom_snapshot_is_bounded() {
        let text = (0..50_000).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let idx = from_value(serde_json::json!({
            "started_at_ms": 0, "url": "https://x/", "events": [
                {"type":"dom_snapshot","t_ms":0,"text": text}
            ]
        }));
        assert!(idx.on_screen_text.len() <= 300, "snapshot not bounded: {}", idx.on_screen_text.len());
    }
}
