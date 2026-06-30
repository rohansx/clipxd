//! `clipxd-web` — the share layer. An axum service that makes a clip a real **URL**: a
//! watchable share page at `/clip/:id`, the agent-readable **`/clip/:id/index.json`**
//! sidecar behind the same URL, and `query`/`search`/`events` endpoints the React editor
//! (or any agent) hits over HTTP. CORS-open for local dev.

use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use clipxd_index::{query, Index};
use clipxd_recorder::EventTrack;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower_http::cors::CorsLayer;

pub mod auth;
use auth::{AuthState, AuthUser};

#[derive(Clone)]
pub struct AppState {
    pub clips_dir: Arc<PathBuf>,
    /// Read-only public mode: drops ingest/render/cursor + clip enumeration so the server is
    /// safe to expose over a tunnel — a viewer can watch/ask one (unguessable) clip and nothing more.
    pub public: bool,
    /// Public base URL (e.g. a Tailscale-Funnel `https://…ts.net` origin) the editor's Share
    /// button should hand out instead of the LAN address.
    pub public_base: Option<Arc<String>>,
    /// Multi-tenant auth (accounts + per-user clip ownership). `None` = local/LAN mode (no auth).
    pub auth: Option<AuthState>,
    /// Storage backend. Today this is `Local` only; the `CLIPXD_STORAGE=s3://...` knob is
    /// parsed at boot so the env-file contract is stable. When set, we log a WARN and fall
    /// through to local reads so misconfiguration is loud but doesn't break anything.
    pub storage_kind: StorageKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageKind {
    Local,
    /// URL like `s3://bucket/prefix?endpoint=https://…&region=auto`. Captured for future
    /// implementation; the actual S3 read/write path is not yet wired.
    S3Configured { bucket: String, prefix: String },
}

/// Build the router serving clips out of `clips_dir`. With `public = true` the mutating and
/// listing routes are omitted entirely (404), leaving only the read-only watch/ask surface —
/// the safe set to put behind a public tunnel.
pub fn app(clips_dir: PathBuf, public: bool) -> Router {
    let public_base = std::env::var("CLIPXD_PUBLIC_BASE").ok().filter(|s| !s.is_empty()).map(Arc::new);
    // Multi-tenant auth when CLIPXD_AUTH=1 (the hosted deploy). Misconfig is a fatal boot error.
    let auth = if std::env::var("CLIPXD_AUTH").map(|v| v == "1" || v == "true").unwrap_or(false) {
        Some(AuthState::from_env(&clips_dir).expect("auth init (CLIPXD_AUTH=1) failed"))
    } else {
        None
    };
    let has_auth = auth.is_some();
    let storage_kind = parse_storage_kind();
    let state = AppState { clips_dir: Arc::new(clips_dir), public, public_base, auth, storage_kind };
    // read-only surface — always present, safe to expose publicly (unguessable share links)
    let mut router = Router::new()
        .route("/clip/:id", get(share_page))
        .route("/clip/:id/index.json", get(get_index))
        .route("/clip/:id/zoom.json", get(get_zoom))
        .route("/clip/:id/query", get(get_query))
        .route("/clip/:id/search", get(get_search))
        .route("/clip/:id/events", get(get_events))
        .route("/clip/:id/video", get(get_video))
        .route("/clip/:id/frames/:name", get(get_frame))
        // Username-canonical share-link form: /u/:username/clip/:id and all the same
        // sub-resources. Resolved via ownership (404 if the clip isn't owned by that user).
        .route("/u/:username/clip/:id", get(share_page_for_user))
        .route("/u/:username/clip/:id/index.json", get(get_index_for_user))
        .route("/u/:username/clip/:id/zoom.json", get(get_zoom_for_user))
        .route("/u/:username/clip/:id/query", get(get_query_for_user))
        .route("/u/:username/clip/:id/search", get(get_search_for_user))
        .route("/u/:username/clip/:id/events", get(get_events_for_user))
        .route("/u/:username/clip/:id/video", get(get_video_for_user))
        .route("/u/:username/clip/:id/frames/:name", get(get_frame_for_user))
        .route("/net", get(get_net));
    router = if public {
        router.route("/", get(public_root))
    } else {
        // local/full surface — enumeration + mutation. With auth on, these self-check the JWT
        // and scope to the owner; without auth (LAN mode) they're open as before.
        router
            .route("/", get(list_clips))
            .route("/clips", get(list_clips_json))
            .route("/clip/:id/render", post(render_clip))
            .route("/clip/:id/cursor", post(set_cursor))
            .route("/clip/:id/claim", post(clip_claim))
            .route("/clip/:id/re-enrich", post(clip_re_enrich))
            .route("/ingest", post(ingest))
            .route("/import", post(import_url))
    };
    if has_auth {
        router = router
            .route("/auth/signup", post(auth_signup))
            .route("/auth/login", post(auth_login))
            .route("/auth/logout", post(auth_logout))
            .route("/auth/me", get(auth_me))
            .route("/auth/username", post(auth_set_username))
            .route("/auth/github", get(github_start))
            .route("/auth/github/callback", get(github_callback));
    }
    // Tunneled ingest: only meaningful when `CLIPXD_YT_TUNNEL_URL` is set. The forwarder
    // (your home box) calls this with the video bytes it pulled via yt-dlp.
    // We expose it even when auth is off (so a single-user LAN setup can use the tunnel too);
    // auth is the `?token=<shared-secret>` query param matching CLIPXD_YT_TUNNEL_URL.
    router = router
        .route("/ingest/tunneled", post(ingest_tunneled));
    router
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

// ---- auth helpers shared by the editor handlers ----

/// In auth mode, require a valid session and return the user; in local mode, return `None` (open).
fn require_user(s: &AppState, headers: &HeaderMap) -> Result<Option<AuthUser>, WebErr> {
    match &s.auth {
        None => Ok(None),
        Some(a) => auth::authenticate(&a.jwt_secret, headers)
            .map(Some)
            .ok_or((StatusCode::UNAUTHORIZED, "login required".into())),
    }
}

/// In auth mode, require that the caller owns `id` (unowned legacy clips are allowed through);
/// in local mode, always allow.
fn require_clip_access(s: &AppState, headers: &HeaderMap, id: &str) -> Result<(), WebErr> {
    let Some(a) = &s.auth else { return Ok(()) };
    let user = auth::authenticate(&a.jwt_secret, headers).ok_or((StatusCode::UNAUTHORIZED, "login required".into()))?;
    match a.db.clip_owner(id).ok().flatten() {
        Some(owner) if owner == user.id => Ok(()),
        None => Ok(()), // pre-auth clip with no recorded owner — allow
        Some(_) => Err((StatusCode::FORBIDDEN, "not your clip".into())),
    }
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())?
        .split(';')
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

fn auth_of(s: &AppState) -> Result<&AuthState, WebErr> {
    s.auth.as_ref().ok_or((StatusCode::NOT_FOUND, "auth disabled".into()))
}

fn user_json(u: &auth::User) -> serde_json::Value {
    serde_json::json!({
        "id": u.id,
        "email": u.email,
        "name": u.name,
        "username": u.username,
        "github": u.github_id.is_some(),
    })
}

/// Parse the `CLIPXD_STORAGE` env var. Today only the `local` form is implemented; setting an
/// `s3://...` URL will parse the bucket+prefix+endpoint and log a warning, then fall through to
/// local reads. When the S3 read/write path lands, only this function changes.
fn parse_storage_kind() -> StorageKind {
    let raw = match std::env::var("CLIPXD_STORAGE") {
        Ok(s) if !s.is_empty() => s,
        _ => return StorageKind::Local,
    };
    if raw == "local" || raw.starts_with("file://") {
        return StorageKind::Local;
    }
    if let Some(rest) = raw.strip_prefix("s3://") {
        // s3://bucket[/prefix]?endpoint=...&region=...
        let (path, _query) = rest.split_once('?').unwrap_or((rest, ""));
        let mut parts = path.splitn(2, '/');
        let bucket = parts.next().unwrap_or("").to_string();
        let prefix = parts.next().unwrap_or("").trim_end_matches('/').to_string();
        if !bucket.is_empty() {
            eprintln!(
                "WARN CLIPXD_STORAGE=s3 is set (bucket={bucket}, prefix={prefix}) but the S3 read/write path is not yet implemented; falling back to local disk."
            );
            return StorageKind::S3Configured { bucket, prefix };
        }
    }
    eprintln!(
        "WARN CLIPXD_STORAGE={raw:?} is not a recognised scheme; expected 'local' or 's3://bucket[/prefix]?endpoint=...&region=...'. Falling back to local."
    );
    StorageKind::Local
}

/// JSON response that sets the session cookie AND returns the JWT in the body — the cookie is
/// the secure path for same-origin production; the body token lets the SPA use a Bearer header
/// when it talks to the API cross-origin (local dev), where the cookie wouldn't be sent.
fn json_with_session(jwt: &str, a: &AuthState, mut body: serde_json::Value) -> Response {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("token".into(), serde_json::Value::String(jwt.to_string()));
    }
    let mut resp = Json(body).into_response();
    if let Ok(c) = auth::session_cookie(jwt, a.cookie_secure).parse() {
        resp.headers_mut().insert(header::SET_COOKIE, c);
    }
    resp
}

#[derive(Deserialize)]
struct SignupReq {
    email: String,
    password: String,
    name: Option<String>,
    /// Chosen URL slug for share links. Optional — if missing, the user is created without one
    /// and can claim one later via `POST /auth/username`.
    username: Option<String>,
}

async fn auth_signup(State(s): State<AppState>, Json(req): Json<SignupReq>) -> Result<Response, WebErr> {
    let a = auth_of(&s)?;
    let email = req.email.trim().to_lowercase();
    if !email.contains('@') || email.len() < 3 {
        return Err((StatusCode::BAD_REQUEST, "a valid email is required".into()));
    }
    if req.password.len() < 8 {
        return Err((StatusCode::BAD_REQUEST, "password must be at least 8 characters".into()));
    }
    if a.db.find_by_email(&email).ok().flatten().is_some() {
        return Err((StatusCode::CONFLICT, "that email is already registered".into()));
    }
    let hash = auth::hash_password(&req.password).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Validate the username up front so we return a clean 400 with a helpful message instead
    // of a generic "username taken" / 500 from the DB layer.
    let username = match req.username.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(raw) => Some(
            auth::validate_username(raw)
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?,
        ),
    };
    let user = a.db
        .create_password_user(&email, &hash, req.name.as_deref(), username.as_deref())
        .map_err(|e| match e.to_string().as_str() {
            "username taken" => (StatusCode::CONFLICT, "username taken".into()),
            other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
        })?;
    let jwt = auth::issue_jwt(&a.jwt_secret, &user).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(json_with_session(&jwt, a, user_json(&user)))
}

