//! Shared Ollama-primary/NVIDIA/Gemini-fallback text completion — the "cheap LLM call over
//! already-extracted index text" primitive. [`deeppass`](crate::deeppass) (title/tldr/chapters)
//! and [`docgen`](crate::docgen) (PR description/SOP/QA steps) both build on [`complete`]
//! rather than each hand-rolling the same three HTTP calls.
//!
//! **Three backends, tried in order — not a per-call choice:**
//! 1. **Ollama Cloud** (`OLLAMA_API_KEY`, `https://ollama.com/api/chat`) — tried first.
//!    Model tags used here are the bare cloud names (`kimi-k2.6`, not `kimi-k2.6-cloud`); the
//!    `-cloud` suffix is only needed when a *local* `ollama serve` routes a request to the cloud
//!    — calling `ollama.com` directly, every model already is the cloud one.
//! 2. **NVIDIA NIM** (`NVIDIA_API_KEY`) — free-tier hosted inference (Kimi K2 by default; see
//!    `CLIPXD_NVIDIA_MODEL`). No published per-token price as of 2026-07 (confirmed against
//!    `docs.nvidia.com/nim` — no pricing page exists for the hosted endpoint), so it is
//!    explicitly *not* the thing to depend on for guaranteed uptime or cost.
//! 3. **Gemini** (`GEMINI_API_KEY`, model `CLIPXD_GEMINI_MODEL`, default
//!    `gemini-3.1-flash-lite`) — a real, published, stable price
//!    ($0.25/M in, $1.50/M out — ai.google.dev/gemini-api/docs/pricing, confirmed 2026-07).
//!    Used whenever the earlier tiers are unset, or fail for *any* reason (down, rate-limited,
//!    pricing changed, model retired) — the fallback chain exists so any one backend
//!    disappearing overnight doesn't silently turn every feature built on this off.
//!
//! Moondream (image captioning) is a *separate* concern, handled entirely in
//! `veyo_enrich::caption` — Ollama Cloud does not host Moondream (confirmed 2026-07: no
//! `:cloud` tag on its model page, absent from the cloud model catalog), so it cannot replace
//! that key. This module is text-only completion.

use anyhow::{anyhow, bail, Context, Result};

pub fn has_env(key: &str) -> bool {
    std::env::var(key).map(|v| !v.is_empty()).unwrap_or(false)
}

pub fn any_backend_configured() -> bool {
    has_env("OLLAMA_API_KEY") || has_env("NVIDIA_API_KEY") || has_env("GEMINI_API_KEY")
}

/// Complete `prompt` against Ollama Cloud first (if configured), then NVIDIA, then Gemini —
/// each only tried if the previous one is unconfigured or fails outright. Returns the raw
/// completion text plus which backend answered, for logging. `json_mode` asks Ollama for
/// `format: "json"` and Gemini for `responseMimeType: application/json` when the backend
/// supports requesting it — NVIDIA's OpenAI-style chat completions have no such knob, so a
/// caller needing JSON must ask for it in the prompt text itself either way, and tolerate a
/// markdown-fenced response (ordinary chat models do this even when told not to) — see
/// `deeppass::parse_deep_json`'s fence-stripping for the pattern.
///
/// Thin wrapper over [`complete_with_keys`] using the server's own env-configured keys — kept
/// so existing callers that don't care about BYOK don't need to change.
pub async fn complete(prompt: &str, json_mode: bool) -> Result<(String, &'static str)> {
    complete_with_keys(prompt, json_mode, None, None).await
}

