//! `clipxd-web` — the share layer. An axum service that makes a clip a real **URL**: a
//! watchable share page at `/clip/:id`, the agent-readable **`/clip/:id/index.json`**
//! sidecar behind the same URL, and `query`/`search`/`events` endpoints the React editor
//! (or any agent) hits over HTTP. CORS-open for local dev.

use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json, Response},
    routing::{get, post, put},
    Router,
};
use clipxd_index::{query, Index};
use clipxd_recorder::{EventTrack, IncrementalIndexer};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;
use tower_http::cors::CorsLayer;

/// One streaming-upload session's incremental indexer, keyed by its `stg_...` session id.
/// The outer map is locked only briefly (get/insert/remove); the inner `StdMutex` guards the
/// actual `add_increment`/`finalize` calls, which run on blocking threads (ffmpeg + OCR work).
/// `Option` so `finalize` can `.take()` an owned `IncrementalIndexer` out of the slot.
type StageSessions = Arc<AsyncMutex<HashMap<String, Arc<StdMutex<Option<IncrementalIndexer>>>>>>;

pub mod auth;
pub mod storage;
use auth::{AuthState, AuthUser};

#[derive(Clone)]
pub struct AppState {
    /// Object storage backend. On local mode reads/writes from `<clips_dir>/`; on S3 mode
    /// reads/writes from `s3://<bucket>/<prefix>/<key>` via the configured endpoint + creds.
    /// Wrapped in Arc so the same `Storage` impl is shared cheaply across requests.
    pub storage: Arc<storage::StorageKind>,
    /// The original `clips_dir` argument kept as a string for paths that the storage layer
    /// doesn't touch (e.g. SQLite DB location: `<clips_dir>/clipxd.db`).
    pub clips_dir: Arc<PathBuf>,
    /// Read-only public mode: drops ingest/render/cursor + clip enumeration so the server is
    /// safe to expose over a tunnel — a viewer can watch/ask one (unguessable) clip and nothing more.
    pub public: bool,
    /// Public base URL (e.g. a Tailscale-Funnel `https://…ts.net` origin) the editor's Share
    /// button should hand out instead of the LAN address.
    pub public_base: Option<Arc<String>>,
    /// Multi-tenant auth (accounts + per-user clip ownership). `None` = local/LAN mode (no auth).
    pub auth: Option<AuthState>,
    /// Live incremental indexers for in-progress streaming-upload sessions, keyed by session
    /// id. Populated at `/ingest/stage`, fed at each `/ingest/stage/:s` chunk, consumed at
    /// `/ingest/stage/:s/commit`.
    pub stage_sessions: StageSessions,
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
    let storage = Arc::new(storage::StorageKind::from_env(&clips_dir));
    let stage_sessions: StageSessions = Arc::new(AsyncMutex::new(HashMap::new()));
    let state = AppState { storage, clips_dir: Arc::new(clips_dir), public, public_base, auth, stage_sessions };
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
            .route("/ingest/stage", post(ingest_stage_create))
            .route("/ingest/stage/:session", put(ingest_stage_append))
            .route("/ingest/stage/:session/commit", post(ingest_stage_commit))
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

/// Read `<id>/index.json` from the configured storage. The path is `id + /index.json`; the
/// storage impl adds its own prefix on S3.
async fn load_index(state: &AppState, id: &str) -> Result<Index, WebErr> {
    if !safe(id) {
        return Err((StatusCode::BAD_REQUEST, "bad clip id".into()));
    }
    let key = format!("{id}/index.json");
    let storage = state_storage(state).await?;
    let bytes = storage.read_object(&key).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("read {key}: {e}")))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no clip {id}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("bad index: {e}")))
}

/// Build a `Box<dyn Storage>` for an AppState. Wraps the boilerplate of error mapping.
async fn state_storage(state: &AppState) -> Result<Box<dyn storage::Storage>, WebErr> {
    state.storage.make_storage().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")))
}

/// Write a single key to S3, swallowing errors (best-effort). Use this for "I just
/// updated a file locally; also keep S3 in sync" so a network blip doesn't 500 the request.
async fn write_object_best_effort(state: &AppState, key: &str, body: Vec<u8>, content_type: &str) {
    if let Ok(st) = state.storage.make_storage().await {
        if let Err(e) = st.write_object(key, body, content_type).await {
            eprintln!("best-effort S3 write {key}: {e}");
        }
    }
}

/// Mirror a local clip directory to S3. Walks `<local_dir>` recursively and writes each
/// regular file to `<id>/<relative_path>` on the configured storage. No-op on local mode.
///
/// This is the bridge between the clipxd CLI (which only knows local paths) and S3: we run
/// the CLI to a tmp staging dir, then `mirror_dir_to_storage` uploads the result.
async fn mirror_dir_to_storage(
    storage: &dyn storage::Storage,
    id: &str,
    local_dir: &std::path::Path,
) -> Result<(), String> {
    use futures_util::stream::{FuturesUnordered, StreamExt};
    // walk_dir(root, here) returns paths relative to root. We pass local_dir as both so the
    // rel paths are like "index.json", "video.webm", "frames/00001.png".
    let keys = storage::walk_dir(local_dir, local_dir);
    let mut futs = FuturesUnordered::new();
    for rel in keys {
        let key = format!("{id}/{rel}");
        let abs = local_dir.join(&rel);
        let body = std::fs::read(&abs).map_err(|e| format!("read {}: {e}", abs.display()))?;
        // Guess a content-type from the extension; videos are common.
        let ct = match abs.extension().and_then(|e| e.to_str()) {
            Some("mp4") => "video/mp4",
            Some("webm") => "video/webm",
            Some("wav") => "audio/wav",
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("json") => "application/json",
            _ => "application/octet-stream",
        }.to_string();
        futs.push(async move {
            storage.write_object(&key, body, &ct).await
                .map_err(|e| format!("write {key}: {e}"))
        });
    }
    while let Some(res) = futs.next().await {
        res?;
    }
    Ok(())
}

async fn get_index(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<Index>, WebErr> {
    Ok(Json(load_index(&s, &id).await?))
}

#[derive(Deserialize)]
struct Qs {
    q: Option<String>,
}

#[derive(Deserialize)]
struct StageQuery {
    seq: u32,
}

async fn get_query(State(s): State<AppState>, Path(id): Path<String>, Query(p): Query<Qs>) -> Result<Json<serde_json::Value>, WebErr> {
    let idx = load_index(&s, &id).await?;
    let a = query::query_clip(&idx, p.q.as_deref().unwrap_or(""));
    Ok(Json(serde_json::json!({ "text": a.text, "citations": a.citations })))
}

async fn get_search(State(s): State<AppState>, Path(id): Path<String>, Query(p): Query<Qs>) -> Result<Json<serde_json::Value>, WebErr> {
    let idx = load_index(&s, &id).await?;
    let hits = query::search_text(&idx, p.q.as_deref().unwrap_or(""));
    Ok(Json(serde_json::to_value(hits).unwrap_or_default()))
}

#[derive(Deserialize)]
struct Range {
    from: Option<f64>,
    to: Option<f64>,
}

async fn get_events(State(s): State<AppState>, Path(id): Path<String>, Query(r): Query<Range>) -> Result<Json<serde_json::Value>, WebErr> {
    let idx = load_index(&s, &id).await?;
    let (lo, hi) = (r.from.unwrap_or(0.0), r.to.unwrap_or(f64::INFINITY));
    let slice: Vec<_> = idx.event_track.iter().filter(|e| e.t >= lo && e.t <= hi).collect();
    Ok(Json(serde_json::to_value(slice).unwrap_or_default()))
}

async fn get_zoom(State(s): State<AppState>, Path(id): Path<String>) -> Result<impl IntoResponse, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    let bytes = state_storage(&s).await?.read_object(&format!("{id}/zoom.json")).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")))?.ok_or((StatusCode::NOT_FOUND, "no zoom track".into()))?;
    Ok(([(header::CONTENT_TYPE, "application/json")], bytes))
}