#[derive(Deserialize)]
struct LoginReq {
    email: String,
    password: String,
}

async fn auth_login(State(s): State<AppState>, Json(req): Json<LoginReq>) -> Result<Response, WebErr> {
    let a = auth_of(&s)?;
    let email = req.email.trim().to_lowercase();
    let user = a.db.find_by_email(&email).ok().flatten().ok_or((StatusCode::UNAUTHORIZED, "invalid email or password".into()))?;
    let ok = user.pw_hash.as_deref().map(|h| auth::verify_password(&req.password, h)).unwrap_or(false);
    if !ok {
        return Err((StatusCode::UNAUTHORIZED, "invalid email or password".into()));
    }
    let jwt = auth::issue_jwt(&a.jwt_secret, &user).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(json_with_session(&jwt, a, user_json(&user)))
}

async fn auth_me(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<serde_json::Value>, WebErr> {
    let a = auth_of(&s)?;
    let principal = auth::authenticate(&a.jwt_secret, &headers).ok_or((StatusCode::UNAUTHORIZED, "not logged in".into()))?;
    let user = a.db.find_by_id(principal.id).ok().flatten().ok_or((StatusCode::UNAUTHORIZED, "not logged in".into()))?;
    Ok(Json(user_json(&user)))
}

#[derive(Deserialize)]
struct SetUsernameReq { username: String }

async fn auth_set_username(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SetUsernameReq>,
) -> Result<Json<serde_json::Value>, WebErr> {
    let a = auth_of(&s)?;
    let principal = auth::authenticate(&a.jwt_secret, &headers)
        .ok_or((StatusCode::UNAUTHORIZED, "not logged in".into()))?;
    let raw = req.username.trim();
    let username = auth::validate_username(raw)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    a.db.set_username(principal.id, &username)
        .map_err(|e| match e.to_string().as_str() {
            "username taken" => (StatusCode::CONFLICT, "username taken".into()),
            other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
        })?;
    let user = a.db.find_by_id(principal.id).ok().flatten()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "user vanished".into()))?;
    Ok(Json(user_json(&user)))
}

