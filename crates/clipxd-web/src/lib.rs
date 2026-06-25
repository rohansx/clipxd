//! `clipxd-web` — the share layer. An axum service that makes a clip a real **URL**: a
//! watchable share page at `/clip/:id`, the agent-readable **`/clip/:id/index.json`**
//! sidecar behind the same URL, and `query`/`search`/`events` endpoints the React editor
//! (or any agent) hits over HTTP. CORS-open for local dev.

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use clipxd_index::{query, Index};
use clipxd_recorder::{record_from_video, EventTrack};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower_http::cors::CorsLayer;

#[derive(Clone)]
pub struct AppState {
    pub clips_dir: Arc<PathBuf>,
}

/// Build the router serving clips out of `clips_dir`.
pub fn app(clips_dir: PathBuf) -> Router {
    let state = AppState { clips_dir: Arc::new(clips_dir) };
    Router::new()
        .route("/", get(list_clips))
        .route("/clip/:id", get(share_page))
        .route("/clip/:id/index.json", get(get_index))
        .route("/clip/:id/zoom.json", get(get_zoom))
        .route("/clip/:id/query", get(get_query))
        .route("/clip/:id/search", get(get_search))
        .route("/clip/:id/events", get(get_events))
        .route("/clip/:id/video", get(get_video))
        .route("/clip/:id/frames/:name", get(get_frame))
        .route("/ingest", post(ingest))
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

type WebErr = (StatusCode, String);

/// Reject anything that isn't a plain clip-id / filename (no path traversal).
fn safe(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) && !s.contains("..")
}

fn load_index(state: &AppState, id: &str) -> Result<Index, WebErr> {
    if !safe(id) {
        return Err((StatusCode::BAD_REQUEST, "bad clip id".into()));
    }
    let p = state.clips_dir.join(id).join("index.json");
    let txt = std::fs::read_to_string(&p).map_err(|_| (StatusCode::NOT_FOUND, format!("no clip {id}")))?;
    serde_json::from_str(&txt).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("bad index: {e}")))
}

async fn get_index(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<Index>, WebErr> {
    Ok(Json(load_index(&s, &id)?))
}

#[derive(Deserialize)]
struct Qs {
    q: Option<String>,
}

async fn get_query(State(s): State<AppState>, Path(id): Path<String>, Query(p): Query<Qs>) -> Result<Json<serde_json::Value>, WebErr> {
    let idx = load_index(&s, &id)?;
    let a = query::query_clip(&idx, p.q.as_deref().unwrap_or(""));
    Ok(Json(serde_json::json!({ "text": a.text, "citations": a.citations })))
}

async fn get_search(State(s): State<AppState>, Path(id): Path<String>, Query(p): Query<Qs>) -> Result<Json<serde_json::Value>, WebErr> {
    let idx = load_index(&s, &id)?;
    let hits = query::search_text(&idx, p.q.as_deref().unwrap_or(""));
    Ok(Json(serde_json::to_value(hits).unwrap_or_default()))
}

#[derive(Deserialize)]
struct Range {
    from: Option<f64>,
    to: Option<f64>,
}

async fn get_events(State(s): State<AppState>, Path(id): Path<String>, Query(r): Query<Range>) -> Result<Json<serde_json::Value>, WebErr> {
    let idx = load_index(&s, &id)?;
    let (lo, hi) = (r.from.unwrap_or(0.0), r.to.unwrap_or(f64::INFINITY));
    let slice: Vec<_> = idx.event_track.iter().filter(|e| e.t >= lo && e.t <= hi).collect();
    Ok(Json(serde_json::to_value(slice).unwrap_or_default()))
}

async fn get_zoom(State(s): State<AppState>, Path(id): Path<String>) -> Result<impl IntoResponse, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    let p = s.clips_dir.join(&id).join("zoom.json");
    let bytes = std::fs::read(&p).map_err(|_| (StatusCode::NOT_FOUND, "no zoom track".into()))?;
    Ok(([(header::CONTENT_TYPE, "application/json")], bytes))
}

async fn get_video(State(s): State<AppState>, Path(id): Path<String>) -> Result<impl IntoResponse, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    let dir = s.clips_dir.join(&id);
    let file = ["video.mp4", "video.webm", "source.mp4"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.exists())
        .ok_or((StatusCode::NOT_FOUND, "no video".into()))?;
    let ct = if file.extension().and_then(|e| e.to_str()) == Some("webm") { "video/webm" } else { "video/mp4" };
    let bytes = std::fs::read(&file).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(([(header::CONTENT_TYPE, ct)], bytes))
}

async fn get_frame(State(s): State<AppState>, Path((id, name)): Path<(String, String)>) -> Result<impl IntoResponse, WebErr> {
    if !safe(&id) || !safe(&name) {
        return Err((StatusCode::BAD_REQUEST, "bad path".into()));
    }
    let p = s.clips_dir.join(&id).join("frames").join(&name);
    let bytes = std::fs::read(&p).map_err(|_| (StatusCode::NOT_FOUND, "no frame".into()))?;
    let ct = if name.ends_with(".jpg") || name.ends_with(".jpeg") { "image/jpeg" } else { "image/png" };
    Ok(([(header::CONTENT_TYPE, ct)], bytes))
}

