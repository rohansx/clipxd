//! Optional Tier-2 "deep pass" over a finished clip — **off by default, opt-in by env**.
//!
//! The default pipeline is local-first (PaddleOCR on the box, Moondream for per-keyframe
//! captions) and stays that way. This pass exists for the cloud tier: when explicitly
//! enabled, the *whole video* is sent to Gemini once, which reasons over time + audio
//! jointly and returns what per-frame captioning can't — a narrative title, a real tl;dr,
//! and timestamped chapters — for roughly $0.001–0.002 per minute of video (Flash-Lite,
//! low media resolution). It never runs on the request path: `spawn_phase2` fires it after
//! enrichment, and every failure is logged-and-swallowed (the clip is already complete).
//!
//! Enable with BOTH:
//!   `CLIPXD_DEEP_PASS=gemini`  and  `GEMINI_API_KEY=<key>`
//! Optional: `CLIPXD_GEMINI_MODEL` (default `gemini-2.5-flash-lite`),
//!           `CLIPXD_DEEP_PASS_MAX_MB` (default 512 — skip larger videos).

use anyhow::{anyhow, bail, Context, Result};
use clipxd_index::Index;
use std::path::Path;

const API_BASE: &str = "https://generativelanguage.googleapis.com";

pub fn enabled() -> bool {
    std::env::var("CLIPXD_DEEP_PASS").map(|v| v.eq_ignore_ascii_case("gemini")).unwrap_or(false)
        && std::env::var("GEMINI_API_KEY").map(|k| !k.is_empty()).unwrap_or(false)
}

fn model() -> String {
    // gemini-2.5-flash-lite returned repeated 503 "high demand" errors in testing (2026-07) --
    // likely deprioritized capacity now that newer generations exist. gemini-3.1-flash-lite is
    // the current stable cheap/fast tier and worked cleanly (17.6s for a whole-video pass on a
    // ~27s clip). Re-verify with `curl .../v1beta/models` against your key if this starts
    // failing again -- Google's naming/availability shifts faster than this comment will.
    std::env::var("CLIPXD_GEMINI_MODEL").ok().filter(|m| !m.is_empty()).unwrap_or_else(|| "gemini-3.1-flash-lite".into())
}

/// What the deep pass asks for — matches the fields it is allowed to merge into the index.
#[derive(serde::Deserialize)]
struct DeepResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    tldr: String,
    #[serde(default)]
    chapters: Vec<DeepChapter>,
}

#[derive(serde::Deserialize)]
struct DeepChapter {
    #[serde(default)]
    start: f64,
    #[serde(default)]
    title: String,
}

