//! Back-compat guard against a REAL production `index.json`.
//!
//! The `label` field added to `VisualMoment` is `#[serde(default)]`, and there is a unit test
//! for that in isolation — but the thing that actually matters is whether the ~30 clips already
//! sitting on the box (and in S3) still deserialize after the change. A synthetic fixture can
//! agree with the code and both be wrong about the shape real data has. So this pins a verbatim
//! copy of a clip indexed BEFORE the field existed: 16 moments, no `label` key anywhere.
//!
//! If this ever fails, a schema change has just orphaned every existing clip.

use clipxd_index::Index;

const REAL_PROD_INDEX: &str = include_str!("fixtures/pre_label_index.json");

#[test]
fn a_real_pre_label_production_index_still_parses() {
    let idx: Index = serde_json::from_str(REAL_PROD_INDEX).expect("pre-label production index.json must still deserialize");

    // The payload survived, not just the outer envelope.
    assert!(!idx.visual_timeline.is_empty(), "moments should survive the round-trip");
    assert!(
        idx.visual_timeline.iter().all(|m| m.label.is_none()),
        "data written before the field existed must read back as None, never as a default string"
    );
    assert!(
        idx.visual_timeline.iter().any(|m| !m.caption.is_empty()),
        "captions must survive — they are the fallback every consumer renders when label is absent"
    );
}

#[test]
fn re_serializing_a_pre_label_index_does_not_inject_a_label_key() {
    let idx: Index = serde_json::from_str(REAL_PROD_INDEX).unwrap();
    let out = serde_json::to_string(&idx).unwrap();
    // `skip_serializing_if = "Option::is_none"` — an untouched old clip must round-trip without
    // gaining a null field, or every re-write churns S3 and confuses older readers.
    assert!(!out.contains("\"label\""), "None must not serialize into the document");
}