/// `POST /ingest` — accept a screen-recording (webm bytes from the browser's MediaRecorder),
/// run it through the same veyo→enrich→index pipeline as the CLI recorder, transcode a clean
/// mp4 for playback, and return the new clip id. This is what makes the Record button real on
/// machines where native screen-grab (scap/PipeWire) can't run — the browser captures, the
/// server indexes. Blocking work (ffmpeg + OCR) runs off the async runtime.
async fn ingest(State(s): State<AppState>, body: Bytes) -> Result<Json<serde_json::Value>, WebErr> {
    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty body".into()));
    }
    let dir = s.clips_dir.clone();
    let join = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        std::fs::create_dir_all(dir.as_path())?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let tmp = std::env::temp_dir().join(format!("clipxd-ingest-{stamp}.webm"));
        std::fs::write(&tmp, &body)?;

        let out = record_from_video(&tmp, &EventTrack::default(), dir.as_path(), 4.0)?;

        // give it a real title instead of the temp filename
        {
            let mut idx = out.index.clone();
            idx.metadata.title = "Screen recording".into();
            let _ = std::fs::write(out.clip_dir.join("index.json"), serde_json::to_string_pretty(&idx).unwrap_or_default());
        }

        // record_from_video copies the source to video.mp4 verbatim; for a webm upload that's
        // mislabeled, so transcode a real H.264 mp4 the browser can play.
        let mp4 = out.clip_dir.join("video.mp4");
        let _ = std::fs::remove_file(&mp4);
        let ok = std::process::Command::new("ffmpeg")
            .args(["-y", "-i"])
            .arg(&tmp)
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .arg(&mp4)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
        if !ok {
            // fall back to serving the original container
            let _ = std::fs::copy(&tmp, out.clip_dir.join("video.webm"));
        }
        let _ = std::fs::remove_file(&tmp);
        Ok(out.clip_dir.file_name().and_then(|n| n.to_str()).unwrap_or("clip").to_string())
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let id = join.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("ingest failed: {e:#}")))?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn list_clips(State(s): State<AppState>) -> Html<String> {
    let mut rows = String::new();
    if let Ok(entries) = std::fs::read_dir(s.clips_dir.as_path()) {
        for e in entries.flatten() {
            let id = e.file_name().to_string_lossy().to_string();
            if e.path().join("index.json").exists() {
                let title = load_index(&s, &id).map(|i| i.metadata.title).unwrap_or_default();
                rows.push_str(&format!(
                    "<li><a href=\"/clip/{id}\">{id}</a> — {}</li>",
                    html_escape(&title)
                ));
            }
        }
    }
    Html(format!(
        "<!doctype html><meta charset=utf-8><title>clipxd</title>\
         <body style='font:15px system-ui;background:#0a0d12;color:#e6edf3;padding:40px'>\
         <h1>clip<span style='color:#58a6ff'>xd</span></h1>\
         <p style='color:#8b97a7'>Record once. Humans watch it. Agents read it.</p>\
         <ul>{rows}</ul></body>"
    ))
}

async fn share_page(State(s): State<AppState>, Path(id): Path<String>) -> Result<Html<String>, WebErr> {
    let idx = load_index(&s, &id)?;
    Ok(Html(share_html(&id, &idx)))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A minimal but real share page: the video + the agent ask box behind the same URL.
fn share_html(id: &str, idx: &Index) -> String {
    let title = html_escape(&idx.metadata.title);
    format!(
        r##"<!doctype html><meta charset=utf-8><title>{title} — clipxd</title>
<body style="font:15px system-ui;background:#0a0d12;color:#e6edf3;margin:0;padding:32px;max-width:920px;margin:auto">
  <h2>{title}</h2>
  <video src="/clip/{id}/video" controls style="width:100%;border-radius:10px;background:#000"></video>
  <p style="color:#8b97a7">{n_ev} events · {n_ost} on-screen text · agent-queryable ·
     <a href="/clip/{id}/index.json" style="color:#58a6ff">index.json</a></p>
  <div style="display:flex;gap:8px;margin:14px 0">
    <input id=q value="what error showed up and what was the user doing right before it"
      style="flex:1;background:#161c27;border:1px solid #232b38;color:#e6edf3;padding:10px;border-radius:8px">
    <button onclick=ask() style="background:#1f6feb;color:#fff;border:0;padding:0 16px;border-radius:8px">Ask</button>
  </div>
  <div id=a style="background:#11161f;border:1px solid #232b38;border-radius:10px;padding:14px"></div>
  <script>
    async function ask() {{
      const q = document.getElementById('q').value;
      const r = await fetch(`/clip/{id}/query?q=${{encodeURIComponent(q)}}`);
      const j = await r.json();
      document.getElementById('a').innerHTML = j.text +
        (j.citations.length? `<div style="margin-top:8px;color:#58a6ff">cited: ${{j.citations.map(c=>c.toFixed(1)+'s').join(', ')}}</div>`:'');
    }}
    ask();
  </script>
</body>"##,
        n_ev = idx.event_track.len(),
        n_ost = idx.on_screen_text.len(),
    )
}