/// Run the deep pass for the clip in `clip_dir` and merge the result into its `index.json`.
/// Merge rules are conservative: the title is only set while it's still the recorder's
/// default (never stomp a user edit), tl;dr/chapters only when Gemini returned something.
pub async fn run(clip_dir: &Path, id: &str) -> Result<()> {
    let video = ["video.webm", "video.mp4", "source.mp4"]
        .iter()
        .map(|n| clip_dir.join(n))
        .find(|p| p.exists())
        .ok_or_else(|| anyhow!("no video file in {}", clip_dir.display()))?;
    let size = std::fs::metadata(&video)?.len();
    let max_mb: u64 = std::env::var("CLIPXD_DEEP_PASS_MAX_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(512);
    if size > max_mb * 1024 * 1024 {
        bail!("video is {} MB > CLIPXD_DEEP_PASS_MAX_MB ({max_mb}) — skipping", size / (1024 * 1024));
    }
    let mime = if video.extension().and_then(|e| e.to_str()) == Some("webm") { "video/webm" } else { "video/mp4" };
    let key = std::env::var("GEMINI_API_KEY").context("GEMINI_API_KEY")?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    let file = upload_video(&client, &key, &video, mime, size, id).await?;
    let outcome = generate(&client, &key, &file.uri, mime).await;
    // Best-effort hygiene: the uploaded copy auto-expires after 48h anyway, but delete it
    // now — the user's recording shouldn't linger on a third party longer than the request.
    let _ = client
        .delete(format!("{API_BASE}/v1beta/{}", file.name))
        .header("x-goog-api-key", &key)
        .send()
        .await;
    let deep = outcome?;

    merge_into_index(clip_dir, &deep)?;
    eprintln!(
        "deep pass ({}): merged title/tldr/{} chapters for {id}",
        model(),
        deep.chapters.len()
    );
    Ok(())
}

struct UploadedFile {
    name: String,
    uri: String,
}

/// Files-API resumable upload (single shot: start → upload+finalize → poll ACTIVE).
async fn upload_video(client: &reqwest::Client, key: &str, video: &Path, mime: &str, size: u64, id: &str) -> Result<UploadedFile> {
    let start = client
        .post(format!("{API_BASE}/upload/v1beta/files"))
        .header("x-goog-api-key", key)
        .header("X-Goog-Upload-Protocol", "resumable")
        .header("X-Goog-Upload-Command", "start")
        .header("X-Goog-Upload-Header-Content-Length", size.to_string())
        .header("X-Goog-Upload-Header-Content-Type", mime)
        .json(&serde_json::json!({ "file": { "display_name": id } }))
        .send()
        .await
        .context("files:start")?;
    let upload_url = start
        .headers()
        .get("x-goog-upload-url")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("files:start returned no upload url (status {})", start.status()))?;

    let body = reqwest::Body::from(tokio::fs::File::open(video).await?);
    let uploaded: serde_json::Value = client
        .post(&upload_url)
        .header("Content-Length", size.to_string())
        .header("X-Goog-Upload-Offset", "0")
        .header("X-Goog-Upload-Command", "upload, finalize")
        .body(body)
        .send()
        .await
        .context("files:upload")?
        .json()
        .await
        .context("files:upload response")?;
    let name = uploaded["file"]["name"].as_str().ok_or_else(|| anyhow!("upload response missing file.name"))?.to_string();
    let uri = uploaded["file"]["uri"].as_str().ok_or_else(|| anyhow!("upload response missing file.uri"))?.to_string();

    // Poll until Gemini finishes transcoding (state PROCESSING → ACTIVE).
    let mut state = uploaded["file"]["state"].as_str().unwrap_or("PROCESSING").to_string();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    while state == "PROCESSING" {
        if std::time::Instant::now() > deadline {
            bail!("file {name} still PROCESSING after 300s");
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let f: serde_json::Value = client
            .get(format!("{API_BASE}/v1beta/{name}"))
            .header("x-goog-api-key", key)
            .send()
            .await?
            .json()
            .await?;
        state = f["state"].as_str().unwrap_or("PROCESSING").to_string();
    }
    if state != "ACTIVE" {
        bail!("file {name} ended in state {state}");
    }
    Ok(UploadedFile { name, uri })
}

const PROMPT: &str = "You are indexing a screen recording so software agents can answer questions about it \
without watching. Watch the whole video (use the audio too) and return JSON only, shaped as \
{\"title\": string, \"tldr\": string, \"chapters\": [{\"start\": number, \"title\": string}]}. \
`title`: one specific, concrete line naming what the recording is about. `tldr`: 2-4 sentences \
narrating what happens in order, naming visible apps, actions, and any errors verbatim. \
`chapters`: 3-12 entries; `start` is the chapter's first moment in seconds from the beginning.";

async fn generate(client: &reqwest::Client, key: &str, file_uri: &str, mime: &str) -> Result<DeepResult> {
    let request = |media_resolution: bool| {
        let mut generation_config = serde_json::json!({ "responseMimeType": "application/json" });
        if media_resolution {
            // 64 tokens/frame instead of 258 — the research-verified cost lever for screen
            // content. Retried without it in case an older API rejects the field.
            generation_config["mediaResolution"] = "MEDIA_RESOLUTION_LOW".into();
        }
        serde_json::json!({
            "contents": [{ "parts": [
                { "file_data": { "mime_type": mime, "file_uri": file_uri } },
                { "text": PROMPT },
            ]}],
            "generationConfig": generation_config,
        })
    };
    let url = format!("{API_BASE}/v1beta/models/{}:generateContent", model());
    let mut resp = client.post(&url).header("x-goog-api-key", key).json(&request(true)).send().await?;
    if !resp.status().is_success() {
        resp = client.post(&url).header("x-goog-api-key", key).json(&request(false)).send().await?;
    }
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.context("generateContent response")?;
    if !status.is_success() {
        bail!("generateContent {status}: {}", body["error"]["message"].as_str().unwrap_or("?"));
    }
    let text = body["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or_else(|| anyhow!("generateContent returned no text part"))?;
    serde_json::from_str(text).with_context(|| format!("deep-pass JSON parse: {text:.200}"))
}

fn merge_into_index(clip_dir: &Path, deep: &DeepResult) -> Result<()> {
    let path = clip_dir.join("index.json");
    let mut index: Index = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let title = deep.title.trim();
    if !title.is_empty() && (index.metadata.title.is_empty() || index.metadata.title == "Screen recording") {
        index.metadata.title = title.to_string();
    }
    if !deep.tldr.trim().is_empty() {
        index.summary.tldr = deep.tldr.trim().to_string();
    }
    if !deep.chapters.is_empty() {
        index.summary.chapters = deep
            .chapters
            .iter()
            .filter(|c| !c.title.trim().is_empty())
            .map(|c| clipxd_index::Chapter { start: c.start.max(0.0), title: c.title.trim().to_string() })
            .collect();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&index)?)?;
    Ok(())
}
