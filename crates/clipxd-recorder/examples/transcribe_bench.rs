//! Ad hoc validation: run the real incremental streaming pipeline against a video with real
//! speech and print the transcript that comes back. Not part of the shipped product.
//! Usage: cargo run --release -p clipxd-recorder --example transcribe_bench -- <dir-with-prefix-*.webm-and-full.webm>

use clipxd_recorder::{EventTrack, IncrementalIndexer};
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: transcribe_bench <dir>"));
    let full = dir.join("full_with_speech.webm");
    let frames_dir = dir.join("bench-frames");
    let mut indexer = IncrementalIndexer::new(frames_dir, 4.0, None);
    for name in ["prefix-10.webm", "prefix-20.7.webm"] {
        let t0 = std::time::Instant::now();
        indexer.add_increment(&dir.join(name), "Screen recording").expect("increment");
        println!("add_increment({name}) in {:.2}s", t0.elapsed().as_secs_f64());
    }
    let clip_dir = dir.join("clip_out");
    std::fs::create_dir_all(&clip_dir).unwrap();
    let t0 = std::time::Instant::now();
    let index = indexer.finalize(&full, &clip_dir, "clp_bench", "Screen recording", &EventTrack::default()).expect("finalize");
    println!("finalize in {:.2}s", t0.elapsed().as_secs_f64());
    println!("\n=== transcript ({} segments) ===", index.transcript.len());
    for seg in &index.transcript {
        println!("  [{:.1}-{:.1}] {}", seg.start, seg.end, seg.text);
    }
}