/// Same as [`complete`], but lets the caller pass a specific clip owner's own BYOK keys
/// (`Db::llm_keys`) to use *instead of* the server's env-configured NVIDIA/Gemini ones. Ollama
/// Cloud has no BYOK override yet — it's server-env-only (`OLLAMA_API_KEY`) for now. `Some(key)`
/// uses that key; `None` falls back to `NVIDIA_API_KEY`/`GEMINI_API_KEY` from the process
/// environment — so a user's clip is billed against their own account when they've supplied a
/// key, and against the server's shared one otherwise.
pub async fn complete_with_keys(
    prompt: &str,
    json_mode: bool,
    nvidia_key: Option<&str>,
    gemini_key: Option<&str>,
) -> Result<(String, &'static str)> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build()?;

    let ollama = resolve_key(None, "OLLAMA_API_KEY");
    let nvidia = resolve_key(nvidia_key, "NVIDIA_API_KEY");
    let gemini = resolve_key(gemini_key, "GEMINI_API_KEY");

    let mut used = "none";
    let mut result: Option<Result<String>> = None;
    if let Some(key) = ollama.as_deref() {
        let r = call_ollama_cascade(&client, prompt, json_mode, key).await;
        if let Err(e) = &r {
            eprintln!("llm: all Ollama Cloud models failed, falling back to NVIDIA: {e:#}");
        } else {
            used = "ollama";
        }
        result = Some(r);
    }
    if result.as_ref().is_none_or(|r| r.is_err()) {
        if let Some(key) = nvidia.as_deref() {
            let r = call_nvidia_cascade(&client, prompt, key).await;
            if let Err(e) = &r {
                eprintln!("llm: all NVIDIA models failed, falling back to Gemini: {e:#}");
            } else {
                used = "nvidia";
            }
            result = Some(r);
        }
    }
    if result.as_ref().is_none_or(|r| r.is_err()) {
        if let Some(key) = gemini.as_deref() {
            let r = call_gemini(&client, prompt, json_mode, key).await;
            if r.is_ok() {
                used = "gemini";
            }
            result = Some(r);
        }
    }
    let text = result.ok_or_else(|| anyhow!("no LLM backend configured (set OLLAMA_API_KEY, NVIDIA_API_KEY, or GEMINI_API_KEY)"))??;
    Ok((text, used))
}

/// `Some(explicit)` (trimmed, non-empty) wins; otherwise fall back to the named env var.
fn resolve_key(explicit: Option<&str>, env_name: &str) -> Option<String> {
    explicit
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var(env_name).ok().filter(|s| !s.is_empty()))
}

/// Which Ollama Cloud-hosted models to try, in order, when `CLIPXD_OLLAMA_MODEL` isn't set to
/// pin a single one — three different underlying model families (Moonshot, Zhipu/GLM, MiniMax)
/// so a rate limit or outage on one provider is uncorrelated with the others, same reasoning as
/// `NVIDIA_MODEL_CASCADE` below. kimi-k2.6 first since it's the same model already known-good as
/// NVIDIA's primary pick for this exact task.
const OLLAMA_MODEL_CASCADE: &[&str] = &["kimi-k2.6", "glm-5.2", "minimax-m2.7"];

fn ollama_models() -> Vec<String> {
    match std::env::var("CLIPXD_OLLAMA_MODEL").ok().filter(|m| !m.is_empty()) {
        Some(pinned) => vec![pinned],
        None => OLLAMA_MODEL_CASCADE.iter().map(|s| s.to_string()).collect(),
    }
}