async fn auth_logout(State(s): State<AppState>) -> Result<Response, WebErr> {
    let a = auth_of(&s)?;
    let mut resp = Json(serde_json::json!({ "ok": true })).into_response();
    if let Ok(c) = auth::clear_cookie(a.cookie_secure).parse() {
        resp.headers_mut().insert(header::SET_COOKIE, c);
    }
    Ok(resp)
}

async fn github_start(State(s): State<AppState>) -> Result<Response, WebErr> {
    let a = auth_of(&s)?;
    let gh = a.github.as_ref().ok_or((StatusCode::NOT_IMPLEMENTED, "GitHub OAuth not configured".into()))?;
    let st = auth::random_token();
    let redirect = format!("{}/auth/github/callback", a.app_base);
    let url = gh.authorize_url(&redirect, &st);
    let state_cookie = format!(
        "clipxd_oauth_state={st}; HttpOnly; Path=/; SameSite=Lax; Max-Age=600{}",
        if a.cookie_secure { "; Secure" } else { "" }
    );
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, url)
        .header(header::SET_COOKIE, state_cookie)
        .body(Body::empty())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

async fn github_callback(State(s): State<AppState>, Query(q): Query<CallbackQuery>, headers: HeaderMap) -> Result<Response, WebErr> {
    let a = auth_of(&s)?;
    let gh = a.github.as_ref().ok_or((StatusCode::NOT_IMPLEMENTED, "GitHub OAuth not configured".into()))?;
    let code = q.code.ok_or((StatusCode::BAD_REQUEST, "missing code".into()))?;
    // CSRF: the returned state must match the cookie we set in github_start.
    let want = cookie_value(&headers, "clipxd_oauth_state");
    if q.state.is_none() || q.state != want {
        return Err((StatusCode::BAD_REQUEST, "invalid oauth state".into()));
    }
    let redirect = format!("{}/auth/github/callback", a.app_base);
    let ident = auth::exchange_github_code(gh, &code, &redirect).await.map_err(|e| (StatusCode::BAD_GATEWAY, format!("github: {e}")))?;
    let user = a.db.upsert_github(ident.github_id, &ident.email, ident.name.as_deref()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let jwt = auth::issue_jwt(&a.jwt_secret, &user).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Set the session, clear the state cookie, and bounce back to the app.
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, a.app_base.as_str())
        .header(header::SET_COOKIE, auth::session_cookie(&jwt, a.cookie_secure))
        .header(header::SET_COOKIE, format!("clipxd_oauth_state=; HttpOnly; Path=/; Max-Age=0{}", if a.cookie_secure { "; Secure" } else { "" }))
        .body(Body::empty())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
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

async fn get_video(State(s): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, WebErr> {
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
    let len = bytes.len() as u64;

    // Honor a Range request so the editor can seek/scrub (browsers won't seek a <video>
    // that doesn't advertise byte ranges, even when it's fully buffered).
    if let Some((start, end)) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|r| parse_range(r, len)) {
        let slice = bytes[start as usize..=end as usize].to_vec();
        return Ok(Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, ct)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
            .header(header::CONTENT_LENGTH, end - start + 1)
            .body(Body::from(slice))
            .unwrap());
    }
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ct)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len)
        .body(Body::from(bytes))
        .unwrap())
}

/// Parse a single `bytes=start-end` range against a known content length. Returns the
/// inclusive `(start, end)`, clamping the end and rejecting unsatisfiable ranges.
fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let (a, b) = h.strip_prefix("bytes=")?.split_once('-')?;
    let start: u64 = if a.is_empty() { 0 } else { a.parse().ok()? };
    let end: u64 = if b.is_empty() { len - 1 } else { b.parse::<u64>().ok()?.min(len - 1) };
    (start <= end && start < len).then_some((start, end))
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

/// `POST /ingest` — accept a screen-recording (webm bytes from the browser's MediaRecorder).
/// **Loom-style, two-phase:** Phase 1 (fast) saves the video + a minimal `status: enriching`
/// index and returns the clip id *immediately* — so the clip is instantly watchable, listable,
/// and shareable. Phase 2 (the slow OCR/captioning) runs in a background task and rewrites the
/// index to `complete` when done. A recording is therefore never lost to slow/failed enrichment.
async fn ingest(State(s): State<AppState>, headers: HeaderMap, body: Bytes) -> Result<Json<serde_json::Value>, WebErr> {
    let user = require_user(&s, &headers)?;
    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty body".into()));
    }
    let dir = s.clips_dir.clone();
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let id = format!("clp_{:08x}", stamp as u32);

    // Phase 1 — persist the video + a stub index, fast. Returns the clip dir + saved video path.
    let (id, video) = {
        let (dir, id) = (dir.clone(), id.clone());
        tokio::task::spawn_blocking(move || -> anyhow::Result<(String, std::path::PathBuf)> {
            std::fs::create_dir_all(dir.as_path())?;
            let tmp = std::env::temp_dir().join(format!("clipxd-ingest-{id}.webm"));
            std::fs::write(&tmp, &body)?;
            let clip_dir = clipxd_recorder::stub_clip(&tmp, dir.as_path(), &id, "Screen recording")?;
            let _ = std::fs::remove_file(&tmp);
            Ok((id, clip_dir.join("video.webm")))
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("ingest failed: {e:#}")))?
    };

    // Record ownership so this clip shows up only in its creator's library (auth mode).
    if let (Some(a), Some(u)) = (&s.auth, &user) {
        let _ = a.db.set_clip_owner(&id, u.id);
    }

    // Phase 2 — enrich in the background; the clip is already usable. On failure, mark the index
    // `partial` (still watchable) rather than leaving it stuck on `enriching`.
    let clip_dir = dir.join(&id);
    let bg_id = id.clone();
    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            if let Err(e) = clipxd_recorder::enrich_clip(&video, &clip_dir, &bg_id, "Screen recording", &EventTrack::default(), 4.0) {
                eprintln!("background enrich failed for {bg_id}: {e:#}");
                if let Ok(s) = std::fs::read_to_string(clip_dir.join("index.json")) {
                    if let Ok(mut idx) = serde_json::from_str::<Index>(&s) {
                        idx.status = clipxd_index::Status::Partial;
                        let _ = std::fs::write(clip_dir.join("index.json"), serde_json::to_string_pretty(&idx).unwrap_or_default());
                    }
                }
            }
        })
        .await;
    });

    Ok(Json(serde_json::json!({ "id": id })))
}