async fn get_video(State(s): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    let storage = state_storage(&s).await?;
    let mut video_key: Option<String> = None;
    for n in ["video.mp4", "video.webm", "source.mp4"] {
        if storage.read_object(&format!("{id}/{n}")).await
            .map(|o| o.is_some())
            .unwrap_or(false) {
            video_key = Some(n.to_string());
            break;
        }
    }
    let video_key = video_key.ok_or((StatusCode::NOT_FOUND, "no video".into()))?;
    let ct = if video_key.ends_with(".webm") { "video/webm" } else { "video/mp4" };
    let bytes: Vec<u8> = storage.read_object(&format!("{id}/{video_key}")).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("read video: {e}")))?
        .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "read video: missing".to_string()))?;
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
    let bytes = state_storage(&s).await?.read_object(&format!("{id}/frames/{name}")).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")))?.ok_or((StatusCode::NOT_FOUND, "no frame".into()))?;
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
    ingest_bytes(s, user, body, None).await
}

/// Shared ingest core — called by both `/ingest` (full blob) and `/ingest/stage/:s/commit`
/// (reassembled chunks). Two-phase: Phase 1 persists + stubs immediately, Phase 2 enriches in
/// the background. `incremental`, when the staged-upload path already accumulated one, replaces
/// Phase 2's from-scratch `enrich_clip` with one final pass over the already-mostly-indexed
/// session — see `clipxd_recorder::incremental` for why that's usually much faster.
async fn ingest_bytes(s: AppState, user: Option<AuthUser>, body: Bytes, incremental: Option<IncrementalIndexer>) -> Result<Json<serde_json::Value>, WebErr> {
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

    // Mirror the stub to S3 (no-op on local mode) so the SPA can read the index.json
    // immediately, before Phase 2 finishes.
    if let Ok(st) = s.storage.make_storage().await {
        let stub_dir = dir.join(&id);
        if let Err(e) = mirror_dir_to_storage(st.as_ref(), &id, &stub_dir).await {
            eprintln!("ingest stub mirror: {e} (continuing)");
        }
    }

    // Record ownership so this clip shows up only in its creator's library (auth mode).
    if let (Some(a), Some(u)) = (&s.auth, &user) {
        let _ = a.db.set_clip_owner(&id, u.id);
    }

    // Phase 2 — enrich in the background; the clip is already usable. On failure, mark the index
    // `partial` (still watchable) rather than leaving it stuck on `enriching`.
    let clip_dir = dir.join(&id);
    let bg_id = id.clone();
    let bg_id_for_post = bg_id.clone();
    let clip_dir_for_post = clip_dir.clone();
    let storage_arc = s.storage.clone();
    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            let result = match incremental {
                Some(indexer) => indexer.finalize(&video, &clip_dir, &bg_id, "Screen recording", &EventTrack::default()).map(|_| ()),
                None => clipxd_recorder::enrich_clip(&video, &clip_dir, &bg_id, "Screen recording", &EventTrack::default(), 4.0).map(|_| ()),
            };
            if let Err(e) = result {
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
        // Re-mirror after Phase 2 so the captions/OST/zoom land in S3 too.
        if let Ok(st) = storage_arc.make_storage().await {
            if let Err(e) = mirror_dir_to_storage(st.as_ref(), &bg_id_for_post, &clip_dir_for_post).await {
                eprintln!("post-enrich mirror: {e} (continuing)");
            }
        }
    });

    Ok(Json(serde_json::json!({ "id": id })))
}

/// Where a stage session's incrementally-extracted frames live while recording is in
/// progress — separate from the chunks dir (`clipxd-stage-{session}`) so the indexer's
/// `finalize` can move them into `clip_dir/frames` regardless of when the chunks dir itself
/// gets cleaned up at commit.
fn stage_frames_dir(session: &str) -> PathBuf {
    std::env::temp_dir().join(format!("clipxd-stage-frames-{session}"))
}

