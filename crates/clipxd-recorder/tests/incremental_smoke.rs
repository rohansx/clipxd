//! Verifies the incremental accumulator produces the same deltas/salient coverage as a
//! straight from-scratch `enrich_clip` run on the identical final video. Skips (rather than
//! failing) when `ffmpeg` isn't on PATH — this is the only test in the crate that shells out
//! to real media tooling, so it degrades gracefully on machines without it, same spirit as
//! `transcribe::tests::missing_audio_or_binary_yields_empty_never_panics`.

use clipxd_recorder::{enrich_clip, EventTrack, IncrementalIndexer};
use std::path::Path;
use std::process::Command;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn make_test_video(path: &Path, duration_s: u32) {
    let status = Command::new("ffmpeg")
        .args(["-y", "-f", "lavfi", "-i", &format!("testsrc=duration={duration_s}:size=320x240:rate=10")])
        .args(["-vf", "drawtext=text='frame %{n}':fontcolor=white:fontsize=24:x=10:y=10"])
        .args(["-c:v", "libvpx", "-f", "webm"])
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("ffmpeg should run");
    assert!(status.success(), "test fixture video generation failed");
}

/// Simulates 3 growing "chunks" (as the streaming upload would produce) by truncating a
/// concat-demuxer copy of the source video at 1/3, 2/3, and full length, then feeding each
/// prefix through the incremental accumulator — the same shape `ingest_stage_append` uses.
fn make_prefix(src: &Path, dst: &Path, seconds: f64) {
    let status = Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(src)
        .args(["-t", &seconds.to_string(), "-c", "copy"])
        .arg(dst)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("ffmpeg should run");
    assert!(status.success(), "prefix generation failed");
}

#[test]
fn incremental_matches_batch_on_delta_and_moment_coverage() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg not on PATH");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("clipxd-incr-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let full = tmp.join("full.webm");
    make_test_video(&full, 6);

    // --- incremental path: 3 growing prefixes through IncrementalIndexer ---
    let frames_dir = tmp.join("incr-frames");
    let mut indexer = IncrementalIndexer::new(frames_dir.clone(), 4.0);
    for seconds in [2.0, 4.0, 6.0] {
        let prefix = tmp.join(format!("prefix-{seconds}.webm"));
        make_prefix(&full, &prefix, seconds);
        indexer.add_increment(&prefix, "Screen recording").expect("increment should succeed");
    }
    let clip_dir = tmp.join("clip_incremental");
    std::fs::create_dir_all(&clip_dir).unwrap();
    let incremental_index = indexer.finalize(&full, &clip_dir, "clp_test_incr", "Screen recording", &EventTrack::default()).expect("finalize should succeed");

    // --- batch path: enrich_clip on the same final video, from scratch ---
    let batch_dir = tmp.join("clip_batch");
    std::fs::create_dir_all(&batch_dir).unwrap();
    let batch_index = enrich_clip(&full, &batch_dir, "clp_test_batch", "Screen recording", &EventTrack::default(), 4.0).expect("batch enrich should succeed");

    eprintln!("=== incremental moments ===");
    for m in &incremental_index.visual_timeline {
        eprintln!("  t={} kind={} caption={:?}", m.t, m.delta, m.caption);
    }
    eprintln!("=== batch moments ===");
    for m in &batch_index.visual_timeline {
        eprintln!("  t={} kind={} caption={:?}", m.t, m.delta, m.caption);
    }

    // Same underlying gate over the same final video should retain the same number of
    // salient moments -- the whole point of the incremental design's correctness argument.
    assert_eq!(
        incremental_index.visual_timeline.len(),
        batch_index.visual_timeline.len(),
        "incremental and batch runs should surface the same number of salient moments"
    );
    assert!(!incremental_index.visual_timeline.is_empty(), "a 6s changing testsrc should produce at least one salient moment");

    // Frames actually landed in the final clip_dir (moved out of the incremental scratch dir).
    assert!(clip_dir.join("frames").read_dir().unwrap().next().is_some(), "frames should have been moved into clip_dir");
    assert!(!frames_dir.exists(), "incremental scratch frames dir should have been moved, not left behind");

    std::fs::remove_dir_all(&tmp).ok();
}