/// `POST /clip/:id/cursor` — the browser recorder captured a cursor track (pointer moves +
/// clicks, screen-normalized). Save it and **recompute the zoom so the camera follows the
/// cursor** (Screen-Studio style) instead of the veyo content-centroid fallback. The clicks
/// also become queryable `event_track` entries.
/// `POST /clip/:id/claim` — bind an orphaned clip to the calling user. Used when the
/// browser uploads to /ingest but loses its session cookie (tab change mid-upload) so
/// `set_clip_owner` never ran. The user can re-bind by hitting this from the same browser
/// session. The first claimant wins.
async fn clip_claim(State(s): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Result<Json<serde_json::Value>, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    let user = require_user(&s, &headers)?;
    let a = auth_of(&s)?;
    let Some(u) = user else {
        return Err((StatusCode::UNAUTHORIZED, "login required".into()));
    };
    if a.db.clip_owner(&id).ok().flatten().is_some() {
        return Err((StatusCode::CONFLICT, "already owned".into()));
    }
    // Sanity: the clip must exist on disk.
    let p = s.clips_dir.join(&id).join("index.json");
    if !p.exists() {
        return Err((StatusCode::NOT_FOUND, "no such clip".into()));
    }
    a.db.set_clip_owner(&id, u.id).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true, "owner": u.id })))
}

/// `POST /clip/:id/re-enrich` — re-run Phase 2 (captioning/OCR/transcription) on an
/// already-Phase-1-complete clip. Useful when the captioner was offline at the time of
/// recording (e.g. Moondream server on the home box was down). Requires the original video
/// + events.json to be present.
async fn clip_re_enrich(State(s): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Result<Json<serde_json::Value>, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    require_clip_access(&s, &headers, &id)?;
    let clip_dir = s.clips_dir.join(&id);
    let video = ["video.mp4", "video.webm", "source.mp4"]
        .iter()
        .map(|n| clip_dir.join(n))
        .find(|p| p.exists())
        .ok_or((StatusCode::NOT_FOUND, "no video file on disk".into()))?;
    let events_path = clip_dir.join("events.json");
    let events = if events_path.exists() {
        std::fs::read(&events_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<EventTrack>(&b).ok())
            .unwrap_or_default()
    } else {
        EventTrack::default()
    };
    let title = std::fs::read_to_string(clip_dir.join("index.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Index>(&s).ok())
        .map(|i| i.metadata.title)
        .unwrap_or_else(|| "Screen recording".to_string());
    // Re-stub so the SPA sees status=enriching while we work.
    if let Ok(idx_str) = std::fs::read_to_string(clip_dir.join("index.json")) {
        if let Ok(mut idx) = serde_json::from_str::<Index>(&idx_str) {
            idx.status = clipxd_index::Status::Enriching;
            let _ = std::fs::write(clip_dir.join("index.json"), serde_json::to_string_pretty(&idx).unwrap_or_default());
        }
    }
    let bg_id = id.clone();
    let bg_dir = clip_dir.clone();
    tokio::spawn(async move {
        if let Err(e) = clipxd_recorder::enrich_clip(&video, &bg_dir, &bg_id, &title, &events, 4.0) {
            eprintln!("background re-enrich failed for {bg_id}: {e:#}");
            if let Ok(s) = std::fs::read_to_string(bg_dir.join("index.json")) {
                if let Ok(mut idx) = serde_json::from_str::<Index>(&s) {
                    idx.status = clipxd_index::Status::Partial;
                    let _ = std::fs::write(bg_dir.join("index.json"), serde_json::to_string_pretty(&idx).unwrap_or_default());
                }
            }
        }
    });
    Ok(Json(serde_json::json!({ "ok": true, "re_enriching": id })))
}

async fn set_cursor(State(s): State<AppState>, Path(id): Path<String>, headers: HeaderMap, body: Bytes) -> Result<Json<serde_json::Value>, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    require_clip_access(&s, &headers, &id)?;
    let events: EventTrack = serde_json::from_slice(&body).map_err(|e| (StatusCode::BAD_REQUEST, format!("bad events: {e}")))?;
    if events.is_empty() {
        return Ok(Json(serde_json::json!({ "ok": true, "keyframes": 0, "note": "no cursor data" })));
    }
    let dir = s.clips_dir.join(&id);
    let mut index = load_index(&s, &id)?;
    let zoom = clipxd_recorder::zoom_track(&events, index.metadata.duration, index.metadata.fps.max(1.0) as f64);
    index.event_track = clipxd_recorder::to_index_events(&events);
    let _ = std::fs::write(dir.join("events.json"), &body);
    std::fs::write(dir.join("zoom.json"), serde_json::to_string(&zoom).unwrap_or_default())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::write(dir.join("index.json"), serde_json::to_string_pretty(&index).unwrap_or_default())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true, "keyframes": zoom.len(), "events": index.event_track.len() })))
}