/// Try each Ollama Cloud model in turn, returning the first success — same "stay within the
/// tier before giving up on it" reasoning as `call_nvidia_cascade`.
async fn call_ollama_cascade(client: &reqwest::Client, prompt: &str, json_mode: bool, key: &str) -> Result<String> {
    let mut last_err = None;
    for model in ollama_models() {
        match call_ollama(client, prompt, &model, json_mode, key).await {
            Ok(text) => return Ok(text),
            Err(e) => {
                eprintln!("llm: ollama model {model} failed, trying next: {e:#}");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no Ollama models configured")))
}

async fn call_ollama(client: &reqwest::Client, prompt: &str, model: &str, json_mode: bool, key: &str) -> Result<String> {
    let mut body = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": prompt }],
        "stream": false,
    });
    if json_mode {
        body["format"] = "json".into();
    }
    let resp = client
        .post("https://ollama.com/api/chat")
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .context("ollama request")?;
    let status = resp.status();
    let out: serde_json::Value = resp.json().await.context("ollama response")?;
    if !status.is_success() {
        bail!("ollama {status}: {}", out["error"].as_str().unwrap_or("?"));
    }
    out["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("ollama response had no message content"))
}

/// Which NVIDIA-hosted models to try, in order, when `CLIPXD_NVIDIA_MODEL` isn't set to pin a
/// single one. kimi-k2.6 was fastest and highest quality of these on this task in testing (see
/// project memory) — minimax and glm are siblings on the same free tier, so a rate limit or
/// outage on one is likely uncorrelated with the others, unlike retrying the same model.
const NVIDIA_MODEL_CASCADE: &[&str] = &["moonshotai/kimi-k2.6", "minimaxai/minimax-m2.7", "z-ai/glm4.7"];

fn nvidia_models() -> Vec<String> {
    match std::env::var("CLIPXD_NVIDIA_MODEL").ok().filter(|m| !m.is_empty()) {
        Some(pinned) => vec![pinned],
        None => NVIDIA_MODEL_CASCADE.iter().map(|s| s.to_string()).collect(),
    }
}

/// Try each NVIDIA model in turn, returning the first success. Distinct from the
/// NVIDIA→Gemini fallback in `complete()`: this stays *within* the free NVIDIA tier before
/// giving up on it entirely, since a single model being down/rate-limited doesn't mean the
/// whole backend is unavailable.
async fn call_nvidia_cascade(client: &reqwest::Client, prompt: &str, key: &str) -> Result<String> {
    let mut last_err = None;
    for model in nvidia_models() {
        match call_nvidia(client, prompt, &model, key).await {
            Ok(text) => return Ok(text),
            Err(e) => {
                eprintln!("llm: nvidia model {model} failed, trying next: {e:#}");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no NVIDIA models configured")))
}

async fn call_nvidia(client: &reqwest::Client, prompt: &str, model: &str, key: &str) -> Result<String> {
    let body = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": prompt }],
        "temperature": 0.3,
        "max_tokens": 2048,
    });
    let resp = client
        .post("https://integrate.api.nvidia.com/v1/chat/completions")
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .context("nvidia request")?;
    let status = resp.status();
    let out: serde_json::Value = resp.json().await.context("nvidia response")?;
    if !status.is_success() {
        bail!("nvidia {status}: {}", out["error"]["message"].as_str().or_else(|| out["detail"].as_str()).unwrap_or("?"));
    }
    out["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("nvidia response had no message content"))
}

fn gemini_model() -> String {
    // gemini-2.5-flash-lite returned repeated 503 "high demand" errors in testing (2026-07) --
    // likely deprioritized capacity now that newer generations exist. gemini-3.1-flash-lite is
    // the current stable cheap/fast tier and has a real published price.
    std::env::var("CLIPXD_GEMINI_MODEL").ok().filter(|m| !m.is_empty()).unwrap_or_else(|| "gemini-3.1-flash-lite".into())
}

async fn call_gemini(client: &reqwest::Client, prompt: &str, json_mode: bool, key: &str) -> Result<String> {
    let mut generation_config = serde_json::json!({});
    if json_mode {
        generation_config["responseMimeType"] = "application/json".into();
    }
    let body = serde_json::json!({
        "contents": [{ "parts": [{ "text": prompt }] }],
        "generationConfig": generation_config,
    });
    let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", gemini_model());
    let resp = client.post(&url).header("x-goog-api-key", key).json(&body).send().await.context("gemini request")?;
    let status = resp.status();
    let out: serde_json::Value = resp.json().await.context("gemini response")?;
    if !status.is_success() {
        bail!("gemini {status}: {}", out["error"]["message"].as_str().unwrap_or("?"));
    }
    out["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("gemini response had no text part"))
}

/// Strip a markdown code fence if the model wrapped its output in one anyway (common even
/// when explicitly told not to, for both JSON and prose responses).
pub fn strip_fence(text: &str) -> &str {
    let t = text.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```markdown")).or_else(|| t.strip_prefix("```")).unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fence_handles_json_markdown_and_bare() {
        assert_eq!(strip_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_fence("```markdown\n# Title\n```"), "# Title");
        assert_eq!(strip_fence("```\nplain\n```"), "plain");
        assert_eq!(strip_fence("no fence here"), "no fence here");
    }
}