/// A black/white/black sequence guarantees a real `RegionChange` + `StateSettle` delta pair
/// (not just keyframe-floor entries), unlike smooth `testsrc` motion which stays under the
/// salience floor. Concatenated via ffmpeg's concat demuxer into one 7s source.
fn make_flash_video(dir: &Path, out: &Path) {
    let segments = [("black", 3u32), ("white", 1u32), ("black", 3u32)];
    let mut list = String::new();
    for (i, (color, dur)) in segments.iter().enumerate() {
        let seg_path = dir.join(format!("seg{i}.webm"));
        let status = Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i", &format!("color=c={color}:s=320x240:rate=10:d={dur}")])
            .args(["-c:v", "libvpx", "-f", "webm"])
            .arg(&seg_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("ffmpeg should run");
        assert!(status.success(), "segment {i} generation failed");
        list.push_str(&format!("file '{}'\n", seg_path.display()));
    }
    let list_path = dir.join("concat.txt");
    std::fs::write(&list_path, list).unwrap();
    let status = Command::new("ffmpeg")
        .args(["-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(&list_path)
        .args(["-c:v", "libvpx", "-f", "webm"])
        .arg(out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("ffmpeg should run");
    assert!(status.success(), "concat failed");
}

#[test]
fn incremental_matches_batch_with_real_deltas_and_uneven_chunk_boundaries() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg not on PATH");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("clipxd-incr-smoke2-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let full = tmp.join("full.webm");
    make_flash_video(&tmp, &full);

    // Uneven boundaries -- not aligned to the flash's own 3/1/3s segment edges, not aligned to
    // the keyframe floor's 2000ms cadence, and not evenly spaced -- to stress the watermark
    // logic against realistic, arbitrary 15s-ish real-world chunk timing.
    let frames_dir = tmp.join("incr-frames");
    let mut indexer = IncrementalIndexer::new(frames_dir.clone(), 4.0);
    for seconds in [1.3, 2.7, 3.4, 5.05, 6.2, 7.0] {
        let prefix = tmp.join(format!("prefix-{seconds}.webm"));
        make_prefix(&full, &prefix, seconds);
        indexer.add_increment(&prefix, "Screen recording").expect("increment should succeed");
    }
    let clip_dir = tmp.join("clip_incremental");
    std::fs::create_dir_all(&clip_dir).unwrap();
    let incremental_index = indexer.finalize(&full, &clip_dir, "clp_test_incr2", "Screen recording", &EventTrack::default()).expect("finalize should succeed");

    let batch_dir = tmp.join("clip_batch");
    std::fs::create_dir_all(&batch_dir).unwrap();
    let batch_index = enrich_clip(&full, &batch_dir, "clp_test_batch2", "Screen recording", &EventTrack::default(), 4.0).expect("batch enrich should succeed");

    eprintln!("=== incremental (uneven chunks) ===");
    for m in &incremental_index.visual_timeline {
        eprintln!("  t={} kind={} caption={:?}", m.t, m.delta, m.caption);
    }
    eprintln!("=== batch (uneven chunks) ===");
    for m in &batch_index.visual_timeline {
        eprintln!("  t={} kind={} caption={:?}", m.t, m.delta, m.caption);
    }

    assert!(
        incremental_index.visual_timeline.iter().any(|m| m.delta == "region_change" || m.delta == "state_settle"),
        "the black/white/black flash should produce at least one real delta-driven moment, not just keyframe floors"
    );
    assert_eq!(
        incremental_index.visual_timeline.len(),
        batch_index.visual_timeline.len(),
        "uneven chunk boundaries should still converge to the same moment count as a batch run"
    );
    assert_eq!(
        incremental_index.on_screen_text.len(),
        batch_index.on_screen_text.len(),
        "on-screen-text coverage should also match"
    );

    std::fs::remove_dir_all(&tmp).ok();
}