/// Resolve the `clipxd` CLI: a sibling release build (fast), then the debug sibling, then PATH.
fn clipxd_bin() -> PathBuf {
    let exe = std::env::current_exe().ok();
    let dir = exe.as_ref().and_then(|p| p.parent());
    let release = dir.and_then(|d| d.parent()).map(|t| t.join("release").join("clipxd"));
    let debug = dir.map(|d| d.join("clipxd"));
    release
        .filter(|p| p.exists())
        .or_else(|| debug.filter(|p| p.exists()))
        .unwrap_or_else(|| PathBuf::from("clipxd"))
}

/// Snapshot the set of clip-dir names currently in `dir` (so we can spot a freshly imported one).
fn clip_dir_names(dir: &std::path::Path) -> std::collections::HashSet<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

#[derive(Deserialize)]
struct ImportReq {
    url: String,
}

/// `POST /import` — import a video URL (Loom/YouTube/Cap/anything yt-dlp understands) into
/// a new clip. Two paths, picked at request time:
///
///   1. **Local forwarder** (preferred): when `CLIPXD_YT_TUNNEL_URL` is set, we POST `{url,
///      callback}` to the user's home box (Tailscale-tunneled) which runs `yt-dlp` there and
///      POSTs the bytes back to our `/ingest/tunneled`. This gets around Loom/YouTube's
///      datacenter-IP blocklist that bites us on the Hetzner side.
///
///   2. **Box-side yt-dlp fallback**: spawns the local `clipxd import` CLI (which calls yt-dlp
///      itself). Useful for sources that don't gate on datacenter IPs (Cap, plain MP4 URLs).
///
/// Local-only (dropped in public mode) since it spawns / writes.
/// `POST /ingest/tunneled` — called by the local yt-dlp forwarder. Body is the raw video bytes.
/// Auth is the `?token=<shared-secret>` query param matching `CLIPXD_YT_TUNNEL_URL`'s trailing
/// path segment. The forwarder is on the user's Tailnet; we don't accept this from the public
/// internet (Caddy doesn't proxy it).
async fn ingest_tunneled(
    State(s): State<AppState>,
    Query(q): Query<TunneledQ>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, WebErr> {
    // Verify shared token.
    let want = yt_tunnel_url().and_then(|u| u.rsplit_once('/').map(|(_, t)| t.to_string())).unwrap_or_default();
    if want.is_empty() || q.token.as_deref() != Some(&want) {
        return Err((StatusCode::UNAUTHORIZED, "bad tunnel token".into()));
    }
    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty body".into()));
    }

    // Trust the forwarder's `X-Clipxd-Filename` for the extension, default to `.mp4`.
    let ext = headers
        .get("x-clipxd-filename")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| std::path::Path::new(s).extension().and_then(|e| e.to_str()))
        .unwrap_or("mp4")
        .to_string();

    let dir = s.clips_dir.clone();
    let id = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        std::fs::create_dir_all(dir.as_path())?;
        let before = clip_dir_names(dir.as_path());
        // Drop the bytes in a tmp file (clipxd import expects a path it can probe).
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let tmp = std::env::temp_dir().join(format!("clipxd-tunnel-{stamp}.{ext}"));
        std::fs::write(&tmp, &body)?;
        let out = std::process::Command::new(clipxd_bin())
            .arg("import")
            .arg(&tmp)
            .arg("--out")
            .arg(dir.as_path())
            .output();
        let _ = std::fs::remove_file(&tmp);
        let out = out?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("; ");
            anyhow::bail!("clipxd import failed ({}): {}", out.status, tail);
        }
        // Same trick as the local path: parse `… → <path>` from stdout, else diff dir, else newest.
        let stdout = String::from_utf8_lossy(&out.stdout);
        if let Some(id) = stdout
            .lines()
            .rev()
            .find_map(|l| l.rsplit_once("→ ").map(|(_, p)| p.trim()))
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .filter(|n| safe(n))
        {
            return Ok(id.to_string());
        }
        let after = clip_dir_names(dir.as_path());
        if let Some(fresh) = after.difference(&before).next() {
            return Ok(fresh.clone());
        }
        let newest = std::fs::read_dir(dir.as_path())?
            .flatten()
            .filter(|e| e.path().is_dir())
            .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
            .and_then(|e| e.file_name().into_string().ok());
        newest.ok_or_else(|| anyhow::anyhow!("no clip produced"))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("tunneled ingest failed: {e:#}")))?;

    // Ownership: when called via the tunnel, we don't know which user wanted the clip. The
    // forwarder should send `X-Clipxd-Owner-Email` for us to resolve. Until that's wired,
    // the clip is unowned (visible to anyone via the share link, just not in any user's library).
    if let (Some(a), Some(owner_email)) = (&s.auth, headers.get("x-clipxd-owner-email").and_then(|v| v.to_str().ok()))
    {
        if let Ok(Some(u)) = a.db.find_by_email(owner_email) {
            let _ = a.db.set_clip_owner(&id, u.id);
        }
    }

    Ok(Json(serde_json::json!({ "id": id })))
}

#[derive(Deserialize)]
struct TunneledQ {
    token: Option<String>,
}

