//! Speech-to-text for recordings — shell out to a locally-installed whisper and parse its
//! JSON into time-aligned transcript segments, so the Ask-panel can quote what was *said*,
//! not just what was on screen. Two backends are auto-detected:
//!   * **openai-whisper** CLI (`whisper`), JSON `segments[]` with float `start`/`end`;
//!   * **whisper.cpp** (`whisper-cli` / `whisper-cpp`), JSON `transcription[]` with ms `offsets`.
//!
//! No binary/model installed → empty transcript (the recording still indexes via OCR + the
//! veyo visual timeline). Audio never leaves the box — transcription is local.

use clipxd_index::TranscriptSegment;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Transcribe a `.wav` into segments. Empty on any failure or when no whisper is installed.
pub fn transcribe(audio: &Path) -> Vec<TranscriptSegment> {
    if !audio.exists() {
        return Vec::new();
    }
    run_whisper(audio).unwrap_or_default()
}

fn run_whisper(audio: &Path) -> Option<Vec<TranscriptSegment>> {
    let dir = audio.parent()?;
    let stem = audio.file_stem()?.to_str()?;

    // openai-whisper CLI → <dir>/<stem>.json with float-second segments
    if let Some(bin) = which(&["whisper"]) {
        let ok = Command::new(&bin)
            .arg(audio)
            .args(["--model", "base", "--output_format", "json", "--fp16", "False", "--output_dir"])
            .arg(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            let json = dir.join(format!("{stem}.json"));
            let txt = std::fs::read_to_string(&json).ok()?;
            let _ = std::fs::remove_file(&json);
            return Some(parse_openai_whisper(&txt));
        }
    }

    // whisper.cpp → <base>.json with millisecond offsets (needs a ggml model)
    if let Some(bin) = which(&["whisper-cli", "whisper-cpp"]) {
        let model = whisper_cpp_model()?;
        let base = dir.join(stem);
        let ok = Command::new(&bin)
            .args(["-m"])
            .arg(&model)
            .args(["-f"])
            .arg(audio)
            .args(["-oj", "-of"])
            .arg(&base)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            let json = base.with_extension("json");
            let txt = std::fs::read_to_string(&json).ok()?;
            let _ = std::fs::remove_file(&json);
            return Some(parse_whisper_cpp(&txt));
        }
    }
    None
}

fn parse_openai_whisper(json: &str) -> Vec<TranscriptSegment> {
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    v["segments"]
        .as_array()
        .map(|segs| {
            segs.iter()
                .filter_map(|s| {
                    let text = s["text"].as_str()?.trim().to_string();
                    if text.is_empty() {
                        return None;
                    }
                    Some(TranscriptSegment { start: s["start"].as_f64()?, end: s["end"].as_f64()?, speaker: None, text })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_whisper_cpp(json: &str) -> Vec<TranscriptSegment> {
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    v["transcription"]
        .as_array()
        .map(|segs| {
            segs.iter()
                .filter_map(|s| {
                    let text = s["text"].as_str()?.trim().to_string();
                    if text.is_empty() {
                        return None;
                    }
                    let from = s["offsets"]["from"].as_f64()? / 1000.0;
                    let to = s["offsets"]["to"].as_f64()? / 1000.0;
                    Some(TranscriptSegment { start: from, end: to, speaker: None, text })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn which(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for n in names {
            let p = dir.join(n);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn whisper_cpp_model() -> Option<PathBuf> {
    if let Some(m) = std::env::var_os("WHISPER_MODEL") {
        let p = PathBuf::from(m);
        if p.is_file() {
            return Some(p);
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = [
        format!("{home}/.local/share/whisper"),
        "/usr/share/whisper.cpp/models".to_string(),
        "/usr/share/whisper".to_string(),
    ];
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                let is_ggml = p.extension().and_then(|x| x.to_str()) == Some("bin")
                    && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("ggml"));
                if is_ggml {
                    return Some(p);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_whisper_segments() {
        let j = r#"{"segments":[{"start":0.0,"end":2.5,"text":" Deploying to production."},{"start":2.5,"end":4.0,"text":" It failed."}]}"#;
        let segs = parse_openai_whisper(j);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "Deploying to production.");
        assert!((segs[0].end - 2.5).abs() < 1e-9);
    }

    #[test]
    fn parses_whisper_cpp_offsets_to_seconds() {
        let j = r#"{"transcription":[{"offsets":{"from":0,"to":2500},"text":" Hello world"},{"offsets":{"from":2500,"to":3000},"text":"  "}]}"#;
        let segs = parse_whisper_cpp(j);
        assert_eq!(segs.len(), 1, "blank segment dropped");
        assert_eq!(segs[0].text, "Hello world");
        assert!((segs[0].start - 0.0).abs() < 1e-9 && (segs[0].end - 2.5).abs() < 1e-9);
    }

    #[test]
    fn missing_audio_or_binary_yields_empty_never_panics() {
        assert!(transcribe(Path::new("/nonexistent-clipxd-audio.wav")).is_empty());
    }
}