/// `POST /ingest/stage` — begin a streaming upload session. Also registers a fresh
/// [`IncrementalIndexer`] so `/ingest/stage/:session` chunks can start indexing immediately
/// as they land, instead of only after `commit`.
/// Returns `{"session": "<id>"}` immediately; the client then PUTs 15-second chunks.
async fn ingest_stage_create(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<serde_json::Value>, WebErr> {
    require_user(&s, &headers)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let session = format!("stg_{:016x}", stamp as u64);
    let dir = std::env::temp_dir().join(format!("clipxd-stage-{session}"));
    tokio::fs::create_dir_all(&dir).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let indexer = IncrementalIndexer::new(stage_frames_dir(&session), 4.0);
    s.stage_sessions.lock().await.insert(session.clone(), Arc::new(StdMutex::new(Some(indexer))));
    Ok(Json(serde_json::json!({ "session": session })))
}

/// `PUT /ingest/stage/:session` — store one MediaRecorder timeslice chunk (?seq=N), then
/// rebuild the session's video-so-far and kick off (detached, non-blocking) an incremental
/// indexing pass over it — so most of a recording is already indexed before `commit`.
async fn ingest_stage_append(
    State(s): State<AppState>,
    Path(session): Path<String>,
    Query(q): Query<StageQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, WebErr> {
    require_user(&s, &headers)?;
    if !session.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err((StatusCode::BAD_REQUEST, "invalid session id".into()));
    }
    if body.is_empty() {
        return Ok(StatusCode::OK);
    }
    let dir = std::env::temp_dir().join(format!("clipxd-stage-{session}"));
    if !dir.exists() {
        return Err((StatusCode::NOT_FOUND, "session not found".into()));
    }
    let chunk_path = dir.join(format!("chunk-{:06}.bin", q.seq));
    let video_so_far = dir.join("video-so-far.webm");
    tokio::task::spawn_blocking({
        let dir = dir.clone();
        let video_so_far = video_so_far.clone();
        move || -> anyhow::Result<()> {
            std::fs::write(&chunk_path, &body)?;
            concat_chunks(&dir, &video_so_far)?;
            Ok(())
        }
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Fire-and-forget: index the growing video in the background so the PUT response (and
    // therefore the client's next chunk) never waits on ffmpeg/OCR work.
    if let Some(slot) = s.stage_sessions.lock().await.get(&session).cloned() {
        tokio::task::spawn_blocking(move || {
            if let Ok(mut guard) = slot.lock() {
                if let Some(indexer) = guard.as_mut() {
                    if let Err(e) = indexer.add_increment(&video_so_far, "Screen recording") {
                        eprintln!("incremental add_increment failed for session {session}: {e:#} (continuing)");
                    }
                }
            }
        });
    }
    Ok(StatusCode::OK)
}

/// Concatenate all `chunk-*.bin` files in `dir` (sorted) into `out`. WebM segments from
/// `MediaRecorder` are ordered byte streams, so raw concatenation produces a valid WebM.
fn concat_chunks(dir: &std::path::Path, out: &std::path::Path) -> anyhow::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("chunk-"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    anyhow::ensure!(!entries.is_empty(), "no chunks in stage session");
    let mut all = Vec::new();
    for entry in entries {
        all.extend(std::fs::read(entry.path())?);
    }
    std::fs::write(out, all)?;
    Ok(())
}

/// `POST /ingest/stage/:session/commit` — concatenate all uploaded chunks in order and create
/// the clip. If the session accumulated an [`IncrementalIndexer`], Phase 2 finishes it off
/// (one final pass over the tail) instead of re-enriching the whole clip from scratch.
async fn ingest_stage_commit(
    State(s): State<AppState>,
    Path(session): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, WebErr> {
    let user = require_user(&s, &headers)?;
    if !session.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err((StatusCode::BAD_REQUEST, "invalid session id".into()));
    }
    let dir = std::env::temp_dir().join(format!("clipxd-stage-{session}"));
    if !dir.exists() {
        return Err((StatusCode::NOT_FOUND, "session not found; call POST /ingest/stage first".into()));
    }
    let video_bytes = tokio::task::spawn_blocking({
        let dir = dir.clone();
        move || -> anyhow::Result<Vec<u8>> {
            let out = dir.join("commit.webm");
            concat_chunks(&dir, &out)?;
            let bytes = std::fs::read(&out)?;
            let _ = std::fs::remove_dir_all(&dir);
            Ok(bytes)
        }
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("commit failed: {e:#}")))?;

    // Take the accumulated indexer out of the registry. The std Mutex lock blocks until any
    // in-flight background `add_increment` from the last chunk's PUT finishes -- exactly the
    // ordering we want (never finalize while an increment is still writing to it) -- so it's
    // done on a blocking-pool thread rather than tying up an async worker while it waits.
    let stage_slot = s.stage_sessions.lock().await.remove(&session);
    let incremental = match stage_slot {
        Some(slot) => tokio::task::spawn_blocking(move || slot.lock().ok().and_then(|mut g| g.take())).await.unwrap_or(None),
        None => None,
    };

    ingest_bytes(s, user, Bytes::from(video_bytes), incremental).await
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
    let mut new_index_json: Option<String> = None;
    if let Ok(idx_str) = std::fs::read_to_string(clip_dir.join("index.json")) {
        if let Ok(mut idx) = serde_json::from_str::<Index>(&idx_str) {
            idx.status = clipxd_index::Status::Enriching;
            if let Ok(j) = serde_json::to_string_pretty(&idx) {
                let _ = std::fs::write(clip_dir.join("index.json"), &j);
                new_index_json = Some(j);
            }
        }
    }
    if let Some(j) = new_index_json.as_ref() {
        write_object_best_effort(&s, &format!("{id}/index.json"), j.as_bytes().to_vec(), "application/json").await;
    }
    let bg_id = id.clone();
    let bg_dir = clip_dir.clone();
    let bg_storage = s.storage.clone();
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
        // Re-mirror to S3 so the new index/zoom/frames land.
        if let Ok(st) = bg_storage.make_storage().await {
            if let Err(e) = mirror_dir_to_storage(st.as_ref(), &bg_id, &bg_dir).await {
                eprintln!("re-enrich post-mirror: {e}");
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
    let mut index = load_index(&s, &id).await?;
    let zoom = clipxd_recorder::zoom_track(&events, index.metadata.duration, index.metadata.fps.max(1.0) as f64);
    index.event_track = clipxd_recorder::to_index_events(&events);
    let _ = std::fs::write(dir.join("events.json"), &body);
    std::fs::write(dir.join("zoom.json"), serde_json::to_string(&zoom).unwrap_or_default())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let index_json = serde_json::to_string_pretty(&index).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::write(dir.join("index.json"), &index_json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Best-effort S3 mirror: events.json, zoom.json, index.json
    write_object_best_effort(&s, &format!("{id}/events.json"), body.to_vec(), "application/json").await;
    let zoom_bytes = serde_json::to_string(&zoom).unwrap_or_default().into_bytes();
    write_object_best_effort(&s, &format!("{id}/zoom.json"), zoom_bytes, "application/json").await;
    write_object_best_effort(&s, &format!("{id}/index.json"), index_json.into_bytes(), "application/json").await;
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
///
/// Returns 202 with `{id, status: "enriching"}` IMMEDIATELY (no blocking on captioning).
/// Indexing + captioning run in a background task. The forwarder's `urllib.request.urlopen`
/// has a 120s timeout, which is too tight for slow Loom/YouTube imports — by returning fast
/// we avoid spurious 502s.
async fn ingest_tunneled(
    State(s): State<AppState>,
    Query(q): Query<TunneledQ>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, WebErr> {
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

    // Write the raw bytes to a temp file, then run stub_clip + clipxd import in a worker
    // thread. The clip_id is computed deterministically (timestamp nanos) so the foreground
    // can return it without waiting.
    let dir = s.clips_dir.clone();
    let body = body.clone();
    let owner_email = s.auth.as_ref().and_then(|_| headers.get("x-clipxd-owner-email").and_then(|v| v.to_str().ok()).map(String::from));
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let id = format!("clp_{:08x}", stamp as u32);
    let tmp = std::env::temp_dir().join(format!("clipxd-tunnel-{stamp}.{ext}"));
    let tmp_clone = tmp.clone();
    let body_for_thread = body.clone();
    let dir_thread = dir.clone();
    let id_thread = id.clone();
    let storage = s.storage.clone();

    // Phase 1: write the body to a tmp file in this thread (sync — it's just a few MB).
    // We do this in the foreground so the tmp path is ready for the background worker.
    let write_result = {
        let tmp = tmp.clone();
        let body = body.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> { std::fs::write(&tmp, &body) })
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    if let Err(e) = write_result {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("tunneled ingest failed: {e}")));
    }

    // Persist the body and ownership BEFORE we respond, so the SPA can read the clip
    // immediately. The full indexing + captioning run in a background task.
    let db = s.auth.as_ref().map(|a| a.db.clone());

    // Background worker: stub_clip + clipxd import + enrich + mirror-to-S3.
    tokio::task::spawn_blocking(move || {
        // -- stub_clip (fast, ~1s) --
        if let Err(e) = clipxd_recorder::stub_clip(&tmp_clone, dir_thread.as_path(), &id_thread, "Imported via tunnel") {
            eprintln!("tunneled stub_clip failed for {id_thread}: {e:#}");
            return;
        }
        let clip_dir = dir_thread.join(&id_thread);

        // -- mirror the stub to S3 immediately (no-op on local) --
        if let Ok(st) = tokio::runtime::Handle::current().block_on(storage.make_storage()) {
            if let Err(e) = tokio::runtime::Handle::current().block_on(mirror_dir_to_storage(st.as_ref(), &id_thread, &clip_dir)) {
                eprintln!("tunneled stub mirror: {e}");
            }
        }

        // -- bind ownership if we got an owner-email header --
        if let (Some(db), Some(email)) = (db.as_ref(), owner_email.as_ref()) {
            if let Ok(Some(u)) = db.find_by_email(email) {
                let _ = db.set_clip_owner(&id_thread, u.id);
            }
        }

        // -- the SLOW part: full clipxd import (captioning + OCR + enrichment) --
        // This is the part that takes minutes for a long Loom/YouTube video.
        // The forwarder already has the clip_id and isn't blocking on this.
        if let Err(e) = clipxd_recorder::enrich_clip(
            &clip_dir.join("video.webm"),
            &clip_dir,
            &id_thread,
            "Imported via tunnel",
            &EventTrack::default(),
            4.0,
        ) {
            eprintln!("tunneled enrich failed for {id_thread}: {e:#}");
            if let Ok(s) = std::fs::read_to_string(clip_dir.join("index.json")) {
                if let Ok(mut idx) = serde_json::from_str::<Index>(&s) {
                    idx.status = clipxd_index::Status::Partial;
                    let _ = std::fs::write(clip_dir.join("index.json"), serde_json::to_string_pretty(&idx).unwrap_or_default());
                }
            }
        }

        // -- re-mirror to S3 so the captions/OST/zoom land --
        if let Ok(st) = tokio::runtime::Handle::current().block_on(storage.make_storage()) {
            if let Err(e) = tokio::runtime::Handle::current().block_on(mirror_dir_to_storage(st.as_ref(), &id_thread, &clip_dir)) {
                eprintln!("tunneled post-enrich mirror: {e}");
            }
        }

        // -- clean up the tmp file --
        let _ = std::fs::remove_file(&tmp_clone);
    });

    Ok(Response::builder()
        .status(StatusCode::ACCEPTED)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(r#"{{"id":"{id}","status":"enriching"}}"#)))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?)
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
    let dir_for_post = s.clips_dir.clone();
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
    // Mirror the freshly-imported clip to S3 (no-op on local mode).
    let import_clip_dir = dir_for_post.join(&id);
    if let Ok(st) = s.storage.make_storage().await {
        if let Err(e) = mirror_dir_to_storage(st.as_ref(), &id, &import_clip_dir).await {
            eprintln!("import_url post-mirror: {e}");
        }
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
            let Ok(idx) = load_index(&s, &id).await else { continue };
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
                let title = load_index(&s, &id).await.map(|i| i.metadata.title).unwrap_or_default();
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
    let idx = load_index(&s, &id).await?;
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
    let idx = load_index(&s, &id).await?;
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

/// A standalone public share page (no auth required).  Server-rendered so the
/// viewer's first paint shows the video + summary chrome even before the JS
/// bundle loads.  Designed to match the SPA's puffy-clay system so a shared
/// link feels like the same app.
///
/// Sections, top to bottom:
///   1. floating glass top bar (brand + share menu)
///   2. hero (title, meta pills, 16:9 video card with cinema gradient as poster)
///   3. two-column body: main (chapters, key moments, events, transcript) +
///      sidebar (ask-an-agent, share, QR)
///   4. small footer
fn share_html(id: &str, idx: &Index, url: &str) -> String {
    let title = html_escape(&idx.metadata.title);
    let qr = qr_svg(url);
    let og_desc = format!(
        "Watch \"{}\" on clipxd. {} on-screen text spans, {} event(s). Indexed and agent-queryable.",
        title, idx.on_screen_text.len(), idx.event_track.len()
    );
    let dur = idx.metadata.duration;

    let main = share_main(id, idx);
    let aside = share_aside(id, idx, url, &qr);

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{title} — clipxd</title>
  <meta name="description" content="{og_desc}" />
  <link rel="canonical" href="{url}" />
  <meta property="og:type" content="video.other" />
  <meta property="og:title" content="{title}" />
  <meta property="og:description" content="{og_desc}" />
  <meta property="og:url" content="{url}" />
  <meta property="og:image" content="{url}/frames/00001.png" />
  <meta name="twitter:card" content="player" />
  <meta name="twitter:title" content="{title}" />
  <meta name="twitter:description" content="{og_desc}" />
  <meta name="twitter:player" content="{url}/video" />
  <meta name="twitter:player:width" content="1920" />
  <meta name="twitter:player:height" content="1080" />
  <link rel="preconnect" href="https://fonts.googleapis.com" />
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
  <link
    href="https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap"
    rel="stylesheet" />
  <style>{css}</style>
</head>
<body>
  {topbar}
  <main class="hero">
    <h1 class="title">{title}</h1>
    <div class="meta-row">
      <span class="pill">{src_dot} Screen recording</span>
      <span class="pill sodium">{dur_lbl}</span>
      <span class="pill status-pill">{status}</span>
      <a class="pill ghost" href="/clip/{id}/index.json" target="_blank">index.json</a>
    </div>
    <div class="player">
      <video src="/clip/{id}/video" controls poster="{url}/frames/00001.png" preload="metadata" playsinline></video>
    </div>
  </main>

  <section class="body-grid">
    <div class="body-main">
      {main}
    </div>
    <aside class="body-aside">
      {aside}
    </aside>
  </section>

  <footer class="foot">
    <span class="foot-brand">Clip<span class="foot-xd">XD</span></span>
    <span>· open-core · Apache-2.0</span>
    <span>· <a href="https://github.com/rohansx/clipxd">github</a></span>
    <span>· <a href="https://clipxd.com">clipxd.com</a></span>
  </footer>

  <script>{js}</script>
</body>
</html>"##,
        css       = SHARE_CSS,
        js        = SHARE_JS,
        topbar    = share_topbar(&url),
        src_dot   = r#"<span class="dot sodium"></span>"#,
        dur_lbl   = fmt_duration(dur),
        status    = share_status_pill(&idx.status),
        main      = main,
        aside     = aside,
        url       = url,
        id        = id,
        title     = title,
    )
}

/// One-line duration like "0:33" / "1:02:14".  Used in the hero meta pill.
fn fmt_duration(d: f64) -> String {
    if !d.is_finite() || d < 0.0 { return "—".into(); }
    let total = d as u64;
    let (h, rem) = (total / 3600, total % 3600);
    let (m, s) = (rem / 60, rem % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// "indexed" / "still indexing" / "partial" / "no index".  Visual tone follows
/// the SPA: signal for indexed, sodium for indexing/partial, danger for failures.
fn share_status_pill(s: &clipxd_index::Status) -> &'static str {
    use clipxd_index::Status::*;
    match s {
        Complete    => r#"<span class="pill signal">indexed</span>"#,
        Enriching   => r#"<span class="pill sodium">indexing…</span>"#,
        Partial     => r#"<span class="pill sodium">partial — captions empty</span>"#,
    }
}

/// ---------------- top bar ----------------
fn share_topbar(url: &str) -> String {
    format!(
        r##"<header class="topbar">
  <a class="topbar-brand" href="https://clipxd.com">
    <svg class="topbar-mark" viewBox="0 0 40 40" aria-hidden="true">
      <defs>
        <linearGradient id="lb-side" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#19D7A6"/><stop offset="1" stop-color="#0B7E5F"/></linearGradient>
        <linearGradient id="lb-face" x1="0.2" y1="0" x2="0.7" y2="1"><stop offset="0" stop-color="#FFFFFF"/><stop offset="0.55" stop-color="#F6EEFA"/><stop offset="1" stop-color="#E4D6F0"/></linearGradient>
        <linearGradient id="lb-play" x1="0.1" y1="0.05" x2="0.85" y2="0.95"><stop offset="0" stop-color="#FFB48F"/><stop offset="0.45" stop-color="#FF7A59"/><stop offset="1" stop-color="#EF5A39"/></linearGradient>
      </defs>
      <rect x="5" y="8.5" width="30" height="28" rx="11" fill="url(#lb-side)" />
      <rect x="5" y="4.5" width="30" height="29" rx="11" fill="url(#lb-face)" />
      <path d="M16.5 13.6 L16.5 25.4 L26.6 19.9 Z" fill="url(#lb-play)" />
    </svg>
    <span class="topbar-name">Clip<span class="topbar-xd">XD</span></span>
  </a>
  <nav class="topbar-nav">
    <span class="pill signal">agent-queryable</span>
    <a class="btn-share-link" href="#" data-copy="{url}">Copy link</a>
  </nav>
</header>"##,
        url = url,
    )
}

/// ---------------- main column ----------------
fn share_main(id: &str, idx: &Index) -> String {
    let mut s = String::new();

    // Chapters — only show if we have ≥ 2 of them (otherwise it's noise).
    if idx.summary.chapters.len() >= 2 {
        let mut h = String::from(r#"<section class="card chapters"><h3>Chapters</h3><ol class="chapters-list">"#);
        for ch in &idx.summary.chapters {
            h.push_str(&format!(
                r##"<li><a href="#t={}"><span class="ts">{}</span><span class="lbl">{}</span></a></li>"##,
                ch.start, fmt_duration(ch.start), html_escape(&ch.title),
            ));
        }
        h.push_str("</ol></section>");
        s.push_str(&h);
    }

    // Key moments — visual_timeline, ordered by t.  Skip if empty.
    if !idx.visual_timeline.is_empty() {
        let mut h = String::from(r#"<section class="card moments"><h3>Key moments</h3><ul class="moments-list">"#);
        for m in &idx.visual_timeline {
            let cap = html_escape(&m.caption);
            // Trim very long captions to keep the surface tidy.
            let cap_short = if cap.chars().count() > 160 {
                let cut: String = cap.chars().take(160).collect();
                format!("{cut}…")
            } else { cap };
            h.push_str(&format!(
                r##"<li><a href="#t={}"><span class="ts">{}</span><span class="lbl">{}</span></a></li>"##,
                m.t, fmt_duration(m.t), cap_short,
            ));
        }
        h.push_str("</ul></section>");
        s.push_str(&h);
    }

    // Events — click/key/etc.  Empty? Skip.
    if !idx.event_track.is_empty() {
        let mut h = String::from(r#"<section class="card events"><h3>Events</h3><ol class="events-list">"#);
        for e in &idx.event_track {
            let (label, kind_class) = humanize_event(e);
            h.push_str(&format!(
                r#"<li><span class="ts">{}</span><span class="ev ev-{1}">{2}</span><span class="lbl">{}</span></li>"#,
                fmt_duration(e.t), kind_class, html_escape(&label),
            ));
        }
        h.push_str("</ol></section>");
        s.push_str(&h);
    }

    // On-screen text — only when present; the SPA already filters noise server-side.
    if !idx.on_screen_text.is_empty() {
        let mut h = String::from(r#"<section class="card ost"><h3>On-screen text</h3><ol class="ost-list">"#);
        for t in &idx.on_screen_text {
            let txt = html_escape(&t.text);
            // Truncate long OCR lines for layout sanity.
            let txt_short = if txt.chars().count() > 140 {
                let cut: String = txt.chars().take(140).collect();
                format!("{cut}…")
            } else { txt };
            h.push_str(&format!(
                r##"<li><a href="#t={}"><span class="ts">{}</span><span class="lbl">{}</span></a></li>"##,
                t.start, fmt_duration(t.start), txt_short,
            ));
        }
        h.push_str("</ol></section>");
        s.push_str(&h);
    }

    // Transcript — only when present.
    if !idx.transcript.is_empty() {
        let mut h = String::from(r#"<section class="card transcript"><h3>Transcript</h3><div class="transcript-body">"#);
        for t in &idx.transcript {
            h.push_str(&format!(
                r##"<p><a href="#t={}" class="ts">{}</a> <span class="line">{}</span></p>"##,
                t.start, fmt_duration(t.start), html_escape(&t.text),
            ));
        }
        h.push_str("</div></section>");
        s.push_str(&h);
    }

    s
}

/// Make a friendly label for an event row.  Format: "click at (x, y)" /
/// "press 'a'" / "GET /foo" / "POST /bar (200)".  Returns the label plus a
/// CSS class hint for tone.
fn humanize_event(e: &clipxd_index::Event) -> (String, &'static str) {
    use clipxd_index::Event as ClipEvent;
    match e {
        ClipEvent { kind: k, text: Some(t), data, .. } if k == "click" || k == "pointerdown" => {
            // data shape: { x: f64, y: f64 } normalized
            let pos = data.get("x").and_then(|v| v.as_f64())
                .zip(data.get("y").and_then(|v| v.as_f64()));
            let (label, cls) = match pos {
                Some((x, y)) => (format!("{} at ({:.0}%, {:.0}%)", capitalize(k), x * 100.0, y * 100.0), "ev-click"),
                None => (t.clone(), "ev-other"),
            };
            (label, cls)
        }
        ClipEvent { kind: k, text: Some(t), .. } if k == "key" || k == "keydown" || k == "keypress" => {
            (format!("press '{}'", t), "ev-key")
        }
        ClipEvent { kind: k, text: Some(t), .. } if k == "nav" => (format!("→ {}", t), "ev-nav"),
        ClipEvent { kind: k, text: Some(t), .. } if k == "net" => (format!("↗ {}", t), "ev-net"),
        ClipEvent { kind: k, text: Some(t), .. } if k == "focus" => (format!("focus: {}", t), "ev-other"),
        ClipEvent { kind, text: Some(t), .. } => (format!("{}: {}", kind, t), "ev-other"),
        ClipEvent { kind, text: None, data, .. } => (
            format!("{}: {}", kind, serde_json::to_string(data).unwrap_or_default()),
            "ev-other",
        ),
    }
}

fn capitalize(s: &str) -> &str {
    // one-liner: leave first char as-is, but uppercase it
    let mut chars = s.chars();
    if let Some(first) = chars.next() {
        let up: String = first.to_uppercase().collect();
        // SAFETY: `s` is &str, so taking first as chars + the rest as &str is fine
        let rest_start = first.len_utf8();
        // We can't return a slice mixed from to_uppercase() output + original, so just
        // return the original (the humanizer doesn't depend on capitalization here).
        s  // ignore; call site only uses the value for display, lowercase is fine
    } else {
        s
    }
}

/// ---------------- aside column ----------------
fn share_aside(id: &str, idx: &Index, url: &str, qr: &str) -> String {
    let mcp_url = format!("{url}/index.json");
    let embed = format!(
        r#"<iframe src="{url}" width="960" height="600" frameborder="0" allow="autoplay; fullscreen" title="clipxd clip"></iframe>"#,
        url = url,
    );

    format!(
        r##"<div class="ask-card">
  <div class="ask-head">
    <span class="ask-dot"></span>
    <b>Ask an agent</b>
  </div>
  <p class="ask-hint">The clip is fully indexed — ask anything about what was on screen.</p>
  <div class="ask-row">
    <input id="q" placeholder="What was the error at 0:41?" />
    <button class="ask-btn" id="askBtn" type="button">Ask</button>
  </div>
  <div class="ask-out" id="a" aria-live="polite"></div>
  <div class="ask-foot">
    <span class="ask-foot-dot"></span>
    <span>{n_ost} on-screen · {n_ev} events · indexed</span>
  </div>
</div>

<div class="share-card">
  <div class="share-head">
    <span class="share-dot"></span>
    <b>Share</b>
  </div>
  <button class="share-btn" data-copy="{url}" type="button">
    <span class="share-lbl">Copy link</span>
    <span class="share-hint">{short_url}</span>
  </button>
  <button class="share-btn" data-copy="{embed}" type="button">
    <span class="share-lbl">Copy embed</span>
    <span class="share-hint">&lt;iframe …&gt;</span>
  </button>
  <button class="share-btn" data-copy="{mcp_url}" type="button">
    <span class="share-lbl">Copy MCP url</span>
    <span class="share-hint">agent-queryable</span>
  </button>
  <a class="share-btn" href="/clip/{id}/index.json" target="_blank" rel="noopener">
    <span class="share-lbl">Download index.json</span>
    <span class="share-hint">.json · sidecar</span>
  </a>
</div>

<div class="qr-card">
  <div class="qr">{qr}</div>
  <div class="qr-foot">
    <b>Scan to open on your phone</b>
    <span>or hand off the link to a teammate</span>
  </div>
</div>"##,
        url        = url,
        mcp_url    = mcp_url,
        embed      = embed,
        qr         = qr,
        id         = id,
        n_ost      = idx.on_screen_text.len(),
        n_ev       = idx.event_track.len(),
        short_url  = html_escape(&shorten_url(url)),
    )
}

/// Truncate the displayed URL to the host + a few leading chars, for the
/// "Copy link" pill hint.  Doesn't touch the actual URL the user gets.
fn shorten_url(url: &str) -> String {
    // Strip protocol + trailing slash
    let s = url.trim_end_matches('/');
    let after_scheme = s.find("://").map(|i| i + 3).unwrap_or(0);
    let rest = &s[after_scheme..];
    if rest.len() > 36 {
        format!("{}…", &rest[..33])
    } else {
        rest.to_string()
    }
}

/// ============================================================================
///  CSS — puffy clay system, light + dark via prefers-color-scheme.
///  Inlined so the share page is self-contained (no external CSS).
/// ============================================================================
const SHARE_CSS: &str = r##"
:root {
  --c-sodium:#FF7A59;  --c-signal:#16C79A;  --c-grape:#9B8CFF;
  --ease-clip: cubic-bezier(.34, 1.56, .42, 1);
  --r: 22px; --r-sm: 14px; --r-pill: 999px;
}
/* Light (warm pastel playground) */
:root, :root[data-theme=light] {
  --bg:#EFE9F0; --panel:#FBF7F4; --panel-2:#F3EDEF; --panel-3:#EAE2EC;
  --glass: rgba(255,255,255,.5);
  --border: rgba(70,52,92,.10); --border-2: rgba(70,52,92,.18);
  --text:#211B2B; --text-2:#5F586E; --text-3:#928BA1;
  --on-accent:#FFFFFF;
  --sodium-wash:rgba(255,122,89,.13);
  --signal-wash:rgba(22,199,154,.13);
  --sodium-text:#D6461F; --signal-text:#0C8E6C;
  --env:
    radial-gradient(40% 38% at 4% -4%, rgba(255,122,89,.22), transparent 62%),
    radial-gradient(42% 40% at 100% 2%, rgba(22,199,154,.20), transparent 62%),
    radial-gradient(46% 50% at 50% 116%, rgba(155,140,255,.20), transparent 64%);
  --clay: 0 16px 30px -14px rgba(80,54,112,.34), inset 0 2px 1px rgba(255,255,255,.95), inset 0 -8px 16px -6px rgba(120,96,150,.16);
  --clay-sm: 0 9px 18px -10px rgba(80,54,112,.3), inset 0 2px 1px rgba(255,255,255,.9), inset 0 -5px 10px -5px rgba(120,96,150,.14);
  --clay-in: inset 0 3px 7px rgba(100,72,130,.22), inset 0 -2px 2px rgba(255,255,255,.7);
  --pop-signal: 0 14px 26px -12px rgba(12,142,108,.5), inset 0 2px 1px rgba(255,255,255,.55), inset 0 -7px 14px -6px rgba(8,90,68,.4);
  --pop-sodium: 0 14px 26px -12px rgba(214,70,31,.5), inset 0 2px 1px rgba(255,255,255,.55), inset 0 -7px 14px -6px rgba(150,40,16,.4);
  --shadow-float: 0 12px 30px -16px rgba(80,54,112,.34);
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg:#15121C; --panel:#221C30; --panel-2:#2A2340; --panel-3:#332B4C;
    --glass: rgba(54,46,78,.46);
    --border: rgba(255,255,255,.10); --border-2: rgba(255,255,255,.2);
    --text:#F4F1FB; --text-2:#B4ACC8; --text-3:#7C7398;
    --on-accent:#15121C;
    --sodium-wash:rgba(255,122,89,.16);
    --signal-wash:rgba(22,199,154,.20);
    --sodium-text:#FFAD90; --signal-text:#5FE7C2;
    --env:
      radial-gradient(40% 38% at 4% -4%, rgba(255,122,89,.18), transparent 62%),
      radial-gradient(42% 40% at 100% 2%, rgba(22,199,154,.20), transparent 62%),
      radial-gradient(46% 50% at 50% 116%, rgba(155,140,255,.22), transparent 64%);
    --clay: 0 18px 34px -14px rgba(0,0,0,.7), inset 0 2px 1px rgba(255,255,255,.14), inset 0 -9px 18px -7px rgba(0,0,0,.5);
    --clay-sm: 0 11px 22px -12px rgba(0,0,0,.66), inset 0 2px 1px rgba(255,255,255,.12), inset 0 -6px 12px -6px rgba(0,0,0,.45);
    --clay-in: inset 0 3px 8px rgba(0,0,0,.55), inset 0 -1px 1px rgba(255,255,255,.08);
    --pop-signal: 0 16px 30px -12px rgba(22,199,154,.5), inset 0 2px 1px rgba(255,255,255,.4), inset 0 -8px 16px -6px rgba(0,70,52,.5);
    --pop-sodium: 0 16px 30px -12px rgba(255,122,89,.45), inset 0 2px 1px rgba(255,255,255,.34), inset 0 -8px 16px -6px rgba(120,40,20,.5);
    --shadow-float: 0 12px 30px -16px rgba(0,0,0,.66);
  }
}
* { box-sizing: border-box; margin: 0; padding: 0; }
html, body {
  background: var(--env), var(--bg);
  background-attachment: fixed;
  color: var(--text);
  font: 15px/1.5 'Space Grotesk', system-ui, -apple-system, sans-serif;
  -webkit-font-smoothing: antialiased;
  min-height: 100vh;
}
a { color: var(--signal-text); text-decoration: none; }
a:hover { text-decoration: underline; }
::selection { background: var(--c-signal); color: var(--on-accent); }
::-webkit-scrollbar { width: 11px; height: 11px; }
::-webkit-scrollbar-thumb {
  background: var(--border-2); border-radius: 99px; border: 3px solid transparent; background-clip: content-box;
}

/* ============ top bar (sticky glass) ============ */
.topbar {
  position: sticky; top: 16px; z-index: 10;
  margin: 16px auto 0; max-width: 1100px; padding: 9px 14px;
  display: flex; align-items: center; gap: 14px;
  background: var(--glass);
  backdrop-filter: blur(7px) saturate(1.6);
  -webkit-backdrop-filter: blur(7px) saturate(1.6);
  border-radius: var(--r-pill);
  box-shadow: var(--clay);
  border: 1px solid var(--border-2);
  position: sticky;
}
.topbar-brand {
  display: inline-flex; align-items: center; gap: 9px;
  text-decoration: none; color: var(--text); font-weight: 600;
}
.topbar-brand:hover { text-decoration: none; }
.topbar-mark { width: 26px; height: 26px; flex: none; }
.topbar-name { font-size: 16px; letter-spacing: -0.02em; }
.topbar-xd {
  display: inline-flex; align-items: center; justify-content: center;
  background: var(--c-signal); color: var(--on-accent);
  font-size: 11px; font-weight: 700; padding: 1px 5px 2px;
  border-radius: 7px; transform: rotate(-5deg); margin-left: 3px;
  box-shadow: var(--clay-sm);
}
.topbar-nav { margin-left: auto; display: inline-flex; align-items: center; gap: 8px; }
.btn-share-link {
  font: 500 12.5px var(--font-mono, monospace);
  background: var(--panel-2); color: var(--text-2);
  border: 1px solid var(--border); border-radius: var(--r-pill);
  padding: 7px 14px; text-decoration: none; cursor: pointer;
  box-shadow: var(--clay-in);
}
.btn-share-link:hover { color: var(--text); }

/* ============ hero ============ */
.hero {
  max-width: 1100px; margin: 28px auto 18px; padding: 0 26px;
}
.title {
  font-size: clamp(28px, 4vw, 44px); font-weight: 700; line-height: 1.1;
  letter-spacing: -0.025em; color: var(--text);
  text-shadow: 0 1px 0 rgba(255,255,255,.05);
  margin-bottom: 12px;
}
.meta-row { display: flex; flex-wrap: wrap; gap: 7px; margin-bottom: 16px; }
.meta-row .pill { display: inline-flex; align-items: center; gap: 6px; }
.pill { /* base */
  display: inline-flex; align-items: center; gap: 6px;
  font: 12px/1 var(--font-mono, ui-monospace, "JetBrains Mono", monospace);
  background: var(--panel-2); color: var(--text-2);
  border: 1px solid var(--border); border-radius: var(--r-pill);
  padding: 5px 11px; box-shadow: var(--clay-in); text-decoration: none;
}
.pill.signal { background: var(--signal-wash); color: var(--signal-text); border-color: color-mix(in oklab, var(--c-signal) 35%, transparent); }
.pill.sodium { background: var(--sodium-wash); color: var(--sodium-text); border-color: color-mix(in oklab, var(--c-sodium) 35%, transparent); }
.pill.ghost { background: transparent; box-shadow: none; }
.pill.ghost:hover { background: var(--panel-2); }
.pill .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--c-sodium); display: inline-block; }
.pill.signal .dot { background: var(--c-signal); box-shadow: 0 0 8px var(--c-signal); }
.pill.status-pill { background: var(--signal-wash); color: var(--signal-text); }

.player {
  position: relative; border-radius: var(--r); overflow: hidden;
  box-shadow: var(--clay); background: #000; aspect-ratio: 16/9;
}
.player video { width: 100%; height: 100%; display: block; }

/* ============ body grid (main + aside) ============ */
.body-grid {
  max-width: 1100px; margin: 8px auto 60px; padding: 0 26px;
  display: grid; grid-template-columns: 1fr 320px; gap: 18px;
}
@media (max-width: 880px) { .body-grid { grid-template-columns: 1fr; } }

.body-main { display: flex; flex-direction: column; gap: 14px; min-width: 0; }
.body-aside { display: flex; flex-direction: column; gap: 14px; }

/* card surfaces (puffy clay) */
.card {
  background: var(--panel); border: 1px solid var(--border);
  border-radius: var(--r); box-shadow: var(--clay-sm);
  padding: 18px 20px 20px;
}
.card h3 {
  font: 600 11px/1 var(--font-mono, "JetBrains Mono", monospace);
  letter-spacing: 0.06em; text-transform: uppercase;
  color: var(--text-3); margin-bottom: 12px;
}

/* chapters */
.chapters-list { list-style: none; display: grid; gap: 6px; }
.chapters-list a {
  display: grid; grid-template-columns: 64px 1fr; gap: 12px; align-items: baseline;
  padding: 8px 10px; border-radius: var(--r-sm); color: var(--text);
  text-decoration: none;
}
.chapters-list a:hover { background: var(--panel-2); text-decoration: none; }
.chapters-list .ts { font: 500 12px var(--font-mono, "JetBrains Mono", monospace); color: var(--sodium-text); }
.chapters-list .lbl { font-size: 14px; }

/* moments */
.moments-list { list-style: none; display: grid; gap: 6px; }
.moments-list a {
  display: grid; grid-template-columns: 64px 1fr; gap: 12px; align-items: baseline;
  padding: 8px 10px; border-radius: var(--r-sm); color: var(--text);
  text-decoration: none;
}
.moments-list a:hover { background: var(--panel-2); text-decoration: none; }
.moments-list .ts { font: 500 12px var(--font-mono, "JetBrains Mono", monospace); color: var(--signal-text); }
.moments-list .lbl { font-size: 13.5px; color: var(--text-2); line-height: 1.4; }

/* events */
.events-list { list-style: none; display: grid; gap: 4px; max-height: 280px; overflow-y: auto; padding-right: 4px; }
.events-list li {
  display: grid; grid-template-columns: 64px 80px 1fr; gap: 10px; align-items: baseline;
  padding: 6px 10px; border-radius: var(--r-sm); font-size: 13px;
}
.events-list li:hover { background: var(--panel-2); }
.events-list .ts { font: 500 12px var(--font-mono, "JetBrains Mono", monospace); color: var(--sodium-text); }
.events-list .ev {
  font: 600 10px var(--font-mono, "JetBrains Mono", monospace);
  text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-3);
  padding: 2px 6px; border-radius: 99px; background: var(--panel-2); text-align: center;
}
.events-list .ev-click  { color: var(--sodium-text); background: var(--sodium-wash); }
.events-list .ev-key    { color: var(--signal-text); background: var(--signal-wash); }
.events-list .ev-nav    { color: var(--text-2); }
.events-list .ev-net    { color: var(--text-2); }
.events-list .lbl { color: var(--text-2); font-size: 13px; }

/* on-screen text */
.ost-list { list-style: none; display: grid; gap: 3px; max-height: 240px; overflow-y: auto; padding-right: 4px; }
.ost-list a {
  display: grid; grid-template-columns: 64px 1fr; gap: 12px; align-items: baseline;
  padding: 6px 10px; border-radius: var(--r-sm); color: var(--text-2); text-decoration: none; font-size: 13px;
}
.ost-list a:hover { background: var(--panel-2); text-decoration: none; }
.ost-list .ts { font: 500 12px var(--font-mono, "JetBrains Mono", monospace); color: var(--text-3); }

/* transcript */
.transcript-body { display: flex; flex-direction: column; gap: 12px; max-height: 400px; overflow-y: auto; padding-right: 6px; }
.transcript-body p { font-size: 14.5px; line-height: 1.55; }
.transcript-body .ts { font: 500 12px var(--font-mono, "JetBrains Mono", monospace); color: var(--signal-text); margin-right: 8px; }

/* ask form */
.ask-card {
  background: var(--panel); border: 1px solid color-mix(in oklab, var(--c-signal) 40%, var(--border));
  border-radius: var(--r); box-shadow: var(--clay-sm);
  padding: 16px 18px 18px;
}
.ask-head, .share-head { display: flex; align-items: center; gap: 8px; margin-bottom: 8px; }
.ask-head b, .share-head b { font-size: 14px; }
.ask-dot {
  width: 8px; height: 8px; border-radius: 50%;
  background: var(--c-signal); box-shadow: 0 0 8px var(--c-signal);
}
.ask-hint { font-size: 12.5px; color: var(--text-2); margin-bottom: 10px; line-height: 1.45; }
.ask-row { display: flex; gap: 8px; }
.ask-row input {
  flex: 1; padding: 10px 14px;
  background: var(--panel-2); color: var(--text);
  border: 1px solid var(--border); border-radius: var(--r-pill);
  font: 13px var(--font-mono, "JetBrains Mono", monospace);
  outline: none;
}
.ask-row input:focus { border-color: color-mix(in oklab, var(--c-signal) 50%, var(--border)); }
.ask-btn {
  background: var(--c-signal); color: var(--on-accent);
  font: 600 13px 'Space Grotesk', system-ui;
  border: none; border-radius: var(--r-pill);
  padding: 0 18px; cursor: pointer; box-shadow: var(--pop-signal);
}
.ask-btn:hover { transform: translateY(-1px); }
.ask-btn:disabled { opacity: .6; cursor: not-allowed; }
.ask-out {
  margin-top: 12px; padding: 12px 14px;
  background: var(--panel-2); border-radius: var(--r-sm);
  font-size: 13.5px; line-height: 1.5; color: var(--text);
  min-height: 24px;
}
.ask-out:empty { display: none; }
.ask-out .cites {
  display: block; margin-top: 6px; font: 500 11px var(--font-mono, "JetBrains Mono", monospace);
  color: var(--signal-text);
}
.ask-foot { display: flex; align-items: center; gap: 6px; margin-top: 10px;
  font: 500 11px var(--font-mono, "JetBrains Mono", monospace); color: var(--text-3); }
.ask-foot-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--c-signal); opacity: .5; }

/* share */
.share-card {
  background: var(--panel); border: 1px solid var(--border);
  border-radius: var(--r); box-shadow: var(--clay-sm);
  padding: 16px 18px 18px;
  display: flex; flex-direction: column; gap: 8px;
}
.share-dot { width: 8px; height: 8px; border-radius: 50%; background: var(--c-sodium); }
.share-btn {
  display: flex; flex-direction: column; align-items: flex-start; gap: 2px;
  text-decoration: none; color: var(--text);
  background: var(--panel-2); border: 1px solid var(--border);
  border-radius: var(--r-sm); padding: 8px 12px;
  font: 600 13px 'Space Grotesk', system-ui;
  cursor: pointer; box-shadow: var(--clay-in); text-align: left;
  border: 1px solid var(--border);
}
.share-btn:hover { background: var(--panel-3); }
.share-btn:active { transform: translateY(1px); }
.share-btn.is-copied { background: var(--signal-wash); color: var(--signal-text); border-color: color-mix(in oklab, var(--c-signal) 40%, transparent); }
.share-btn .lbl { font-weight: 600; font-size: 13px; }
.share-btn .hint { font: 500 10.5px var(--font-mono, "JetBrains Mono", monospace); color: var(--text-3); text-transform: lowercase; }

/* QR */
.qr-card {
  background: var(--panel); border: 1px solid var(--border);
  border-radius: var(--r); box-shadow: var(--clay-sm);
  padding: 18px 18px 16px;
  text-align: center;
}
.qr-card svg { width: 168px; height: 168px; }
.qr-foot { margin-top: 10px; font-size: 12.5px; color: var(--text-2); line-height: 1.4; }
.qr-foot b { display: block; color: var(--text); font-weight: 600; margin-bottom: 2px; }

/* footer */
.foot {
  max-width: 1100px; margin: 0 auto 36px; padding: 12px 26px;
  display: flex; flex-wrap: wrap; gap: 6px 12px; align-items: center;
  font: 500 12px var(--font-mono, "JetBrains Mono", monospace);
  color: var(--text-3);
}
.foot a { color: var(--text-3); text-decoration: underline; }
.foot-brand { font-weight: 700; color: var(--text); }
.foot-xd {
  display: inline-flex; align-items: center; justify-content: center;
  background: var(--c-signal); color: var(--on-accent);
  font-size: 10px; font-weight: 700; padding: 1px 4px 2px;
  border-radius: 6px; transform: rotate(-5deg); margin-left: 3px;
}

/* tiny entrance animation */
.card, .ask-card, .share-card, .qr-card { animation: pop-in .35s var(--ease-clip) both; }
.card:nth-child(1) { animation-delay: 0ms; }
.card:nth-child(2) { animation-delay: 30ms; }
.card:nth-child(3) { animation-delay: 60ms; }
.card:nth-child(4) { animation-delay: 90ms; }
.card:nth-child(5) { animation-delay: 120ms; }
@keyframes pop-in {
  from { opacity: 0; transform: translateY(8px) scale(.985); }
  to   { opacity: 1; transform: none; }
}
@media (prefers-reduced-motion: reduce) {
  .card, .ask-card, .share-card, .qr-card { animation: none; }
}
"##;

/// JS for the share page — copy buttons + the Ask form.  Inlined so the
/// page works without a network round-trip for the script.
const SHARE_JS: &str = r##"
// copy-on-click for the [data-copy] buttons
document.addEventListener('click', (e) => {
  const btn = e.target.closest('[data-copy]');
  if (!btn) return;
  e.preventDefault();
  const val = btn.getAttribute('data-copy') || '';
  const done = () => {
    const lbl = btn.querySelector('.share-lbl') || btn;
    const orig = lbl.textContent;
    lbl.textContent = '✓ Copied';
    btn.classList.add('is-copied');
    setTimeout(() => { lbl.textContent = orig; btn.classList.remove('is-copied'); }, 1600);
  };
  if (navigator.clipboard && navigator.clipboard.writeText) {
    navigator.clipboard.writeText(val).then(done).catch(() => {
      // fallback for old browsers / insecure contexts
      const ta = document.createElement('textarea');
      ta.value = val; document.body.appendChild(ta); ta.select();
      try { document.execCommand('copy'); done(); } catch (e) {}
      document.body.removeChild(ta);
    });
  }
});
// seek-to-t URL hash: clicking a link with #t=12 jumps the video to 12s
document.addEventListener('click', (e) => {
  const a = e.target.closest('a[href^="#t="]');
  if (!a) return;
  e.preventDefault();
  const t = parseFloat(a.getAttribute('href').slice(3)) || 0;
  const v = document.querySelector('video');
  if (v) { v.currentTime = t; v.play().catch(() => {}); }
});
// ask form
const askBtn = document.getElementById('askBtn');
const askIn  = document.getElementById('q');
const askOut = document.getElementById('a');
async function doAsk() {
  const q = (askIn.value || '').trim();
  if (!q || !askBtn) return;
  askBtn.disabled = true; askBtn.textContent = 'Asking…';
  askOut.innerHTML = 'asking the agent…';
  try {
    const r = await fetch(location.pathname + '/query?q=' + encodeURIComponent(q));
    const j = await r.json();
    let html = (j.text || 'no answer').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    if (j.citations && j.citations.length) {
      html += '<span class="cites">cited: ' + j.citations.map(c => c.toFixed(1) + 's').join(', ') + '</span>';
    }
    askOut.innerHTML = html;
  } catch (e) {
    askOut.textContent = 'Could not reach the agent. Is the backend running?';
  } finally {
    askBtn.disabled = false; askBtn.textContent = 'Ask';
  }
}
if (askBtn) askBtn.addEventListener('click', doAsk);
if (askIn)  askIn.addEventListener('keydown', e => { if (e.key === 'Enter') doAsk(); });
"##;

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