async fn import_url(State(s): State<AppState>, headers: HeaderMap, Json(req): Json<ImportReq>) -> Result<Json<serde_json::Value>, WebErr> {
    let user = require_user(&s, &headers)?;
    let url = req.url.trim().to_string();
    if url.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty url".into()));
    }
    let dir = s.clips_dir.clone();

    // Path 1: tunnel to the local forwarder (your home box running `tools/yt_forwarder.py`).
    if let Some(tunnel) = yt_tunnel_url() {
        match tunnel_fetch_and_post_back(&tunnel, &url).await {
            Ok(id) => {
                if let (Some(a), Some(u)) = (&s.auth, &user) {
                    let _ = a.db.set_clip_owner(&id, u.id);
                }
                return Ok(Json(serde_json::json!({ "id": id })));
            }
            Err(e) => {
                // Forwarder is unreachable / refused: surface that to the caller. We don't
                // silently fall back to box-side yt-dlp because that fails for Loom/YouTube
                // every time and produces a worse error message.
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!("forwarder offline or refused the URL: {e}"),
                ));
            }
        }
    }

    // Path 2: local yt-dlp fallback.
    let id = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        std::fs::create_dir_all(dir.as_path())?;
        let before = clip_dir_names(dir.as_path());
        // Capture output: stdout carries the `✓ imported → <path>` line (deterministic, even
        // under concurrent imports); stderr carries failure detail to surface to the caller.
        let out = std::process::Command::new(clipxd_bin())
            .arg("import")
            .arg(&url)
            .arg("--out")
            .arg(dir.as_path())
            .output()?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("; ");
            anyhow::bail!("clipxd import failed ({}): {}", out.status, tail);
        }
        // Primary: parse the clip dir the CLI reported (`… → <clips_dir>/clp_xxxx`).
        let stdout = String::from_utf8_lossy(&out.stdout);
        if let Some(id) = stdout
            .lines()
            .rev()
            .find_map(|l| l.rsplit_once("→ ").map(|(_, p)| p.trim()))
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .filter(|n| safe(n))
        {
            return Ok(id.to_string());
        }
        // Fallback: the dir that appeared, else newest by mtime.
        let after = clip_dir_names(dir.as_path());
        if let Some(fresh) = after.difference(&before).next() {
            return Ok(fresh.clone());
        }
        let newest = std::fs::read_dir(dir.as_path())?
            .flatten()
            .filter(|e| e.path().is_dir())
            .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
            .and_then(|e| e.file_name().into_string().ok());
        newest.ok_or_else(|| anyhow::anyhow!("no clip produced"))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("import failed: {e:#}")))?;
    if let (Some(a), Some(u)) = (&s.auth, &user) {
        let _ = a.db.set_clip_owner(&id, u.id);
    }
    Ok(Json(serde_json::json!({ "id": id })))
}

/// Read CLIPXD_YT_TUNNEL_URL (e.g. `http://100.94.163.62:8911/<shared-token>`).
fn yt_tunnel_url() -> Option<String> {
    std::env::var("CLIPXD_YT_TUNNEL_URL").ok().filter(|s| !s.is_empty())
}

/// Where the local forwarder should POST the bytes back to us. Defaults to MagicDNS
/// `clipxd-web:8787` on the Tailnet; override with `CLIPXD_YT_TUNNEL_CALLBACK` if your
/// forwarder can't resolve that (e.g. it's not on the Tailnet).
fn yt_tunnel_callback() -> String {
    std::env::var("CLIPXD_YT_TUNNEL_CALLBACK").ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://clipxd-web:8787".to_string())
}

/// POST `{url, callback}` to the forwarder, wait for it to call back our `/ingest/tunneled`,
/// and return the resulting clip id.
async fn tunnel_fetch_and_post_back(tunnel_base: &str, url: &str) -> anyhow::Result<String> {
    // The shared secret = trailing path segment of CLIPXD_YT_TUNNEL_URL. The forwarder echoes
    // it on its callback POST as ?token=… and verified server-side.
    let token = tunnel_base.rsplit_once('/').map(|(_, t)| t).unwrap_or("");
    // Strip the trailing /<token> before POSTing — the forwarder's own URL is at the origin.
    let origin = tunnel_base.rsplit_once('/').map(|(o, _)| o).unwrap_or(tunnel_base);
    let callback = format!("{}/ingest/tunneled?token={token}", yt_tunnel_callback());
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build()?;
    let resp = client
        .post(format!("{origin}/fetch"))
        .json(&serde_json::json!({ "url": url, "callback": callback }))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("forwarder refused (HTTP {}): {}", status, body.chars().take(200).collect::<String>());
    }
    let v: serde_json::Value = resp.json().await?;
    let id = v.get("id").and_then(|s| s.as_str()).ok_or_else(|| anyhow::anyhow!("forwarder response missing 'id'"))?;
    Ok(id.to_string())
}

#[derive(Deserialize)]
struct RenderQ {
    format: Option<String>,
    mockup: Option<bool>,
    bg: Option<String>,
}

/// Whitelist the wallpaper name (preset or hex) so it's safe to pass to the renderer.
fn safe_bg(s: Option<&str>) -> String {
    match s {
        Some(b) if ["aurora", "dusk", "ocean", "violet", "noir", "gradient"].contains(&b) => b.to_string(),
        Some(b) if b.starts_with('#') && b.len() <= 7 && b[1..].chars().all(|c| c.is_ascii_hexdigit()) => b.to_string(),
        _ => "aurora".to_string(),
    }
}

/// `POST /clip/:id/render` — produce the final beautified video (browser mockup + the clip's
/// content-aware auto-zoom from its `zoom.json`) by invoking the `clipxd beautify` renderer,
/// and stream it back as a download. This closes the editor→output loop.
async fn render_clip(State(s): State<AppState>, Path(id): Path<String>, Query(p): Query<RenderQ>, headers: HeaderMap, body: Bytes) -> Result<Response, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    require_clip_access(&s, &headers, &id)?;
    // the POST body, if present, is the editor's .clipxd project (zoom/trim/speed) → bake it in
    let project_file = if body.is_empty() {
        None
    } else {
        let pf = std::env::temp_dir().join(format!("clipxd-proj-{id}.json"));
        std::fs::write(&pf, &body).ok().map(|_| pf)
    };
    let dir = s.clips_dir.join(&id);
    // Imported clips ship a `video.webm`; recorded/ingested ones a transcoded `video.mp4`.
    // `beautify` accepts either, so render whichever exists (mirrors `get_video`'s lookup).
    let video = ["video.mp4", "video.webm", "source.mp4"]
        .iter()
        .map(|f| dir.join(f))
        .find(|p| p.exists())
        .ok_or((StatusCode::NOT_FOUND, "no video".into()))?;
    let zoom = dir.join("zoom.json");
    let events = dir.join("events.json");
    let bg = safe_bg(p.bg.as_deref());
    let fmt = match p.format.as_deref() {
        Some("gif") => "gif",
        Some("webm") => "webm",
        _ => "mp4",
    };
    let mockup = p.mockup.unwrap_or(true);
    let out = std::env::temp_dir().join(format!("clipxd-render-{id}.{fmt}"));
    let bin = clipxd_bin(); // release → debug → PATH
    let (out2, fmt2, proj, bg2, ev2) = (out.clone(), fmt.to_string(), project_file.clone(), bg, events);
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let mut c = std::process::Command::new(&bin);
        c.arg("beautify").arg(&video).args(["--format", &fmt2, "--padding", "8", "--bg", &bg2]);
        if zoom.exists() {
            c.arg("--zoom").arg(&zoom);
        }
        if ev2.exists() {
            c.arg("--events").arg(&ev2); // cursor effects (spotlight + click ripples)
        }
        if let Some(pf) = &proj {
            c.arg("--project").arg(pf);
        }
        if mockup {
            c.arg("--mockup");
        }
        c.arg("--out").arg(&out2);
        let st = c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status()?;
        let b = if st.success() { std::fs::read(&out2)? } else { Vec::new() };
        let _ = std::fs::remove_file(&out2);
        if let Some(pf) = &proj {
            let _ = std::fs::remove_file(pf);
        }
        anyhow::ensure!(!b.is_empty(), "beautify produced no output");
        Ok(b)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("render failed: {e:#}")))?;

    let ct = match fmt {
        "gif" => "image/gif",
        "webm" => "video/webm",
        _ => "video/mp4",
    };
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ct)
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{id}.{fmt}\""))
        .body(Body::from(bytes))
        .unwrap())
}

