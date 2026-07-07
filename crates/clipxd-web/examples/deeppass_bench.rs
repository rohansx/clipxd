//! Ad hoc benchmark harness: run the Gemini deep pass against an already-ingested clip dir
//! and report wall-clock. Not part of the shipped product — a throwaway for comparing
//! Moondream (per-frame) vs Gemini (whole-video) latency/quality on the same test clip.
//! Usage: cargo run --release -p clipxd-web --example deeppass_bench -- <clip_dir> <id>

use std::time::Instant;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let clip_dir = std::path::PathBuf::from(&args[1]);
    let id = &args[2];
    let t0 = Instant::now();
    match clipxd_web::deeppass::run(&clip_dir, id, None, None).await {
        Ok(()) => println!("deep pass ok in {:.2}s", t0.elapsed().as_secs_f64()),
        Err(e) => println!("deep pass FAILED after {:.2}s: {e:#}", t0.elapsed().as_secs_f64()),
    }
}