/// `GET /clips` — JSON list of every clip in the dir (newest first) for the library view.
async fn list_clips_json(State(s): State<AppState>, headers: HeaderMap) -> Json<serde_json::Value> {
    // In auth mode, only the caller's own clips (by recorded ownership).
    let owned: Option<std::collections::HashSet<String>> = match &s.auth {
        Some(a) => match auth::authenticate(&a.jwt_secret, &headers) {
            Some(u) => Some(a.db.clips_for_owner(u.id).unwrap_or_default()),
            None => return Json(serde_json::json!({ "clips": [] })), // not logged in → nothing
        },
        None => None,
    };
    let mut clips: Vec<(std::time::SystemTime, serde_json::Value)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(s.clips_dir.as_path()) {
        for e in entries.flatten() {
            let id = e.file_name().to_string_lossy().to_string();
            if let Some(owned) = &owned {
                if !owned.contains(&id) {
                    continue;
                }
            }
            let Ok(idx) = load_index(&s, &id) else { continue };
            let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            clips.push((
                mtime,
                serde_json::json!({
                    "id": id,
                    "metadata": idx.metadata,
                    "source": idx.source,
                    "status": idx.status,
                    "counts": {
                        "events": idx.event_track.len(),
                        "on_screen_text": idx.on_screen_text.len(),
                        "transcript": idx.transcript.len(),
                        "visual": idx.visual_timeline.len(),
                    },
                }),
            ));
        }
    }
    clips.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    Json(serde_json::json!({ "clips": clips.into_iter().map(|(_, c)| c).collect::<Vec<_>>() }))
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

async fn share_page(State(s): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Result<Html<String>, WebErr> {
    let idx = load_index(&s, &id)?;
    // If the owner has a username, redirect to the canonical /u/<username>/clip/<id> form so
    // share-link brand carries through. Pre-username clips (owner has no slug yet) pass through.
    if let Some(a) = &s.auth {
        if let Some(owner_id) = a.db.clip_owner(&id).ok().flatten() {
            if let Ok(Some(u)) = a.db.find_by_id(owner_id) {
                if let Some(slug) = u.username.as_deref() {
                    let target = format!("/u/{}/clip/{}", slug, id);
                    return Ok(redirect_to(&headers, &target));
                }
            }
        }
    }
    share_page_body(&s, &id, &idx, &headers)
}

/// Resolve /u/:username/clip/:id: ensure the clip is owned by that username, then render.
async fn share_page_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Html<String>, WebErr> {
    let _ = check_owner(&s, &username, &id)?;
    let idx = load_index(&s, &id)?;
    share_page_body(&s, &id, &idx, &headers)
}

/// Confirm `(username, clip_id)` is owned by `username`; 404 otherwise. Used by the
/// /u/:username/clip/:id/* sub-resources to short-circuit any cross-username probing.
fn check_owner(s: &AppState, username: &str, id: &str) -> Result<(), WebErr> {
    let a = auth_of(s)?;
    let user = a.db
        .find_by_username(username)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "not found".into()))?;
    let owner = a.db
        .clip_owner(id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "not found".into()))?;
    if owner != user.id {
        return Err((StatusCode::NOT_FOUND, "not found".into()));
    }
    Ok(())
}

// Username-prefixed sub-resources: same handlers, ownership-checked first.

async fn get_index_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
) -> Result<Json<Index>, WebErr> {
    check_owner(&s, &username, &id)?;
    get_index(State(s), Path(id)).await
}

async fn get_query_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
    Query(p): Query<Qs>,
) -> Result<Json<serde_json::Value>, WebErr> {
    check_owner(&s, &username, &id)?;
    get_query(State(s), Path(id), Query(p)).await
}

async fn get_search_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
    Query(p): Query<Qs>,
) -> Result<Json<serde_json::Value>, WebErr> {
    check_owner(&s, &username, &id)?;
    get_search(State(s), Path(id), Query(p)).await
}

async fn get_events_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
    Query(r): Query<Range>,
) -> Result<Json<serde_json::Value>, WebErr> {
    check_owner(&s, &username, &id)?;
    get_events(State(s), Path(id), Query(r)).await
}

async fn get_zoom_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
) -> Result<impl IntoResponse, WebErr> {
    check_owner(&s, &username, &id)?;
    get_zoom(State(s), Path(id)).await
}

async fn get_video_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, WebErr> {
    check_owner(&s, &username, &id)?;
    get_video(State(s), Path(id), headers).await
}

async fn get_frame_for_user(
    State(s): State<AppState>,
    Path((username, id, name)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, WebErr> {
    check_owner(&s, &username, &id)?;
    get_frame(State(s), Path((id, name))).await
}

fn share_page_body(
    s: &AppState,
    id: &str,
    idx: &Index,
    headers: &HeaderMap,
) -> Result<Html<String>, WebErr> {
    // Absolute URL of THIS page, for the "scan to open on your phone" QR: prefer the public
    // tunnel origin if one is configured, else reconstruct from the request Host.
    let base = s.public_base.as_ref().map(|b| b.to_string()).unwrap_or_else(|| {
        let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("localhost");
        let scheme = headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("https");
        format!("{scheme}://{host}")
    });
    let url = format!("{base}/clip/{id}");
    Ok(Html(share_html(&id, &idx, &url)))
}

fn redirect_to(headers: &HeaderMap, target: &str) -> Html<String> {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("localhost");
    // Trust X-Forwarded-Proto when we're behind Caddy/reverse-proxy, otherwise default to https
    // (production) or http (localhost).
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("https");
    let abs = format!("{scheme}://{host}{target}");
    Html(format!(
        r##"<!doctype html><meta charset=utf-8><meta http-equiv=refresh content="0;url={abs}">
<link rel=canonical href="{abs}"><title>Redirecting…</title>
<body style="font:14px system-ui;background:#0a0d12;color:#e6edf3;padding:32px">
<a href="{abs}" style="color:#58a6ff">{abs}</a>
</body>"##
    ))
}

/// Inline SVG QR for `data` (no external requests — works offline on the viewer's phone).
fn qr_svg(data: &str) -> String {
    match qrcode::QrCode::new(data.as_bytes()) {
        Ok(code) => code
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(150, 150)
            .quiet_zone(true)
            .dark_color(qrcode::render::svg::Color("#0a0d12"))
            .light_color(qrcode::render::svg::Color("#ffffff"))
            .build(),
        Err(_) => String::new(),
    }
}

/// `/net` — tell the editor which base URL the Share button should hand out: the public tunnel
/// origin if one is configured (`CLIPXD_PUBLIC_BASE`), otherwise the **LAN** address (so the
/// link still works for others on the network, not the `127.0.0.1` the operator opened with).
async fn get_net(State(s): State<AppState>, headers: HeaderMap) -> Json<serde_json::Value> {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    let ip = lan_ip().unwrap_or_else(|| "127.0.0.1".to_string());
    let mut body = serde_json::json!({
        "lan_ip": ip,
        "share_base": share_base(host, &ip),
        "public_base": s.public_base.as_ref().map(|b| b.as_str()),
    });
    // If the caller is authenticated, also include their canonical `/u/{username}/clip/…` base.
    if let Some(a) = &s.auth {
        if let Some(principal) = auth::authenticate(&a.jwt_secret, &headers) {
            if let Ok(Some(u)) = a.db.find_by_id(principal.id) {
                if let Some(slug) = u.username.as_deref() {
                    let ubase = s.public_base.as_ref().map(|b| b.as_str()).unwrap_or("https://clipxd.local");
                    let body_mut = body.as_object_mut().unwrap();
                    body_mut.insert("username".into(), serde_json::Value::String(slug.to_string()));
                    body_mut.insert("user_share_base".into(), serde_json::Value::String(format!("{ubase}/u/{slug}/clip")));
                }
            }
        }
    }
    Json(body)
}

/// Public-mode landing: no clip enumeration — a viewer must arrive with a specific clip link.
async fn public_root() -> Html<String> {
    Html(r#"<!doctype html><meta charset=utf-8><title>clipxd</title>
<body style="font:15px system-ui;background:#0a0d12;color:#e6edf3;display:grid;place-items:center;height:100vh;margin:0">
  <div style="text-align:center"><h2>clip<span style="color:#1f6feb">xd</span></h2>
  <p style="color:#8b97a7">Open a specific recording link to watch it &amp; ask it questions.</p></div>
</body>"#.to_string())
}

/// Build `http://<lan-ip>:<port>` — port taken from the request's Host header (the port the
/// client actually reached us on), defaulting to 8787.
fn share_base(host: Option<&str>, lan_ip: &str) -> String {
    let port = host
        .and_then(|h| h.rsplit(':').next())
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8787);
    format!("http://{lan_ip}:{port}")
}

/// Best-effort primary LAN IPv4: "connect" a UDP socket toward a public address (no packets are
/// actually sent) and read which local interface the OS would route through.
fn lan_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback()).then(|| ip.to_string())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A minimal but real share page: the video + the agent ask box + a scan-to-open QR, all
/// behind the same URL.
fn share_html(id: &str, idx: &Index, url: &str) -> String {
    let title = html_escape(&idx.metadata.title);
    let qr = qr_svg(url);
    format!(
        r##"<!doctype html><meta charset=utf-8><title>{title} — clipxd</title>
<body style="font:15px system-ui;background:#0a0d12;color:#e6edf3;margin:0;padding:32px;max-width:920px;margin:auto">
  <div style="display:flex;gap:20px;align-items:flex-start;flex-wrap:wrap">
    <div style="flex:1;min-width:300px">
      <h2>{title}</h2>
      <video src="/clip/{id}/video" controls style="width:100%;border-radius:10px;background:#000"></video>
      <p style="color:#8b97a7">{n_ev} events · {n_ost} on-screen text · agent-queryable ·
         <a href="/clip/{id}/index.json" style="color:#58a6ff">index.json</a></p>
    </div>
    <div style="text-align:center;background:#fff;border-radius:12px;padding:12px 12px 8px;width:174px">
      <div style="width:150px;height:150px">{qr}</div>
      <div style="color:#0a0d12;font-size:12px;margin-top:4px">Scan to open on your phone</div>
    </div>
  </div>
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

#[cfg(test)]
mod tests {
    use super::share_base;

    #[test]
    fn share_base_uses_host_port_and_lan_ip() {
        // port comes from the Host header; the host/ip part is replaced by the detected LAN ip
        assert_eq!(share_base(Some("192.168.1.42:8787"), "192.168.1.42"), "http://192.168.1.42:8787");
        assert_eq!(share_base(Some("localhost:9000"), "10.0.0.5"), "http://10.0.0.5:9000");
        // no port / unparseable → default 8787
        assert_eq!(share_base(None, "10.0.0.5"), "http://10.0.0.5:8787");
        assert_eq!(share_base(Some("box.local"), "10.0.0.5"), "http://10.0.0.5:8787");
    }
}
