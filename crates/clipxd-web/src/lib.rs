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
use tower_http::cors::{self, CorsLayer};

/// One streaming-upload session's incremental indexer, keyed by its session id (== the clip
/// id for instant-link sessions). The outer map is locked only briefly (get/insert/remove);
/// the inner `StdMutex` guards the actual `add_increment`/`finalize` calls, which run on
/// blocking threads (ffmpeg + OCR work). `Option` so `finalize` can `.take()` an owned
/// `IncrementalIndexer` out of the slot.
type StageSessions = Arc<AsyncMutex<HashMap<String, Arc<StdMutex<Option<IncrementalIndexer>>>>>>;

/// Clip ids currently being finalized (Phase 1 + Phase 2). Three actors can try to write the
/// same `clip_dir` — a staged commit, the `/ingest?reuse=` fallback, and the abandoned-session
/// sweeper — and none of them may overlap: concurrent `promote_staged`/`enrich` runs on one
/// dir interleave `video.webm`/`index.json`/`frames/` writes with no ordering. A finalizer
/// must hold the id's [`ClaimGuard`] for the whole job; whoever loses the claim backs off
/// (HTTP 409 for requests, skip-this-tick for the sweeper).
type ClipClaims = Arc<StdMutex<std::collections::HashSet<String>>>;

/// RAII release for a [`ClipClaims`] entry — dropped (and thus released) wherever the
/// finalization ends, including every early-error path.
struct ClaimGuard {
    claims: ClipClaims,
    id: String,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = self.claims.lock() {
            g.remove(&self.id);
        }
    }
}

/// Claim `id` for exclusive finalization; `None` when another finalizer holds it.
fn try_claim(claims: &ClipClaims, id: &str) -> Option<ClaimGuard> {
    let mut g = claims.lock().ok()?;
    g.insert(id.to_string()).then(|| ClaimGuard { claims: claims.clone(), id: id.to_string() })
}

/// Mint a fresh clip id: 64 bits of wall-clock nanoseconds folded with 32 bits of randomness,
/// rendered as 24 hex chars. A bare 32-bit truncated timestamp (the old scheme) collides with
/// 50% probability after ~65K clips (birthday bound) — on a busy box that's days, not never —
/// and a collision here means silently overwriting a stranger's `index.json`/video. This is
/// generated *before* any content exists (unlike the timestamp ids `clipxd-import`/the tunnel
/// path derive from bytes already in hand), so it has no content to disambiguate on collision;
/// the entropy has to carry the whole guarantee.
fn mint_clip_id() -> String {
    use rand::RngCore;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let salt = rand::thread_rng().next_u32();
    format!("clp_{stamp:016x}{salt:08x}")
}

pub mod auth;
pub mod deeppass;
pub mod docgen;
pub mod emphasis;
pub mod llm;
pub mod mcp;
pub mod preview_gif;
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
    /// Clip ids being finalized right now — see [`ClipClaims`].
    pub clip_claims: ClipClaims,
    /// Caps how many Phase-2 enrichments run at once. Every ingest detaches one; unbounded,
    /// a burst of recordings would stack ffmpeg+OCR jobs until the 4 GB box falls over.
    pub phase2_permits: Arc<tokio::sync::Semaphore>,
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
    let state = AppState {
        storage,
        clips_dir: Arc::new(clips_dir),
        public,
        public_base,
        auth,
        stage_sessions,
        clip_claims: Arc::new(StdMutex::new(std::collections::HashSet::new())),
        phase2_permits: Arc::new(tokio::sync::Semaphore::new(2)),
    };
    // read-only surface — always present, safe to expose publicly (unguessable share links)
    let mut router = Router::new()
        .route("/clip/:id", get(share_page))
        .route("/clip/:id/agent.md", get(get_agent_md))
        .route("/clip/:id/doc/:kind", get(get_doc))
        .route("/clip/:id/preview.gif", get(get_preview_gif))
        .route("/clip/:id/index.json", get(get_index))
        .route("/clip/:id/zoom.json", get(get_zoom))
        .route("/clip/:id/thumbnail", get(get_thumbnail))
        .route("/clip/:id/query", get(get_query))
        .route("/clip/:id/search", get(get_search))
        .route("/clip/:id/events", get(get_events))
        .route("/clip/:id/video", get(get_video))
        .route("/clip/:id/frames/:name", get(get_frame))
        .route("/clip/:id/view", post(post_view))
        .route("/clip/:id/comments", get(get_comments).post(post_comment))
        // Username-canonical share-link form: /u/:username/clip/:id and all the same
        // sub-resources. Resolved via ownership (404 if the clip isn't owned by that user).
        .route("/u/:username/clip/:id", get(share_page_for_user))
        // Branded share link: /u/:username/<title-slug>-<short>. The slug is purely
        // cosmetic — the real access control is the short tail of the clip id, looked
        // up against the user's clips (see `resolve_share_slug`). Same canonical
        // experience as /u/:username/clip/:id, just a URL humans actually want to read
        // out loud.
        .route("/u/:username/:slug", get(share_page_for_slug))
        .route("/u/:username/clip/:id/agent.md", get(get_agent_md_for_user))
        .route("/u/:username/clip/:id/doc/:kind", get(get_doc_for_user))
        .route("/u/:username/clip/:id/preview.gif", get(get_preview_gif_for_user))
        .route("/u/:username/clip/:id/index.json", get(get_index_for_user))
        .route("/u/:username/clip/:id/zoom.json", get(get_zoom_for_user))
        .route("/u/:username/clip/:id/thumbnail", get(get_thumbnail_for_user))
        .route("/u/:username/clip/:id/query", get(get_query_for_user))
        .route("/u/:username/clip/:id/search", get(get_search_for_user))
        .route("/u/:username/clip/:id/events", get(get_events_for_user))
        .route("/u/:username/clip/:id/video", get(get_video_for_user))
        .route("/u/:username/clip/:id/frames/:name", get(get_frame_for_user))
        .route("/u/:username/clip/:id/view", post(post_view_for_user))
        .route("/u/:username/clip/:id/comments", get(get_comments_for_user).post(post_comment_for_user))
        .route("/net", get(get_net))
        // Multi-tenant MCP: "add clipxd.com as an MCP server" — every tool takes an explicit
        // clip_id, same unguessable-id security model as the read-only routes above (no
        // per-clip auth check; the id itself is the access control, matching /clip/:id/query).
        .route_service("/mcp", mcp::mcp_service(state.clone()));
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
            .route("/clip/:id/local-captions", post(post_local_captions))
            .route("/clip/:id/subtitle-style", post(set_subtitle_style))
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
            .route("/auth/github/callback", get(github_callback))
            .route("/settings/keys", get(get_settings_keys).post(post_settings_keys));
    }
    // Tunneled ingest: only meaningful when `CLIPXD_YT_TUNNEL_URL` is set. The forwarder
    // (your home box) calls this with the video bytes it pulled via yt-dlp.
    // We expose it even when auth is off (so a single-user LAN setup can use the tunnel too);
    // auth is the `?token=<shared-secret>` query param matching CLIPXD_YT_TUNNEL_URL.
    router = router
        .route("/ingest/tunneled", post(ingest_tunneled))
        .route("/ingest/browser-trace", post(ingest_browser_trace));
    // Reap abandoned instant-link sessions (tab closed mid-recording → a `recording` stub +
    // stage dir that will never see a commit). Guarded: `app()` is also built in tests with
    // no runtime.
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let sweep_state = state.clone();
        handle.spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(600)).await;
                sweep_abandoned_stages(&sweep_state).await;
                sweep_orphaned_recording_stubs(&sweep_state).await;
            }
        });
    }
    router
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        // `CorsLayer::permissive()` sends `Access-Control-Allow-Origin: *`, which browsers
        // categorically reject on any request sent with `credentials: 'include'` (used
        // throughout the SPA so the session cookie flows to the API) — a wildcard ACAO can
        // never be paired with credentials, no matter what other headers say. Mirroring the
        // request's actual Origin instead of a wildcard is the standard fix, and is required
        // for the SPA's supported cross-origin dev mode (`?api=http://localhost:8787` pointing
        // a Vite dev server at a separately-run backend).
        .layer(
            CorsLayer::new()
                .allow_origin(cors::AllowOrigin::mirror_request())
                .allow_methods(cors::AllowMethods::mirror_request())
                .allow_headers(cors::AllowHeaders::mirror_request())
                .allow_credentials(true),
        )
        .with_state(state)
}

/// Salvage or drop stage sessions whose tab died mid-recording. Stage-dir mtime refreshes on
/// every ~15s chunk PUT, so >30 min of stillness means abandoned — no commit is coming. If
/// chunks landed, promote them into a normal enriching clip (the user's recording is not
/// lost — it shows up in their library like any other); if nothing landed, drop the empty
/// `recording` stub so the library doesn't accumulate ghosts.
async fn sweep_abandoned_stages(s: &AppState) {
    const STALE_SECS: u64 = 30 * 60;
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(suffix) = name.strip_prefix("clipxd-stage-clp_") else { continue };
        let id = format!("clp_{suffix}");
        if !safe(&id) {
            continue;
        }
        let stale = e.metadata().and_then(|m| m.modified()).ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|el| el.as_secs() > STALE_SECS);
        if !stale {
            continue;
        }
        // Claim the id before touching stage_dir/clip_dir at all. A staged commit or a
        // reuse-fallback ingest may be finalizing this exact id right now (its stage-dir mtime
        // can look stale if it's been blocked awaiting a slow in-flight add_increment) — losing
        // this race means skip this tick entirely rather than run promote_staged/delete
        // concurrently with whoever's holding it (two finalizers writing clip_dir at once was
        // the CRITICAL bug this claim exists to prevent).
        let Some(claim) = try_claim(&s.clip_claims, &id) else {
            continue;
        };
        let slot = s.stage_sessions.lock().await.remove(&id);
        let incremental = match slot {
            Some(slot) => tokio::task::spawn_blocking(move || slot.lock().ok().and_then(|mut g| g.take())).await.unwrap_or(None),
            None => None,
        };
        let stage_dir = e.path();
        let clip_dir = s.clips_dir.join(&id);
        let has_chunks = std::fs::read_dir(&stage_dir)
            .map(|mut it| it.any(|c| c.is_ok_and(|c| c.file_name().to_string_lossy().starts_with("chunk-"))))
            .unwrap_or(false);
        if has_chunks {
            eprintln!("sweeper: salvaging abandoned recording {id}");
            let promoted = tokio::task::spawn_blocking({
                let (stage_dir, clip_dir, id) = (stage_dir.clone(), clip_dir.clone(), id.clone());
                move || promote_staged(&stage_dir, &clip_dir, &id)
            })
            .await;
            match promoted {
                Ok(Ok(video)) => spawn_phase2(s, id, clip_dir, video, incremental, claim, None),
                Ok(Err(e)) => eprintln!("sweeper: salvage failed for {id}: {e:#}"),
                Err(e) => eprintln!("sweeper: salvage task died for {id}: {e}"),
            }
        } else {
            let _ = std::fs::remove_dir_all(&stage_dir);
            let _ = std::fs::remove_dir_all(stage_frames_dir(&id));
            let is_empty_recording_stub = std::fs::read_to_string(clip_dir.join("index.json")).ok()
                .and_then(|s| serde_json::from_str::<Index>(&s).ok())
                .is_some_and(|i| i.status == clipxd_index::Status::Recording)
                && !clip_dir.join("video.webm").exists();
            if is_empty_recording_stub {
                let _ = std::fs::remove_dir_all(&clip_dir);
            }
        }
    }
}

/// Reap clips left at `status: recording` whose stage dir is gone entirely.
///
/// `sweep_abandoned_stages` is driven by `read_dir(temp_dir())`, so it can only ever see a
/// clip whose `clipxd-stage-<id>` dir still exists — a missing stage dir is a blind spot to
/// it, not a signal. When the OS tmp reaper (or a reboot, or a container restart) removes
/// the stage dir while the `index.json` stub survives in `clips_dir`, the clip becomes
/// invisible to that sweep and sits in the library saying "recording…" forever, with no code
/// path anywhere that can ever move it off that status. This pass walks `clips_dir` instead,
/// which inverts the test: the *absence* of a stage dir is exactly what marks the ghost.
async fn sweep_orphaned_recording_stubs(s: &AppState) {
    const STALE_SECS: u64 = 30 * 60;
    let Ok(entries) = std::fs::read_dir(s.clips_dir.as_path()) else { return };
    for e in entries.flatten() {
        let id = e.file_name().to_string_lossy().to_string();
        if !safe(&id) {
            continue;
        }
        // A recording that is genuinely still running owns a live stage dir (chunk PUTs keep
        // it there). Never touch one, however long it has been going — this is the guard that
        // makes the mtime check below safe for a 3-hour recording.
        if std::env::temp_dir().join(format!("clipxd-stage-{id}")).exists() {
            continue;
        }
        let clip_dir = e.path();
        let index_path = clip_dir.join("index.json");
        let Some(idx) = std::fs::read_to_string(&index_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Index>(&raw).ok())
        else {
            continue;
        };
        if idx.status != clipxd_index::Status::Recording {
            continue;
        }
        // The stub is written once at stage-open and not touched again until commit, so its
        // mtime is effectively "when this recording started".
        let stale = std::fs::metadata(&index_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|el| el.as_secs() > STALE_SECS);
        if !stale {
            continue;
        }
        // Don't race a commit / reuse-fallback ingest that is finalizing this very id.
        let Some(_claim) = try_claim(&s.clip_claims, &id) else {
            continue;
        };
        let has_video = clip_dir.join("video.webm").exists() || clip_dir.join("video.mp4").exists();
        if has_video {
            // The video landed but the commit never finished the index. Keep the recording —
            // just stop lying about its status.
            eprintln!("sweeper: orphaned recording {id} has video — marking partial");
            mark_partial(&clip_dir);
            if let Ok(raw) = std::fs::read(&index_path) {
                write_object_best_effort(s, &format!("{id}/index.json"), raw, "application/json").await;
            }
        } else {
            eprintln!("sweeper: dropping orphaned empty recording stub {id}");
            let _ = std::fs::remove_dir_all(&clip_dir);
        }
    }
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

/// `GET /settings/keys` — presence/absence of each BYOK key + caption mode for the caller.
/// Never returns the actual key values (see [`auth::KeyStatus`]).
async fn get_settings_keys(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<auth::KeyStatus>, WebErr> {
    let a = auth_of(&s)?;
    let principal = auth::authenticate(&a.jwt_secret, &headers).ok_or((StatusCode::UNAUTHORIZED, "not logged in".into()))?;
    let status = a.db.key_status(principal.id).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(status))
}

/// A raw JSON-value field: parsed as "leave unchanged" (key absent from the body), "clear"
/// (`null`, or a string that's empty/all-whitespace once trimmed), or "set" (a non-empty
/// trimmed string) — a plain `Option<String>` can't distinguish "absent" from "explicit null",
/// so the body is read as a generic object and each field inspected directly instead of via a
/// `#[derive(Deserialize)]` struct.
fn norm_key_field(v: &serde_json::Value) -> Result<Option<String>, WebErr> {
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => {
            let t = s.trim();
            Ok(if t.is_empty() { None } else { Some(t.to_string()) })
        }
        _ => Err((StatusCode::BAD_REQUEST, "expected a string or null".into())),
    }
}

/// `POST /settings/keys` — set/clear any subset of the caller's BYOK keys and/or caption mode.
/// Body: `{"nvidia_api_key"?: string|null, "gemini_api_key"?: string|null,
/// "moondream_api_key"?: string|null, "caption_mode"?: "server"|"local"}`. Fields absent from
/// the body are left untouched; `null` (or an all-whitespace string) clears that key. Returns
/// the caller's fresh [`auth::KeyStatus`] — never the raw key values.
async fn post_settings_keys(State(s): State<AppState>, headers: HeaderMap, Json(body): Json<serde_json::Value>) -> Result<Json<auth::KeyStatus>, WebErr> {
    let a = auth_of(&s)?;
    let principal = auth::authenticate(&a.jwt_secret, &headers).ok_or((StatusCode::UNAUTHORIZED, "not logged in".into()))?;
    let obj = body.as_object().ok_or((StatusCode::BAD_REQUEST, "expected a JSON object body".into()))?;

    if let Some(v) = obj.get("nvidia_api_key") {
        let key = norm_key_field(v)?;
        a.db.set_nvidia_key(principal.id, key.as_deref()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    if let Some(v) = obj.get("gemini_api_key") {
        let key = norm_key_field(v)?;
        a.db.set_gemini_key(principal.id, key.as_deref()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    if let Some(v) = obj.get("moondream_api_key") {
        let key = norm_key_field(v)?;
        a.db.set_moondream_key(principal.id, key.as_deref()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    if let Some(v) = obj.get("caption_mode") {
        let mode = v.as_str().ok_or((StatusCode::BAD_REQUEST, "caption_mode must be a string".into()))?;
        a.db.set_caption_mode(principal.id, mode).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }

    let status = a.db.key_status(principal.id).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(status))
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
    let bytes = read_object_or_local(state, &key).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("read {key}: {e}")))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no clip {id}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("bad index: {e}")))
}

/// Read `key` from the configured storage, falling back to the local clips dir. Every clip
/// file is written locally *first* and mirrored to S3 in the background (the mirror is off
/// the request path since the instant-link work), so a just-created clip must stay readable
/// from local disk while its S3 copy is still uploading. `key` components are `safe()`-checked
/// by every caller before reaching here.
async fn read_object_or_local(state: &AppState, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
    if let Some(bytes) = state.storage.make_storage().await?.read_object(key).await? {
        return Ok(Some(bytes));
    }
    let local = state.clips_dir.join(key);
    if local.exists() {
        return Ok(Some(tokio::fs::read(&local).await?));
    }
    Ok(None)
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

/// Read a clip's current view count without incrementing it (0 if never viewed).
async fn view_count(s: &AppState, id: &str) -> u64 {
    read_object_or_local(s, &format!("{id}/views.txt"))
        .await
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0)
}

/// Record one view of a clip and return the new total. Kept as a plain counter file
/// alongside the clip's other side-files (zoom.json, events.json) rather than a field on
/// `Index` itself — views are ephemeral traffic metadata, not part of what the recording
/// *is*, and folding them into index.json would mean every page view rewrites (and risks
/// racing) the whole index during background enrichment. Read-modify-write isn't atomic
/// across concurrent requests; an occasional lost increment under simultaneous viewers is an
/// acceptable trade for staying a plain file instead of a database, since this is a rough
/// traffic count, not a strict ledger.
async fn bump_view_count(s: &AppState, id: &str) -> u64 {
    let next = view_count(s, id).await + 1;
    let key = format!("{id}/views.txt");
    let _ = tokio::fs::write(s.clips_dir.join(&key), next.to_string()).await;
    write_object_best_effort(s, &key, next.to_string().into_bytes(), "text/plain").await;
    next
}

/// A timestamped comment on a clip — the unit of the Fathom-style "chat about this minute"
/// thread. Anchored to `t` (seconds into the clip) so the UI can render it against the
/// timeline and click-to-seek. `@m:ss` mentions inside `text` are parsed client-side into
/// seek links; stored verbatim here.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Comment {
    id: String,
    t: f64,
    author: String,
    text: String,
    created_at: u64,
}

/// Read a clip's comment thread (empty if none yet). Stored as `<id>/comments.json` — a
/// sibling side-file, same rationale as the view counter: it's conversation *about* the clip,
/// not part of what the recording *is*, so it stays out of index.json and never races the
/// enrichment that rewrites the index.
async fn read_comments(s: &AppState, id: &str) -> Vec<Comment> {
    read_object_or_local(s, &format!("{id}/comments.json"))
        .await
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_slice::<Vec<Comment>>(&b).ok())
        .unwrap_or_default()
}

/// Append a comment and persist. Read-modify-write is non-atomic across concurrent posters
/// (same acceptable trade as the view counter — this is a discussion thread, not a ledger).
async fn append_comment(s: &AppState, id: &str, c: &Comment) -> anyhow::Result<()> {
    let mut all = read_comments(s, id).await;
    all.push(c.clone());
    let bytes = serde_json::to_vec(&all)?;
    let key = format!("{id}/comments.json");
    tokio::fs::write(s.clips_dir.join(&key), &bytes).await?;
    write_object_best_effort(s, &key, bytes, "application/json").await;
    Ok(())
}

/// Resolve the display name for a comment's author from the authenticated user: their
/// username if they've set one, else the local-part of their email.
fn author_name(s: &AppState, user: &AuthUser) -> String {
    if let Some(a) = &s.auth {
        if let Ok(Some(u)) = a.db.find_by_id(user.id) {
            if let Some(name) = u.username.as_deref().filter(|n| !n.is_empty()) {
                return name.to_string();
            }
        }
    }
    user.email.split('@').next().unwrap_or("someone").to_string()
}

#[derive(serde::Deserialize)]
struct NewComment {
    t: f64,
    text: String,
}

/// `GET /clip/:id/comments` — public read of the thread.
async fn get_comments(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    let comments = read_comments(&s, &id).await;
    Ok(Json(serde_json::json!({ "comments": comments })))
}

/// `POST /clip/:id/comments` — add a comment (login required; author is the caller). In local
/// mode (no auth configured) posting is open and authored as "you".
async fn post_comment(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<NewComment>,
) -> Result<Json<Comment>, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    let text = body.text.trim();
    if text.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty comment".into()));
    }
    if text.chars().count() > 2000 {
        return Err((StatusCode::BAD_REQUEST, "comment too long (2000 char max)".into()));
    }
    load_index(&s, &id).await?; // 404 cleanly if the clip doesn't exist
    let author = match require_user(&s, &headers)? {
        Some(u) => author_name(&s, &u),
        None => "you".to_string(), // local mode (auth disabled)
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let comment = Comment {
        id: format!("cmt_{:x}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)),
        t: body.t.max(0.0),
        author,
        text: text.to_string(),
        created_at: now,
    };
    append_comment(&s, &id, &comment).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("save comment: {e}")))?;
    Ok(Json(comment))
}

async fn get_comments_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, WebErr> {
    check_owner(&s, &username, &id)?;
    get_comments(State(s), Path(id)).await
}

async fn post_comment_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Json<NewComment>,
) -> Result<Json<Comment>, WebErr> {
    check_owner(&s, &username, &id)?;
    post_comment(State(s), Path(id), headers, body).await
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

/// `GET /clip/:id/agent.md` — the whole index rendered as agent-readable markdown. This is
/// the "paste a clip link into your agent" surface: one plain-text fetch returns everything
/// an agent needs (transcript, captions, on-screen text, events, chapters) plus pointers to
/// the query endpoints for anything deeper — no video ever watched.
async fn get_agent_md(State(s): State<AppState>, Path(id): Path<String>) -> Result<impl IntoResponse, WebErr> {
    let idx = load_index(&s, &id).await?;
    Ok(([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], agent_markdown(&idx, &id)))
}

async fn get_agent_md_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
) -> Result<impl IntoResponse, WebErr> {
    check_owner(&s, &username, &id)?;
    get_agent_md(State(s), Path(id)).await
}

/// `GET /clip/:id/doc/:kind` — a video-to-document workflow: turn the clip's index into a
/// real markdown document. `kind` is `pr-description` (or `pr`) / `sop` / `qa-steps` (or
/// `qa`). Generated live via the shared NVIDIA/Gemini-fallback LLM primitive — not cached,
/// not run in the background; this is a per-ask output, unlike title/tl;dr/chapters which
/// the deep pass writes into the index once. 404 if `kind` isn't recognized, 503 if no LLM
/// backend is configured (`NVIDIA_API_KEY` / `GEMINI_API_KEY`), 422 if the clip has nothing
/// yet to synthesize from (still enriching/recording).
async fn get_doc(State(s): State<AppState>, Path((id, kind)): Path<(String, String)>) -> Result<impl IntoResponse, WebErr> {
    let kind = docgen::DocKind::parse(&kind).ok_or((StatusCode::NOT_FOUND, "unknown doc kind (want pr-description | sop | qa-steps)".into()))?;
    let idx = load_index(&s, &id).await?;
    let text = docgen::generate(&idx, kind).await.map_err(|e| {
        let msg = format!("{e:#}");
        let status = if msg.contains("no LLM backend configured") { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::UNPROCESSABLE_ENTITY };
        (status, msg)
    })?;
    Ok(([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], text))
}

async fn get_doc_for_user(
    State(s): State<AppState>,
    Path((username, id, kind)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, WebErr> {
    check_owner(&s, &username, &id)?;
    get_doc(State(s), Path((id, kind))).await
}

/// Render an [`Index`] as markdown for agent consumption. Long streams are capped (with an
/// explicit truncation note) so the document stays a sane single fetch; `index.json` is the
/// lossless fallback.
fn agent_markdown(idx: &Index, id: &str) -> String {
    use std::fmt::Write;
    const CAP: usize = 800;
    let ts = |secs: f64| -> String {
        let t = secs.max(0.0) as u64;
        let (h, m, s) = (t / 3600, (t % 3600) / 60, t % 60);
        if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
    };
    let mut md = String::with_capacity(16 * 1024);
    let title = if idx.metadata.title.is_empty() { id } else { &idx.metadata.title };
    let _ = writeln!(md, "# {title}\n");
    if !idx.summary.tldr.is_empty() {
        let _ = writeln!(md, "{}\n", idx.summary.tldr);
    }
    let status = serde_json::to_value(idx.status).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let source = serde_json::to_value(idx.source).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let _ = writeln!(
        md,
        "- id: `{id}` · source: {source} · status: {status}\n- duration: {} · resolution: {}x{}\n",
        ts(idx.metadata.duration),
        idx.metadata.resolution[0],
        idx.metadata.resolution[1],
    );
    let _ = writeln!(
        md,
        "## Query API\n\n- `GET /clip/{id}/query?q=<question>` → answer with timestamp citations\n- `GET /clip/{id}/search?q=<text>` → matching moments\n- `GET /clip/{id}/index.json` → the full structured index (lossless)\n- `GET /clip/{id}/video` → the video itself (you should not need it)\n"
    );
    if !idx.summary.chapters.is_empty() {
        let _ = writeln!(md, "## Chapters ({})\n", idx.summary.chapters.len());
        for c in &idx.summary.chapters {
            let _ = writeln!(md, "- [{}] {}", ts(c.start), c.title);
        }
        let _ = writeln!(md);
    }
    if !idx.transcript.is_empty() {
        let _ = writeln!(md, "## Transcript ({} segments)\n", idx.transcript.len());
        for seg in idx.transcript.iter().take(CAP) {
            let _ = writeln!(md, "- [{}–{}] {}", ts(seg.start), ts(seg.end), seg.text);
        }
        if idx.transcript.len() > CAP {
            let _ = writeln!(md, "- … {} more segments in `index.json`", idx.transcript.len() - CAP);
        }
        let _ = writeln!(md);
    }
    if !idx.visual_timeline.is_empty() {
        let _ = writeln!(md, "## Salient moments ({})\n", idx.visual_timeline.len());
        for m in idx.visual_timeline.iter().take(CAP) {
            let _ = writeln!(md, "- [{}] {} _(delta: {})_", ts(m.t), m.caption, m.delta);
        }
        if idx.visual_timeline.len() > CAP {
            let _ = writeln!(md, "- … {} more moments in `index.json`", idx.visual_timeline.len() - CAP);
        }
        let _ = writeln!(md);
    }
    if !idx.on_screen_text.is_empty() {
        let _ = writeln!(md, "## On-screen text ({} spans, OCR)\n", idx.on_screen_text.len());
        for t in idx.on_screen_text.iter().take(CAP) {
            let _ = writeln!(md, "- [{}] {}", ts(t.start), t.text);
        }
        if idx.on_screen_text.len() > CAP {
            let _ = writeln!(md, "- … {} more spans in `index.json`", idx.on_screen_text.len() - CAP);
        }
        let _ = writeln!(md);
    }
    if !idx.event_track.is_empty() {
        let _ = writeln!(md, "## Events ({})\n", idx.event_track.len());
        for e in idx.event_track.iter().take(CAP) {
            let _ = writeln!(md, "- [{}] {} {}", ts(e.t), e.kind, e.text.as_deref().unwrap_or(""));
        }
        if idx.event_track.len() > CAP {
            let _ = writeln!(md, "- … {} more events in `index.json`", idx.event_track.len() - CAP);
        }
        let _ = writeln!(md);
    }
    md
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
    let bytes = read_object_or_local(&s, &format!("{id}/zoom.json")).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")))?.ok_or((StatusCode::NOT_FOUND, "no zoom track".into()))?;
    Ok(([(header::CONTENT_TYPE, "application/json")], bytes))
}

/// The clip's real first salient frame — NOT necessarily `frames/00001.*`. The salience gate
/// skips frames until something actually changes on screen, so a clip's first *retained* frame
/// is commonly `00003.jpg`, `00007.jpg`, etc. Anything that hardcodes `00001` (og:image, video
/// poster, library thumbnails) 404s on exactly the clips where that skip happens — which is
/// most of them. Falls back to the historical `frames/00001.png` guess only when the index has
/// no visual_timeline at all (e.g. an ancient or transcript-only clip).
fn first_frame_ref(idx: &Index) -> &str {
    idx.visual_timeline.iter().find_map(|m| m.frame_ref.as_deref()).unwrap_or("frames/00001.png")
}

/// `GET /clip/:id/thumbnail` — the clip's real first salient frame, whatever it's actually
/// named. Library cards and any other "just show me a still" consumer should hit this instead
/// of guessing a frame path directly.
async fn get_thumbnail(State(s): State<AppState>, Path(id): Path<String>) -> Result<impl IntoResponse, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    let idx = load_index(&s, &id).await?;
    let frame_ref = first_frame_ref(&idx).to_string();
    let bytes = read_object_or_local(&s, &format!("{id}/{frame_ref}")).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")))?
        .ok_or((StatusCode::NOT_FOUND, "no salient frame yet".into()))?;
    let ct = if frame_ref.ends_with(".jpg") || frame_ref.ends_with(".jpeg") { "image/jpeg" } else { "image/png" };
    Ok(([(header::CONTENT_TYPE, ct)], bytes))
}

async fn get_thumbnail_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
) -> Result<impl IntoResponse, WebErr> {
    check_owner(&s, &username, &id)?;
    get_thumbnail(State(s), Path(id)).await
}

/// `GET /clip/:id/preview.gif` — an animated GIF built from salient frames, for pasting into
/// email/Slack/Notion where an animated thumbnail is the highest-converting share surface
/// (Loom's "wave at the camera" GIF pattern). Generated once, cached to storage at
/// `<id>/preview.gif`; subsequent requests just serve the cached bytes.
async fn get_preview_gif(State(s): State<AppState>, Path(id): Path<String>) -> Result<impl IntoResponse, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    let key = format!("{id}/preview.gif");
    if let Some(cached) = read_object_or_local(&s, &key).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")))? {
        return Ok(([(header::CONTENT_TYPE, "image/gif")], cached));
    }
    let idx = load_index(&s, &id).await?;
    let storage = s.storage.make_storage().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")))?;
    let gif = preview_gif::generate(storage.as_ref(), &id, &idx).await.map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")))?;
    write_object_best_effort(&s, &key, gif.clone(), "image/gif").await;
    Ok(([(header::CONTENT_TYPE, "image/gif")], gif))
}

async fn get_video(State(s): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    let mut found: Option<(String, Vec<u8>)> = None;
    for n in ["video.mp4", "video.webm", "source.mp4"] {
        if let Some(bytes) = read_object_or_local(&s, &format!("{id}/{n}")).await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("read video: {e}")))? {
            found = Some((n.to_string(), bytes));
            break;
        }
    }
    // Instant-link recording in progress: no committed video yet, but the stage session's
    // growing video-so-far is a valid WebM prefix — serve it so the share page can play a
    // live preview (and so playback works in the stop→commit gap).
    if found.is_none() {
        let staged = std::env::temp_dir().join(format!("clipxd-stage-{id}")).join("video-so-far.webm");
        if let Ok(bytes) = tokio::fs::read(&staged).await {
            found = Some(("video.webm".to_string(), bytes));
        }
    }
    let (video_key, bytes) = found.ok_or((StatusCode::NOT_FOUND, "no video".into()))?;
    let ct = if video_key.ends_with(".webm") { "video/webm" } else { "video/mp4" };
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
    // Extension-agnostic: frames used to be PNG and are now JPEG, and several consumers
    // (og:image, poster, thumbnails) hardcode `00001.png` — resolve across both so old
    // links keep working on new clips and vice versa.
    let mut resolved = name.clone();
    let mut bytes = read_object_or_local(&s, &format!("{id}/frames/{name}")).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")))?;
    if bytes.is_none() {
        let alt = match name.rsplit_once('.') {
            Some((stem, "png")) => Some(format!("{stem}.jpg")),
            Some((stem, "jpg")) | Some((stem, "jpeg")) => Some(format!("{stem}.png")),
            _ => None,
        };
        if let Some(alt) = alt {
            bytes = read_object_or_local(&s, &format!("{id}/frames/{alt}")).await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")))?;
            if bytes.is_some() {
                resolved = alt;
            }
        }
    }
    let bytes = bytes.ok_or((StatusCode::NOT_FOUND, "no frame".into()))?;
    let ct = if resolved.ends_with(".jpg") || resolved.ends_with(".jpeg") { "image/jpeg" } else { "image/png" };
    Ok(([(header::CONTENT_TYPE, ct)], bytes))
}

#[derive(Deserialize)]
struct IngestQuery {
    /// Instant-link fallback: commit under this already-minted `clp_` id instead of minting a
    /// new one. The client sends it when a staged commit failed after the id (and its share
    /// URL) already existed — possibly already copied and shared mid-recording — so the
    /// recording must land under the *same* URL, never a fresh one.
    reuse: Option<String>,
}

/// `POST /ingest` — accept a screen-recording (webm bytes from the browser's MediaRecorder).
/// **Loom-style, two-phase:** Phase 1 (fast) saves the video + a minimal `status: enriching`
/// index and returns the clip id *immediately* — so the clip is instantly watchable, listable,
/// and shareable. Phase 2 (the slow OCR/captioning) runs in a background task and rewrites the
/// index to `complete` when done. A recording is therefore never lost to slow/failed enrichment.
async fn ingest(State(s): State<AppState>, Query(q): Query<IngestQuery>, headers: HeaderMap, body: Bytes) -> Result<Json<serde_json::Value>, WebErr> {
    let user = require_user(&s, &headers)?;
    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty body".into()));
    }
    // A reuse id is honored only for an *unfinished* stub (recording/enriching — a complete
    // clip can never be overwritten through this door) owned by the caller in auth mode.
    let reuse = q.reuse
        .filter(|rid| safe(rid) && rid.starts_with("clp_"))
        .filter(|rid| {
            let unfinished = std::fs::read_to_string(s.clips_dir.join(rid).join("index.json")).ok()
                .and_then(|j| serde_json::from_str::<Index>(&j).ok())
                .is_some_and(|i| matches!(i.status, clipxd_index::Status::Recording | clipxd_index::Status::Enriching));
            let owned = match (&s.auth, &user) {
                (Some(a), Some(u)) => a.db.clip_owner(rid).ok().flatten().map_or(true, |o| o == u.id),
                (Some(_), None) => false,
                _ => true,
            };
            unfinished && owned
        });
    if let Some(rid) = &reuse {
        // The full-blob body supersedes whatever the failed staged session left behind.
        s.stage_sessions.lock().await.remove(rid);
        let _ = tokio::fs::remove_dir_all(std::env::temp_dir().join(format!("clipxd-stage-{rid}"))).await;
    }
    ingest_bytes(s, user, body, None, reuse).await
}

/// Shared ingest core — called by both `/ingest` (full blob) and `/ingest/stage/:s/commit`
/// (reassembled chunks). Two-phase: Phase 1 persists + stubs immediately, Phase 2 enriches in
/// the background. `incremental`, when the staged-upload path already accumulated one, replaces
/// Phase 2's from-scratch `enrich_clip` with one final pass over the already-mostly-indexed
/// session — see `clipxd_recorder::incremental` for why that's usually much faster.
/// `reuse` (pre-validated by the caller) commits under an existing instant-link id whose share
/// URL is already out in the world, instead of minting a fresh one.
async fn ingest_bytes(s: AppState, user: Option<AuthUser>, body: Bytes, incremental: Option<IncrementalIndexer>, reuse: Option<String>) -> Result<Json<serde_json::Value>, WebErr> {
    let dir = s.clips_dir.clone();
    let id = reuse.unwrap_or_else(mint_clip_id);

    // Claim the id for the *whole* finalization — Phase 1 below through Phase 2's background
    // completion — so a concurrent staged-commit, reuse-fallback, or sweeper pass for the
    // same id can never write clip_dir at the same time. Held past this function's return by
    // handing it into `spawn_phase2`, which drops it when the background job finishes.
    let claim = try_claim(&s.clip_claims, &id)
        .ok_or((StatusCode::CONFLICT, "clip is already being finalized".into()))?;

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

    spawn_phase2(&s, id.clone(), dir.join(&id), video, incremental, claim, None);

    Ok(Json(serde_json::json!({ "id": id })))
}

/// Spawn the background tail of an ingest: mirror the Phase-1 stub to S3 (previously this
/// blocked the response — the full video upload was the single largest post-Stop latency in
/// hosted mode; `read_object_or_local` covers the gap), run Phase 2 enrichment, then re-mirror
/// so the captions/OST/zoom land in S3 too. On enrich failure the index is marked `partial`
/// (still watchable) rather than left stuck on `enriching`. `claim` — this id's [`ClaimGuard`]
/// — is held for the entire background job and only drops (releasing the id) once every write
/// to `clip_dir` here is done, so no other finalizer for the same id can start until this one
/// truly finishes. `permits` bounds how many of these run at once on the box.
/// Fuse a Browser-mode capture trace into a video clip's already-enriched index: the video
/// enrichment gave us transcript/frame-OCR/captions; the trace adds the real interaction event
/// track (clicks/input/console/network/navigation) plus DOM text/snapshots that pixels can't
/// carry. We keep the video index's transcript/frames and graft the trace's streams on, then
/// re-clean (dedup + rebuild the agent search corpus).
fn merge_browser_trace_into_clip(clip_dir: &std::path::Path, id: &str, trace_json: &str) -> anyhow::Result<()> {
    let index_path = clip_dir.join("index.json");
    let mut idx: Index = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;
    let created_at = idx.metadata.created_at.clone();
    let b = clipxd_browser::ingest_str(trace_json, id, &created_at, &clipxd_browser::SalienceOpts::default())?;
    idx.event_track.extend(b.event_track);
    idx.event_track.sort_by(|x, y| x.t.partial_cmp(&y.t).unwrap_or(std::cmp::Ordering::Equal));
    idx.on_screen_text.extend(b.on_screen_text);
    idx.visual_timeline.extend(b.visual_timeline);
    clipxd_index::clean_index(&mut idx); // dedup streams + rebuild search.events/screen_text
    std::fs::write(&index_path, serde_json::to_string_pretty(&idx)?)?;
    Ok(())
}

/// The clip owner's own NVIDIA/Gemini BYOK keys, if auth is on and the clip has a known owner
/// who set any — `(None, None)` otherwise, which tells `llm::complete_with_keys` to fall back
/// to the server's env-configured keys.
fn owner_llm_keys(auth: &Option<AuthState>, id: &str) -> (Option<String>, Option<String>) {
    let Some(a) = auth else { return (None, None) };
    let Some(owner_id) = a.db.clip_owner(id).ok().flatten() else { return (None, None) };
    a.db.llm_keys(owner_id).unwrap_or((None, None))
}

/// The clip owner's caption backend override, if any — `None` means "use the server's own
/// env-driven cascade unchanged" (see [`veyo_enrich::Enricher::with_local_defaults`]).
/// `caption_mode: local` wins outright (fully offline heuristic captions, no VLM/network at
/// all for this clip); otherwise an owner-set Moondream cloud key overrides just the captioner
/// — everything else (OCR, transcript) is untouched and every *other* owner's clips are
/// unaffected, since this is looked up per clip id rather than mutating shared process env.
fn owner_caption_source(auth: &Option<AuthState>, id: &str) -> Option<veyo_enrich::CaptionSource> {
    let a = auth.as_ref()?;
    let owner_id = a.db.clip_owner(id).ok().flatten()?;
    let status = a.db.key_status(owner_id).ok()?;
    if status.caption_mode == "local" {
        return Some(veyo_enrich::CaptionSource::Local);
    }
    if status.has_moondream {
        if let Some(token) = a.db.moondream_key(owner_id).ok().flatten() {
            return Some(veyo_enrich::CaptionSource::MoondreamKey(token));
        }
    }
    None
}

fn spawn_phase2(s: &AppState, id: String, clip_dir: PathBuf, video: PathBuf, incremental: Option<IncrementalIndexer>, claim: ClaimGuard, browser_trace: Option<String>) {
    let storage_arc = s.storage.clone();
    let permits = s.phase2_permits.clone();
    let auth = s.auth.clone();
    tokio::spawn(async move {
        let _claim = claim; // held until this async block ends
        // The permit bounds concurrent *compute-heavy* enrichment (ffmpeg decode → gate → OCR →
        // whisper), NOT the network-bound LLM refinement that follows. It's dropped the moment
        // the local enrichment finishes (see `drop(permit)` below) so a slow/unreachable LLM
        // backend can never throttle indexing throughput — only two boxes-worth of ffmpeg do.
        let permit = permits.acquire_owned().await.ok();
        if let Ok(st) = storage_arc.make_storage().await {
            if let Err(e) = mirror_dir_to_storage(st.as_ref(), &id, &clip_dir).await {
                eprintln!("ingest stub mirror: {e} (continuing)");
            }
        }
        let bg_id = id.clone();
        let bg_dir = clip_dir.clone();
        // BYOK/local-mode caption override for the from-scratch path — the incremental path
        // already baked this into `indexer` back at `/ingest/stage` creation time.
        let caption_source = owner_caption_source(&auth, &bg_id);
        let _ = tokio::task::spawn_blocking(move || {
            let result = match incremental {
                Some(indexer) => indexer.finalize(&video, &bg_dir, &bg_id, "Screen recording", &EventTrack::default()).map(|_| ()),
                None => clipxd_recorder::enrich_clip(&video, &bg_dir, &bg_id, "Screen recording", &EventTrack::default(), 4.0, caption_source).map(|_| ()),
            };
            if let Err(e) = result {
                eprintln!("background enrich failed for {bg_id}: {e:#}");
                mark_partial(&bg_dir);
            }
        })
        .await;
        // Browser-mode fusion: if the extension recorded this tab's video WITH a structured
        // trace, merge the trace's interaction/DOM streams into the just-built video index —
        // AFTER enrichment (same task, so it never races the finalize that writes index.json).
        // Runs before auto-title so the title sees the real event/DOM context too. One clip is
        // now both watchable (video/transcript/frames) and queryable (clicks/console/network).
        if let Some(trace) = browser_trace {
            let (md, mid) = (clip_dir.clone(), id.clone());
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(e) = merge_browser_trace_into_clip(&md, &mid, &trace) {
                    eprintln!("browser-trace merge for {mid}: {e:#} (continuing)");
                }
            })
            .await;
        }
        // Local enrichment is done and the clip is already `complete` + watchable. Everything
        // below is network-bound LLM refinement (title/description, caption emphasis, optional
        // deep pass) over the finished index — release the compute permit now so the next
        // recording's ffmpeg work isn't stuck behind this clip's LLM round-trips.
        drop(permit);
        // BYOK: use the clip owner's own NVIDIA/Gemini keys if they've set any, so their usage
        // lands on their own account/quota instead of the server's shared one. `None` for
        // either falls back to the server's env-configured key inside `llm::complete_with_keys`.
        let (nvidia_key, gemini_key) = owner_llm_keys(&auth, &id);
        let has_llm_key = nvidia_key.is_some() || gemini_key.is_some() || llm::any_backend_configured();
        // Auto-title + description: unconditional (any LLM backend configured is enough) —
        // cheap, one short prompt, one small JSON reply. Every clip deserves a real name and a
        // one-line library-card description instead of sitting there as "Screen recording"
        // forever. Runs before the optional deep pass so a clip that only gets this (deep pass
        // off) still gets named.
        if has_llm_key {
            if let Err(e) = deeppass::generate_title_and_description(&clip_dir, &id, nvidia_key.as_deref(), gemini_key.as_deref()).await {
                eprintln!("auto-title for {id}: {e:#} (continuing)");
            }
        }
        // Caption-emphasis pass: Ollama-Cloud-first LLM marks which transcript words to focus on
        // for the Karaoke/Bold subtitle designs. Same indexing-time, log-and-swallow contract as
        // auto-title — gated on any backend being configured (server- or owner-supplied), so a
        // local-first box with no key is unaffected and the clip completes unchanged on failure.
        if has_llm_key {
            if let Err(e) = emphasis::run(&clip_dir, &id, nvidia_key.as_deref(), gemini_key.as_deref()).await {
                eprintln!("emphasis pass for {id}: {e:#} (continuing)");
            }
        }
        // Optional Tier-2 deep pass (NVIDIA/Gemini text-context → title/tldr/chapters). Off
        // unless CLIPXD_DEEP_PASS=1 + a backend key (server- or owner-supplied) are available —
        // the local-first default sends nothing anywhere. Runs before the final mirror so the
        // merged index lands in S3.
        if deeppass::enabled(nvidia_key.is_some() || gemini_key.is_some()) {
            if let Err(e) = deeppass::run(&clip_dir, &id, nvidia_key.as_deref(), gemini_key.as_deref()).await {
                eprintln!("deep pass for {id}: {e:#} (continuing)");
            }
        }
        if let Ok(st) = storage_arc.make_storage().await {
            if let Err(e) = mirror_dir_to_storage(st.as_ref(), &id, &clip_dir).await {
                eprintln!("post-enrich mirror: {e} (continuing)");
            }
        }
    });
}

/// Flip a clip's index to `status: partial` on disk — the honest "enrichment died but the
/// video is fine" signal.
fn mark_partial(clip_dir: &std::path::Path) {
    if let Ok(s) = std::fs::read_to_string(clip_dir.join("index.json")) {
        if let Ok(mut idx) = serde_json::from_str::<Index>(&s) {
            idx.status = clipxd_index::Status::Partial;
            let _ = std::fs::write(clip_dir.join("index.json"), serde_json::to_string_pretty(&idx).unwrap_or_default());
        }
    }
}

/// Where a stage session's incrementally-extracted frames live while recording is in
/// progress — separate from the chunks dir (`clipxd-stage-{session}`) so the indexer's
/// `finalize` can move them into `clip_dir/frames` regardless of when the chunks dir itself
/// gets cleaned up at commit.
fn stage_frames_dir(session: &str) -> PathBuf {
    std::env::temp_dir().join(format!("clipxd-stage-frames-{session}"))
}

/// `POST /ingest/stage` — begin a streaming upload session. **The session IS the clip**: the
/// `clp_` id is minted here, at record start — not at commit — so the share URL exists (and
/// resolves, via a `status: recording` stub) the moment recording begins. This is the
/// instant-link mechanic (Cap pre-creates its `/s/{id}` link the same way); the roadmap gate
/// is "record-to-shareable-link < 1s". Also registers a fresh [`IncrementalIndexer`] so
/// chunks start indexing as they land.
/// Returns `{"id": "<clp_…>", "session": "<same>"}`; the client then PUTs 15-second chunks.
/// (`session` is kept as an alias so older clients keep working.)
async fn ingest_stage_create(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<serde_json::Value>, WebErr> {
    let user = require_user(&s, &headers)?;
    let id = mint_clip_id();
    let dir = std::env::temp_dir().join(format!("clipxd-stage-{id}"));
    tokio::fs::create_dir_all(&dir).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Share-URL-resolves-now stub: metadata is zeroed (nothing to probe yet) and status is
    // `recording` — commit fills in the real numbers via `promote_recording_stub`.
    let index_json = {
        let (clip_dir, id) = (s.clips_dir.join(&id), id.clone());
        tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            std::fs::create_dir_all(&clip_dir)?;
            let mut index = Index::new(
                &id,
                clipxd_index::Source::Screen,
                clipxd_index::Metadata {
                    duration: 0.0,
                    resolution: [0, 0],
                    fps: 0.0,
                    created_at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs().to_string()).unwrap_or_else(|_| "0".into()),
                    title: "Screen recording".to_string(),
                    description: String::new(),
                    app_focus: Vec::new(),
                    url_context: None,
                    has_video: true,
                },
            );
            index.status = clipxd_index::Status::Recording;
            index.summary.tldr = "Recording in progress — this link is live; the video and index fill in as it happens.".into();
            let j = serde_json::to_string_pretty(&index)?;
            std::fs::write(clip_dir.join("index.json"), &j)?;
            Ok(j)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("stage stub: {e:#}")))?
    };
    // Ownership now, so the canonical /u/<slug>/clip/<id> form resolves during recording.
    if let (Some(a), Some(u)) = (&s.auth, &user) {
        let _ = a.db.set_clip_owner(&id, u.id);
    }
    // Keep hosted-mode S3 in sync so the stub resolves there too — off the response path
    // (an S3 round trip here would undercut "record-to-shareable-link < 1s"). Any reader
    // hitting S3 before this lands still resolves via `read_object_or_local`'s disk fallback.
    tokio::spawn({
        let (s, key) = (s.clone(), format!("{id}/index.json"));
        async move { write_object_best_effort(&s, &key, index_json.into_bytes(), "application/json").await }
    });

    // Owner is already recorded (just above) — decide the BYOK/local-mode caption override
    // once, up front, so every incremental pass for this session uses the same captioner.
    let caption_source = owner_caption_source(&s.auth, &id);
    let indexer = IncrementalIndexer::new(stage_frames_dir(&id), 4.0, caption_source);
    s.stage_sessions.lock().await.insert(id.clone(), Arc::new(StdMutex::new(Some(indexer))));
    Ok(Json(serde_json::json!({ "id": id, "session": id })))
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

/// `POST /ingest/stage/:session/commit` — assemble the uploaded chunks into the final clip.
/// For instant-link sessions (session == `clp_` id, minted at stage-open) Phase 1 is *cheap*:
/// one on-disk concat + a rename into the clip dir + a probe — never the old
/// read-the-whole-video-into-RAM → temp-file → copy round trip, and no synchronous S3 upload.
/// If the session accumulated an [`IncrementalIndexer`], Phase 2 finishes it off (one final
/// pass over the tail) instead of re-enriching the whole clip from scratch.
async fn ingest_stage_commit(
    State(s): State<AppState>,
    Path(session): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, WebErr> {
    let user = require_user(&s, &headers)?;
    if !session.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err((StatusCode::BAD_REQUEST, "invalid session id".into()));
    }
    // Optional commit body: a Browser-mode capture trace. The web recorder sends no body; the
    // extension sends the tab's BrowserTrace here so the finalized clip is both watchable
    // (the streamed video) and queryable (the trace's interaction/DOM events) — merged in
    // spawn_phase2 after enrichment, so there's no race with the index write.
    let browser_trace = (!body.is_empty()).then(|| String::from_utf8_lossy(&body).into_owned());
    let dir = std::env::temp_dir().join(format!("clipxd-stage-{session}"));
    if !dir.exists() {
        return Err((StatusCode::NOT_FOUND, "session not found; call POST /ingest/stage first".into()));
    }

    let stage_slot = s.stage_sessions.lock().await.remove(&session);

    // Legacy `stg_` sessions (older clients): the clip id doesn't exist yet — fall back to the
    // original mint-at-commit path. This path has no instant-link stub to return early against,
    // so it still takes the indexer synchronously (blocking out any in-flight `add_increment`).
    if !session.starts_with("clp_") {
        let incremental = match stage_slot {
            Some(slot) => tokio::task::spawn_blocking(move || slot.lock().ok().and_then(|mut g| g.take())).await.unwrap_or(None),
            None => None,
        };
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
        return ingest_bytes(s, user, Bytes::from(video_bytes), incremental, None).await;
    }

    let id = session;
    // Claim before we touch clip_dir — see the sweeper's identical claim for why: this handler
    // and the abandoned-session sweeper are the two writers that can otherwise race on the same
    // id. A 409 here means the sweeper decided (wrongly, since a live commit is in fact in
    // progress) that this session was abandoned; the client's stage-commit failure path already
    // falls back to `/ingest?reuse=<id>`, which retries the claim once the salvage finishes.
    let claim = try_claim(&s.clip_claims, &id)
        .ok_or((StatusCode::CONFLICT, "clip is already being finalized".into()))?;
    let clip_dir = s.clips_dir.join(&id);

    // Ownership was recorded at stage-open; re-assert in case the session cookie only became
    // available at commit (e.g. a login completed mid-recording).
    if let (Some(a), Some(u)) = (&s.auth, &user) {
        if a.db.clip_owner(&id).ok().flatten().is_none() {
            let _ = a.db.set_clip_owner(&id, u.id);
        }
    }

    // Return the instant-link id NOW — the assemble + index work runs in the background so the
    // client's Stop resolves in milliseconds, not after the last chunk's indexing pass finishes
    // (which used to make commit block for seconds). The `recording` stub already resolves the
    // share URL; the clip page polls recording → enriching → complete and the video appears the
    // moment `assemble_recording` lands it. Order matters: assemble the video FIRST (fast: concat
    // + probe) so `/clip/:id/video` resolves quickly, THEN take the accumulated indexer (which
    // waits out any in-flight `add_increment` still reading the stage dir), THEN drop the stage
    // dir, THEN run Phase 2.
    let s2 = s.clone();
    let dir2 = dir.clone();
    let clip_dir2 = clip_dir.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        let video = match tokio::task::spawn_blocking({
            let (dir, clip_dir, id) = (dir2.clone(), clip_dir2.clone(), task_id.clone());
            move || assemble_recording(&dir, &clip_dir, &id)
        })
        .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => { eprintln!("commit assemble for {task_id}: {e:#}"); mark_partial(&clip_dir2); return; }
            Err(e) => { eprintln!("commit assemble task for {task_id}: {e}"); mark_partial(&clip_dir2); return; }
        };
        let incremental = match stage_slot {
            Some(slot) => tokio::task::spawn_blocking(move || slot.lock().ok().and_then(|mut g| g.take())).await.unwrap_or(None),
            None => None,
        };
        let _ = tokio::fs::remove_dir_all(&dir2).await;
        spawn_phase2(&s2, task_id, clip_dir2, video, incremental, claim, browser_trace);
    });
    Ok(Json(serde_json::json!({ "id": id })))
}

/// Assemble the committed video from a stage session's chunks and promote the `recording` stub
/// to `enriching` with real probe metadata — WITHOUT deleting the stage dir. Splitting the
/// delete out lets the instant-link commit path land the playable video first (so `/clip/:id/
/// video` resolves fast) and only drop the stage dir afterwards, once the accumulated indexer
/// has been taken (an in-flight `add_increment` may still be reading `video-so-far.webm`).
/// `concat_chunks` reads the `chunk-*.bin` files (never written after Stop) and `move_file`
/// only touches the fresh `commit.webm`, so this is safe to run alongside a final in-flight pass.
fn assemble_recording(stage_dir: &std::path::Path, clip_dir: &std::path::Path, id: &str) -> anyhow::Result<PathBuf> {
    let out = stage_dir.join("commit.webm");
    concat_chunks(stage_dir, &out)?;
    std::fs::create_dir_all(clip_dir)?;
    let video = clip_dir.join("video.webm");
    move_file(&out, &video)?;
    clipxd_recorder::promote_recording_stub(clip_dir, &video, id, "Screen recording")?;
    Ok(video)
}

/// Blocking Phase-1 of the sweeper's salvage path: [`assemble_recording`] plus dropping the
/// stage dir. (The live commit path assembles and drops separately — see `ingest_stage_commit`.)
fn promote_staged(stage_dir: &std::path::Path, clip_dir: &std::path::Path, id: &str) -> anyhow::Result<PathBuf> {
    let video = assemble_recording(stage_dir, clip_dir, id)?;
    let _ = std::fs::remove_dir_all(stage_dir);
    Ok(video)
}

/// `rename` when possible (same filesystem — instant), else copy + remove (temp dir on tmpfs,
/// clips dir on disk).
fn move_file(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dst)?;
    let _ = std::fs::remove_file(src);
    Ok(())
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
    let caption_source = owner_caption_source(&s.auth, &id);
    tokio::spawn(async move {
        if let Err(e) = clipxd_recorder::enrich_clip(&video, &bg_dir, &bg_id, &title, &events, 4.0, caption_source) {
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

/// One client-submitted caption in a `POST /clip/:id/local-captions` body.
#[derive(serde::Deserialize)]
struct LocalCaptionIn {
    /// Seconds into the clip.
    t: f64,
    caption: String,
}

#[derive(serde::Deserialize)]
struct LocalCaptionsBody {
    captions: Vec<LocalCaptionIn>,
}

/// `POST /clip/:id/local-captions` — accept client-submitted captions (e.g. from a
/// fully-client-side/WebGPU captioner, for a clip owner whose `caption_mode` is `local` and so
/// never got server-side VLM captions) and graft them into the clip's `visual_timeline`. Mirrors
/// `merge_browser_trace_into_clip`'s exact pattern: read `index.json`, extend the stream, re-run
/// `clean_index` (dedup + rebuild the agent search corpus), write back — just over
/// `visual_timeline` alone rather than a whole browser trace's several streams. Login +
/// ownership required, same `require_clip_access` gate as `clip_re_enrich`/`set_cursor`.
async fn post_local_captions(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<LocalCaptionsBody>,
) -> Result<Json<serde_json::Value>, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    require_clip_access(&s, &headers, &id)?;
    if body.captions.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no captions supplied".into()));
    }
    if body.captions.len() > 500 {
        return Err((StatusCode::BAD_REQUEST, "too many captions (500 max per call)".into()));
    }

    let mut index = load_index(&s, &id).await?;
    let mut added = 0usize;
    for c in &body.captions {
        if !c.t.is_finite() {
            return Err((StatusCode::BAD_REQUEST, "caption 't' must be a finite number".into()));
        }
        let text = c.caption.trim();
        if text.is_empty() {
            continue; // skip blanks rather than fail the whole batch
        }
        index.visual_timeline.push(clipxd_index::VisualMoment {
            t: c.t.max(0.0),
            // Trusted, explicit client input — high salience so `clean_index`'s MAX_MOMENTS
            // trim keeps these over lower-confidence auto-generated moments.
            salience: 1.0,
            caption: text.chars().take(2000).collect(),
            delta: "local_caption".to_string(),
            frame_ref: None,
        });
        added += 1;
    }
    if added == 0 {
        return Err((StatusCode::BAD_REQUEST, "no non-empty captions supplied".into()));
    }

    clipxd_index::clean_index(&mut index); // dedup streams + rebuild search corpus
    let dir = s.clips_dir.join(&id);
    let index_json = serde_json::to_string_pretty(&index).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::write(dir.join("index.json"), &index_json).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    write_object_best_effort(&s, &format!("{id}/index.json"), index_json.into_bytes(), "application/json").await;

    Ok(Json(serde_json::json!({ "ok": true, "added": added, "visual_timeline": index.visual_timeline.len() })))
}

/// Body of a `POST /clip/:id/subtitle-style` — the user's caption design choice. All fields
/// validated against small allow-lists so a bad client can't write arbitrary strings into the
/// index. `font_scale` is clamped to a sensible range.
#[derive(Deserialize)]
struct SubtitleStyleBody {
    design: String,
    #[serde(default = "one_f32")]
    font_scale: f32,
    #[serde(default = "pos_bottom_str")]
    position: String,
    #[serde(default = "default_true_fn")]
    emphasis: bool,
}

fn one_f32() -> f32 { 1.0 }
fn pos_bottom_str() -> String { "bottom".to_string() }
fn default_true_fn() -> bool { true }

const SUBTITLE_DESIGNS: &[&str] = &["classic", "bold", "karaoke", "minimal", "boxed", "glow"];
const SUBTITLE_POSITIONS: &[&str] = &["bottom", "center", "top"];

/// `POST /clip/:id/subtitle-style` — save the user's chosen caption design + knobs into the
/// clip's `index.json` as `subtitle_style`. Pure presentation metadata; ownership-gated, same
/// `require_clip_access` gate as `set_cursor`/`local-captions`. The emphasis data the designs
/// consume (`subtitle_emphasis`) is server-produced at indexing time and never mutated here.
async fn set_subtitle_style(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SubtitleStyleBody>,
) -> Result<Json<serde_json::Value>, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    require_clip_access(&s, &headers, &id)?;
    let design = body.design.trim().to_ascii_lowercase();
    if !SUBTITLE_DESIGNS.contains(&design.as_str()) {
        return Err((StatusCode::BAD_REQUEST, format!("unknown design '{}'", body.design)));
    }
    let position = body.position.trim().to_ascii_lowercase();
    if !SUBTITLE_POSITIONS.contains(&position.as_str()) {
        return Err((StatusCode::BAD_REQUEST, format!("unknown position '{}'", body.position)));
    }
    let font_scale = body.font_scale.clamp(0.8, 1.6);

    let mut index = load_index(&s, &id).await?;
    index.subtitle_style = Some(clipxd_index::SubtitleStyle {
        design,
        font_scale,
        position,
        emphasis: body.emphasis,
    });
    let dir = s.clips_dir.join(&id);
    let index_json = serde_json::to_string_pretty(&index).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::write(dir.join("index.json"), &index_json).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    write_object_best_effort(&s, &format!("{id}/index.json"), index_json.into_bytes(), "application/json").await;

    Ok(Json(serde_json::json!({ "ok": true, "subtitle_style": index.subtitle_style })))
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

    // Write the raw bytes to a temp file, then run stub_clip in a worker thread. The clip id
    // is minted up front so the foreground can return it without waiting.
    let dir = s.clips_dir.clone();
    let body = body.clone();
    let owner_email = s.auth.as_ref().and_then(|_| headers.get("x-clipxd-owner-email").and_then(|v| v.to_str().ok()).map(String::from));
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let id = mint_clip_id();
    // Claimed for the whole finalization (Phase 1 stub through Phase 2 completion) — same
    // protection the staged-upload/reuse paths get, so a sweeper pass or a retried tunnel
    // POST for this id can never write clip_dir concurrently with this request.
    let claim = try_claim(&s.clip_claims, &id)
        .ok_or((StatusCode::CONFLICT, "clip is already being finalized".into()))?;
    let tmp = std::env::temp_dir().join(format!("clipxd-tunnel-{stamp}.{ext}"));
    let tmp_clone = tmp.clone();
    let dir_thread = dir.clone();
    let id_thread = id.clone();

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

    let db = s.auth.as_ref().map(|a| a.db.clone());
    let ext_clone = ext.clone();

    // Phase 1b: stub_clip (fast, ~1s) + ownership binding, on a blocking thread.
    let stub_result = tokio::task::spawn_blocking(move || -> anyhow::Result<std::path::PathBuf> {
        let clip_dir = clipxd_recorder::stub_clip(&tmp_clone, dir_thread.as_path(), &id_thread, "Imported via tunnel")?;
        if let (Some(db), Some(email)) = (db.as_ref(), owner_email.as_ref()) {
            if let Ok(Some(u)) = db.find_by_email(email) {
                let _ = db.set_clip_owner(&id_thread, u.id);
            }
        }
        let _ = std::fs::remove_file(&tmp_clone);
        // stub_clip preserves the source's own extension (video.mp4/video.webm/...), not
        // always .webm — a prior hardcoded ".webm" here meant enrich_clip was pointed at a
        // file that didn't exist for any non-webm tunneled source (most yt-dlp downloads are
        // mp4), silently landing every such clip at status: partial with zero enrichment.
        Ok(clip_dir.join(format!("video.{ext_clone}")))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let video = match stub_result {
        Ok(v) => v,
        Err(e) => {
            eprintln!("tunneled stub_clip failed for {id}: {e:#}");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("tunneled ingest failed: {e:#}")));
        }
    };

    // Phase 2 (background): mirror stub → enrich (full clipxd_recorder::enrich_clip, since
    // there's no partial-file streaming protocol to index incrementally against here — the
    // forwarder only POSTs once yt-dlp's download is complete) → deep pass → re-mirror. This
    // is the same path /ingest and staged commits use, so it already carries every enrich_clip
    // speedup (oar-ocr, phash dedup, concurrent captions, parallel transcript) automatically.
    spawn_phase2(&s, id.clone(), dir.join(&id), video, None, claim, None);

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

/// `POST /ingest/browser-trace` — accept a Browser-mode capture (a `BrowserTrace` JSON:
/// clicks, DOM snapshots/mutations, console, network, navigation, input, scroll) and turn it
/// into a clipxd index via the clean-room [`clipxd_browser`] ingester, returning the new clip
/// id. This is the hosted counterpart of the CLI's `clipxd ingest-browser`, and the missing
/// server half of Browser mode: Screen recording can only see input while the pointer is over
/// the clipxd tab (a `getDisplayMedia` limit), so a recording of *another* page/app has no
/// interaction track. A trace captured in that page — by the extension — does, and this is
/// where it lands. Body is the raw trace JSON (login required; the clip is owned by the caller).
async fn ingest_browser_trace(State(s): State<AppState>, headers: HeaderMap, body: Bytes) -> Result<Json<serde_json::Value>, WebErr> {
    let user = require_user(&s, &headers)?;
    let json = String::from_utf8(body.to_vec()).map_err(|_| (StatusCode::BAD_REQUEST, "trace body must be UTF-8 JSON".into()))?;
    if json.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty trace".into()));
    }
    let id = mint_clip_id();
    let clips_dir = s.clips_dir.clone();
    let id_for_job = id.clone();
    // Salience derivation + index build is pure CPU over the (possibly large) trace — off the
    // async runtime.
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let created_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs().to_string()).unwrap_or_else(|_| "0".into());
        let mut index = clipxd_browser::ingest_str(&json, &id_for_job, &created_at, &clipxd_browser::SalienceOpts::default())?;
        // Build the flat search corpus (incl. the event stream) so a browser clip is as
        // agent-greppable as a screen clip — the ingester doesn't run this itself.
        clipxd_index::clean_index(&mut index);
        let clip_dir = clips_dir.join(&id_for_job);
        std::fs::create_dir_all(&clip_dir)?;
        std::fs::write(clip_dir.join("index.json"), serde_json::to_string_pretty(&index)?)?;
        Ok(())
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::BAD_REQUEST, format!("could not ingest trace: {e:#}")))?;

    if let (Some(a), Some(u)) = (&s.auth, &user) {
        let _ = a.db.set_clip_owner(&id, u.id);
    }
    let clip_dir = s.clips_dir.join(&id);
    if let Ok(st) = s.storage.make_storage().await {
        if let Err(e) = mirror_dir_to_storage(st.as_ref(), &id, &clip_dir).await {
            eprintln!("browser-trace post-mirror: {e}");
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
    /// Burn the clip's styled subtitles (subtitle_style + subtitle_emphasis) into the render.
    captions: Option<bool>,
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
        // Burn styled captions: pass the clip's index.json; `beautify` no-ops when the user
        // hasn't picked a design (no subtitle_style) or there's no transcript, so passing it
        // is always safe. Gated by the `captions` query param (default off for back-compat).
        if p.captions.unwrap_or(false) && dir.join("index.json").exists() {
            c.arg("--captions").arg(dir.join("index.json"));
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
    // If the owner has a username, redirect to the branded /u/<username>/<slug>-<short>
    // form. We do the redirect on the bare /clip/:id hit (instead of from inside the
    // share-page body) so the visitor's address bar shows the branded URL on the very
    // first paint, and so any share-out copies the branded URL automatically.
    if let Some(a) = &s.auth {
        if let Some(owner_id) = a.db.clip_owner(&id).ok().flatten() {
            if let Ok(Some(u)) = a.db.find_by_id(owner_id) {
                if let Some(slug) = u.username.as_deref() {
                    let tail = share_slug_for(&idx.metadata.title, &id);
                    let target = format!("/u/{slug}/{tail}");
                    return Ok(redirect_to(&headers, &target));
                }
            }
        }
    }
    let views = bump_view_count(&s, &id).await;
    share_page_body(&s, &id, &idx, &headers, views, None)
}

/// Resolve /u/:username/clip/:id: ensure the clip is owned by that username, then render.
async fn share_page_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Html<String>, WebErr> {
    let _ = check_owner(&s, &username, &id)?;
    let idx = load_index(&s, &id).await?;
    let views = bump_view_count(&s, &id).await;
    share_page_body(&s, &id, &idx, &headers, views, Some(&username))
}

/// Branded share-link entry: /u/:username/<title-slug>-<short>.  The slug is just
/// cosmetic — we split off the short tail (last dash-separated alphanumeric token),
/// look up which of `username`'s clips ends with it, and render the same share page
/// as the canonical `/u/:username/clip/:id` form.  Mismatched / ambiguous slugs
/// resolve to 404 so guessing tails is a dead end.
async fn share_page_for_slug(
    State(s): State<AppState>,
    Path((username, slug)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Html<String>, WebErr> {
    let auth = s.auth.as_ref().ok_or((StatusCode::NOT_FOUND, "not found".into()))?;
    let id = resolve_share_slug(&auth.db, &username, &slug)
        .ok_or((StatusCode::NOT_FOUND, "not found".into()))?;
    let idx = load_index(&s, &id).await?;
    let views = bump_view_count(&s, &id).await;
    share_page_body(&s, &id, &idx, &headers, views, Some(&username))
}

/// `POST /clip/:id/view` — the SPA's ClipPage hits this once on mount (the server-rendered
/// share page above counts its own views directly, without a round trip). Returns the new
/// total so the caller can display it without a second request.
async fn post_view(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, WebErr> {
    if !safe(&id) {
        return Err((StatusCode::BAD_REQUEST, "bad id".into()));
    }
    load_index(&s, &id).await?; // 404s cleanly if the clip doesn't exist, instead of counting a view for it
    let views = bump_view_count(&s, &id).await;
    Ok(Json(serde_json::json!({ "views": views })))
}

async fn post_view_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, WebErr> {
    check_owner(&s, &username, &id)?;
    post_view(State(s), Path(id)).await
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

async fn get_preview_gif_for_user(
    State(s): State<AppState>,
    Path((username, id)): Path<(String, String)>,
) -> Result<impl IntoResponse, WebErr> {
    check_owner(&s, &username, &id)?;
    get_preview_gif(State(s), Path(id)).await
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
    views: u64,
    username: Option<&str>,
) -> Result<Html<String>, WebErr> {
    // Absolute URL of THIS page, for the "scan to open on your phone" QR and every
    // "Copy link"/"Copy embed"/"Copy agent link" button: must match whatever's actually in the
    // visitor's address bar (the canonical /u/<username>/... form when the clip is owned by a
    // user with a slug), or copying the link hands out a *different* URL than the one the
    // visitor is looking at — confusingly inconsistent even though the other form still
    // resolves (it 30x's back here). Prefer the public tunnel origin if one is configured,
    // else reconstruct from the request Host.
    let base = s.public_base.as_ref().map(|b| b.to_string()).unwrap_or_else(|| {
        let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("localhost");
        let scheme = headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("https");
        format!("{scheme}://{host}")
    });
    let url = match username {
        Some(slug) => {
            // Branded URL: /u/<username>/<title-slug>-<short>. The short tail is what
            // disambiguates same-titled clips owned by the same user; the title slug
            // is what makes the URL readable. The full clip id is still the access
            // control (resolved server-side from the short).
            let tail = share_slug_for(&idx.metadata.title, id);
            format!("{base}/u/{slug}/{tail}")
        }
        None => format!("{base}/clip/{id}"),
    };
    Ok(Html(share_html(&id, &idx, &url, views)))
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
                    // Backwards-compat alias — older SPA builds look at `user_share_base`
                    // and expect the `/clip/<id>` suffix to be appended client-side. Keep
                    // that working alongside the new branded base.
                    body_mut.insert("user_share_base".into(), serde_json::Value::String(format!("{ubase}/u/{slug}/clip")));
                    // New branded share base: client appends `share_slug_for(title, id)`
                    // instead of a raw id, so the resulting URL reads as a title rather
                    // than a token. The slug computation lives on the server so client
                    // and server stay byte-identical (and so the SPA doesn't have to
                    // ship a slugify implementation).
                    body_mut.insert("user_slug_share_base".into(), serde_json::Value::String(format!("{ubase}/u/{slug}")));
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

/// Turn a free-form clip title into a URL-safe slug.  Used only for *display* in the
/// branded share URL — the real access control is the short tail of the clip id (see
/// `share_slug_for`).  We keep the slug short (≤ ~60 chars) and lowercase, collapse
/// runs of separators, and drop any non-ASCII or non-alphanumeric runs entirely so
/// the resulting path is safe for any URL consumer (email clients, social previews,
/// QR scanners).
pub fn slugify_title(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_dash = true; // suppress leading dash
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') { out.pop(); }
    if out.len() > 60 { out.truncate(60); while out.ends_with('-') { out.pop(); } }
    out
}

/// Last 4 chars of the clip id — the disambiguating tail that lives at the end of the
/// branded share URL.  Strips the `clp_` prefix so the tail is the visible fingerprint
/// rather than a redundant acronym, and falls back to whatever follows if the id
/// doesn't match the expected prefix.
pub fn short_tail(id: &str) -> String {
    let bare = id.strip_prefix("clp_").unwrap_or(id);
    // Walk back from the end up to 4 chars, stopping at the first non-alphanumeric so
    // the tail is always URL-safe even on ids that have odd suffixes.
    let take = bare
        .chars()
        .rev()
        .take(4)
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    bare[bare.len() - take..].to_lowercase()
}

/// Build the `<title-slug>-<short>` segment that goes between `/u/<username>/` and
/// the resolution step.  Always non-empty: if the title slugifies to nothing (e.g. a
/// clip whose title is just an emoji) we fall back to `clip-<short>` so the URL still
/// reads as a share link.
pub fn share_slug_for(title: &str, id: &str) -> String {
    let s = slugify_title(title);
    let t = short_tail(id);
    if t.is_empty() {
        if s.is_empty() { "clip".into() } else { s }
    } else if s.is_empty() {
        format!("clip-{t}")
    } else {
        format!("{s}-{t}")
    }
}

/// Parse the `<title-slug>-<short>` segment back into the clip id.  Inverse of
/// `share_slug_for` — takes the last dash-separated token as the short tail, looks up
/// which of `username`'s clips ends with it.  Returns the resolved clip id when
/// exactly one match exists; `None` for 0 or 2+ matches (so guessing tails never
/// leaks the secret id).
pub fn resolve_share_slug(db: &crate::auth::Db, username: &str, slug: &str) -> Option<String> {
    let short = slug.rsplit('-').next().unwrap_or("");
    if short.len() < 3 || !short.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let user = db.find_by_username(username).ok().flatten()?;
    db.find_clip_by_short(user.id, short).ok().flatten()
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
fn share_html(id: &str, idx: &Index, url: &str, views: u64) -> String {
    let title = html_escape(&idx.metadata.title);
    let qr = qr_svg(url);
    // Prefer the deep pass's real tl;dr (content-aware — "what happens in this recording")
    // over the generic stream-count line, so a Slack/Notion/Twitter unfurl actually describes
    // the clip instead of just proving it was indexed.
    let og_desc = if !idx.summary.tldr.trim().is_empty() {
        html_escape(idx.summary.tldr.trim())
    } else {
        format!(
            "Watch \"{}\" on clipxd. {} on-screen text spans, {} event(s). Indexed and agent-queryable.",
            title, idx.on_screen_text.len(), idx.event_track.len()
        )
    };
    let dur = idx.metadata.duration;

    let main = share_main(id, idx);
    let aside = share_aside(id, idx, url, &qr);

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <!-- Match whatever theme the visitor picked in the app (same origin, so this localStorage
       key is shared) before first paint — falls through to the CSS's own
       prefers-color-scheme media query if they never touched the toggle. -->
  <script>try {{ var t = localStorage.getItem('clipxd:theme'); if (t === 'light' || t === 'dark') document.documentElement.setAttribute('data-theme', t); }} catch (e) {{}}</script>
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{title} — clipxd</title>
  <meta name="description" content="{og_desc}" />
  <link rel="canonical" href="{url}" />
  <meta property="og:type" content="video.other" />
  <meta property="og:title" content="{title}" />
  <meta property="og:description" content="{og_desc}" />
  <meta property="og:url" content="{url}" />
  <meta property="og:image" content="{url}/thumbnail" />
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
  <!-- Umami analytics (privacy-friendly, self-hosted) -->
  <script defer src="https://analytics.rohan.sh/script.js" data-website-id="f0180529-adb3-4603-b5fa-8bc5bd19b4d5"></script>
</head>
<body>
  {topbar}
  <main class="hero">
    <h1 class="title">{title}</h1>
    <div class="meta-row">
      <span class="pill">{src_dot} Screen recording</span>
      <span class="pill sodium">{dur_lbl}</span>
      <span class="pill status-pill">{status}</span>
      <span class="pill ghost" title="views">{views_lbl} view{views_plural}</span>
      <a class="pill ghost" href="/clip/{id}/index.json" target="_blank">index.json</a>
    </div>
    <div class="player">
      <video src="/clip/{id}/video" controls poster="{url}/thumbnail" preload="metadata" playsinline></video>
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
        css       = share_css(),
        js        = SHARE_JS,
        topbar    = share_topbar(&url),
        src_dot   = r#"<span class="dot sodium"></span>"#,
        dur_lbl   = fmt_duration(dur),
        status    = share_status_pill(&idx.status),
        views_lbl = format_view_count(views),
        views_plural = if views == 1 { "" } else { "s" },
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

/// "1", "999", "1.2k", "45k" — matches the compact convention viewers already know from
/// every other video platform, so it reads as a view count at a glance rather than a raw int.
fn format_view_count(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{:.1}k", n as f64 / 1000.0).replace(".0k", "k")
    }
}

/// "indexed" / "still indexing" / "partial" / "no index".  Visual tone follows
/// the SPA: signal for indexed, sodium for indexing/partial, danger for failures.
fn share_status_pill(s: &clipxd_index::Status) -> &'static str {
    use clipxd_index::Status::*;
    match s {
        Complete    => r#"<span class="pill signal">indexed</span>"#,
        Enriching   => r#"<span class="pill sodium">indexing…</span>"#,
        Partial     => r#"<span class="pill sodium">partial — captions empty</span>"#,
        Recording   => r#"<span class="pill sodium">recording…</span>"#,
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
        <linearGradient id="lb-face" x1="0.2" y1="0" x2="0.7" y2="1"><stop offset="0" stop-color="#FFFFFF"/><stop offset="0.55" stop-color="#FBFBF9"/><stop offset="1" stop-color="#EFEEE9"/></linearGradient>
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

    // Outline — a Fathom-style "moments at a glance" that reads as a story arc for the
    // clip rather than a flat dump of OCR lines.  Each moment has a real frame
    // thumbnail, its caption, the timestamp, and a click-to-seek target.  The current
    // moment is highlighted live (see the JS that follows).  Always shown — even an
    // empty timeline falls back to a single "no moments captured yet" row so the
    // layout doesn't shift between indexed and not-yet-indexed clips.
    let mut h = String::from(r#"<section class="card outline"><h3>Outline</h3><ol class="outline-list" id="outlineList">"#);
    if idx.visual_timeline.is_empty() {
        h.push_str(r#"<li class="outline-empty">No moments captured yet — the clip is still being indexed.</li>"#);
    } else {
        for (i, m) in idx.visual_timeline.iter().enumerate() {
            let cap = html_escape(&m.caption);
            // Trim very long captions to keep the surface tidy.
            let cap_short = if cap.chars().count() > 180 {
                let cut: String = cap.chars().take(180).collect();
                format!("{cut}…")
            } else { cap };
            // The thumbnail (frame_ref → /clip/:id/frames/<name>) gives the outline its
            // visual rhythm; a missing frame falls back to a neutral tile so the row
            // doesn't break the grid.
            let thumb_html = match m.frame_ref.as_deref() {
                Some(fr) => {
                    let bare = fr.strip_prefix("frames/").unwrap_or(fr);
                    let url = format!("/clip/{id}/frames/{bare}");
                    format!(r##"<img class="outline-thumb" src="{url}" alt="" loading="lazy" />"##)
                }
                None => r#"<span class="outline-thumb outline-thumb-empty" aria-hidden></span>"#.to_string(),
            };
            h.push_str(&format!(
                r##"<li class="outline-row" data-t="{t}" id="outline-{i}"><a href="#t={t}" class="outline-link">{thumb}<span class="outline-ts">{ts}</span><span class="outline-cap">{cap}</span></a></li>"##,
                t = m.t,
                ts = fmt_duration(m.t),
                cap = cap_short,
                thumb = thumb_html,
                i = i,
            ));
        }
    }
    h.push_str("</ol></section>");
    s.push_str(&h);

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

    // Discussion — the public face of the Fathom-style @-mention chat. The thread is
    // rendered client-side (see SHARE_JS) from GET /clip/:id/comments so it stays live; the
    // compose box POSTs to the same endpoint (login-gated — a logged-in viewer on this
    // same-origin page has the session cookie; logged-out gets a "log in to comment" nudge).
    s.push_str(&format!(
        r##"<section class="card discussion" data-cmt-url="/clip/{id}/comments">
  <h3>Discussion</h3>
  <div class="cmt-thread" id="cmtThread"><div class="cmt-loading">Loading the conversation…</div></div>
  <div class="cmt-form">
    <textarea id="cmtInput" rows="2" placeholder="Comment on the moment you're watching…  (type @0:12 to link a time)"></textarea>
    <div class="cmt-form-row">
      <button type="button" id="cmtAt" class="cmt-at">@ this moment</button>
      <span class="cmt-status" id="cmtStatus" aria-live="polite"></span>
      <button type="button" id="cmtPost" class="cmt-post-btn">Comment</button>
    </div>
  </div>
</section>"##,
        id = id,
    ));

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
    let title = html_escape(&idx.metadata.title);
    let agent_md_url = format!("{url}/agent.md");
    let gif_url = format!("{url}/preview.gif");
    let embed = format!(
        r#"<iframe src="{url}" width="960" height="600" frameborder="0" allow="autoplay; fullscreen" title="clipxd clip"></iframe>"#,
        url = url,
    );
    // Loom's "copy a hyperlinked animated thumbnail" pattern — the pasted <a><img></a> plays
    // inline in email clients (which render animated GIFs, unlike most og:image consumers).
    let gif_embed = format!(r#"<a href="{url}"><img src="{gif_url}" alt="{title} — clipxd" width="480" /></a>"#);

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
  <button class="share-btn" data-copy="{gif_embed}" type="button">
    <span class="share-lbl">Copy GIF</span>
    <span class="share-hint">for email — plays inline</span>
  </button>
  <button class="share-btn" data-copy="{embed}" type="button">
    <span class="share-lbl">Copy embed</span>
    <span class="share-hint">&lt;iframe …&gt;</span>
  </button>
  <button class="share-btn" data-copy="{agent_md_url}" type="button">
    <span class="share-lbl">Copy agent link</span>
    <span class="share-hint">paste into any agent</span>
  </button>
  <a class="share-btn" href="/clip/{id}/index.json" target="_blank" rel="noopener">
    <span class="share-lbl">Download index.json</span>
    <span class="share-hint">.json · sidecar</span>
  </a>
</div>

<div class="share-card">
  <div class="share-head">
    <span class="share-dot"></span>
    <b>Generate a doc</b>
  </div>
  <a class="share-btn" href="/clip/{id}/doc/pr-description" target="_blank" rel="noopener">
    <span class="share-lbl">PR description</span>
    <span class="share-hint">summary + test plan</span>
  </a>
  <a class="share-btn" href="/clip/{id}/doc/sop" target="_blank" rel="noopener">
    <span class="share-lbl">SOP</span>
    <span class="share-hint">numbered repro steps</span>
  </a>
  <a class="share-btn" href="/clip/{id}/doc/qa-steps" target="_blank" rel="noopener">
    <span class="share-lbl">QA steps</span>
    <span class="share-hint">test checklist</span>
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
        agent_md_url = agent_md_url,
        gif_embed  = html_escape(&gif_embed),
        // Attribute-escape the iframe string (it contains `"`), or its quotes break out of
        // `data-copy="…"` and leak `" type="button">` as visible text. `getAttribute` decodes
        // the entities back, so the copied value is still the raw iframe HTML. (gif_embed above
        // was already escaped; embed was the one that slipped through.)
        embed      = html_escape(&embed),
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
///  CSS — puffy clay system, light + dark via prefers-color-scheme, or an explicit
///  `data-theme` override (see the inline theme-sync script in `share_html`) so a visitor
///  who picked a theme in the app sees the same one here regardless of OS preference.
///  Inlined so the share page is self-contained (no external CSS).
/// ============================================================================
const SHARE_CSS_VARS_DARK: &str = r##"
    --bg:#131315; --panel:#1E1E20; --panel-2:#262628; --panel-3:#303032;
    --glass: rgba(46,46,50,.46);
    --border: rgba(255,255,255,.10); --border-2: rgba(255,255,255,.2);
    --text:#F2F1EE; --text-2:#C2C0BA; --text-3:#8B8982;
    --c-grape:#93ABDD;
    --on-accent:#131315;
    --sodium-wash:rgba(255,122,89,.16);
    --signal-wash:rgba(22,199,154,.20);
    --sodium-text:#FFAD90; --signal-text:#5FE7C2;
    --env:
      radial-gradient(70% 50% at 50% -10%, rgba(255,255,255,.026), transparent 72%);
    --clay: 0 18px 34px -14px rgba(0,0,0,.7), inset 0 2px 1px rgba(255,255,255,.14), inset 0 -9px 18px -7px rgba(0,0,0,.5);
    --clay-sm: 0 11px 22px -12px rgba(0,0,0,.66), inset 0 2px 1px rgba(255,255,255,.12), inset 0 -6px 12px -6px rgba(0,0,0,.45);
    --clay-in: inset 0 3px 8px rgba(0,0,0,.55), inset 0 -1px 1px rgba(255,255,255,.08);
    --pop-signal: 0 16px 30px -12px rgba(22,199,154,.5), inset 0 2px 1px rgba(255,255,255,.4), inset 0 -8px 16px -6px rgba(0,70,52,.5);
    --pop-sodium: 0 14px 26px -12px rgba(214,70,31,.5), inset 0 2px 1px rgba(255,255,255,.55), inset 0 -7px 14px -6px rgba(150,40,16,.4);
    --shadow-float: 0 12px 30px -16px rgba(0,0,0,.66);
"##;

/// Builds the full share-page stylesheet. Uses plain `.replace()` on placeholder tokens
/// rather than `format!()`: the CSS that follows is full of literal `{`/`}` braces, which
/// `format!()` would require escaping as `{{`/`}}` throughout — far riskier to touch than
/// substituting two markers into an otherwise-untouched template.
fn share_css() -> String {
    SHARE_CSS_TEMPLATE.replace("/*__DARK_VARS_MEDIA__*/", SHARE_CSS_VARS_DARK).replace("/*__DARK_VARS_EXPLICIT__*/", SHARE_CSS_VARS_DARK)
}

const SHARE_CSS_TEMPLATE: &str = r##"
:root {
  --c-sodium:#FF7A59;  --c-signal:#16C79A;  --c-grape:#5E7BB0;
  --ease-clip: cubic-bezier(.34, 1.56, .42, 1);
  --r: 22px; --r-sm: 14px; --r-pill: 999px;
}
/* Light (warm-paper neutral, no purple glow) */
:root, :root[data-theme=light] {
  --bg:#F1F0EC; --panel:#FCFBF9; --panel-2:#F0EFEB; --panel-3:#E7E6E1;
  --glass: rgba(255,255,255,.5);
  --border: rgba(38,36,32,.11); --border-2: rgba(38,36,32,.20);
  --c-grape:#4E6DA8;
  --text:#1D1B17; --text-2:#5A574F; --text-3:#6A675E;
  --on-accent:#FFFFFF;
  --sodium-wash:rgba(255,122,89,.13);
  --signal-wash:rgba(22,199,154,.13);
  --sodium-text:#A22F18; --signal-text:#08513E;
  --env:
    radial-gradient(70% 50% at 50% -10%, rgba(0,0,0,.022), transparent 72%);
  --clay: 0 16px 30px -14px rgba(50,46,40,.28), inset 0 2px 1px rgba(255,255,255,.95), inset 0 -8px 16px -6px rgba(90,84,74,.14);
  --clay-sm: 0 9px 18px -10px rgba(50,46,40,.24), inset 0 2px 1px rgba(255,255,255,.9), inset 0 -5px 10px -5px rgba(90,84,74,.12);
  --clay-in: inset 0 3px 7px rgba(70,64,54,.18), inset 0 -2px 2px rgba(255,255,255,.7);
  --pop-signal: 0 14px 26px -12px rgba(12,142,108,.5), inset 0 2px 1px rgba(255,255,255,.55), inset 0 -7px 14px -6px rgba(8,90,68,.4);
  --pop-sodium: 0 14px 26px -12px rgba(214,70,31,.5), inset 0 2px 1px rgba(255,255,255,.55), inset 0 -7px 14px -6px rgba(150,40,16,.4);
  --shadow-float: 0 12px 30px -16px rgba(50,46,40,.34);
}
/* OS-level dark preference — unless the user explicitly picked light in the app. */
@media (prefers-color-scheme: dark) {
  :root:not([data-theme=light]) {
/*__DARK_VARS_MEDIA__*/
  }
}
/* Explicit dark override (from the app's theme toggle) — applies regardless of OS preference. */
:root[data-theme=dark] {
/*__DARK_VARS_EXPLICIT__*/
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

/* Outline — Fathom-style "moments at a glance" that replaces the previous chapters +
   moments split.  Each row is a thumbnail + timestamp + caption, the current row is
   highlighted live, and the timeline progress is shown as a thin rail. */
.outline-list { list-style: none; display: flex; flex-direction: column; gap: 4px; }
.outline-row { position: relative; }
.outline-link {
  display: grid; grid-template-columns: 88px 56px 1fr; gap: 12px; align-items: center;
  padding: 8px 10px; border-radius: var(--r-sm); color: var(--text);
  text-decoration: none; border: 1px solid transparent; transition: background .14s, border-color .14s;
}
.outline-link:hover { background: var(--panel-2); text-decoration: none; border-color: var(--border); }
.outline-row.is-current .outline-link {
  background: var(--signal-wash); border-color: color-mix(in oklab, var(--c-signal) 35%, transparent);
}
.outline-row.is-current .outline-ts { color: var(--signal-text); }
.outline-thumb {
  width: 88px; height: 50px; object-fit: cover; border-radius: 8px;
  background: var(--panel-3); border: 1px solid var(--border); flex: none;
}
.outline-thumb-empty { display: inline-block; background: var(--panel-3); }
.outline-ts { font: 500 12px var(--font-mono, "JetBrains Mono", monospace); color: var(--sodium-text); flex: none; text-align: right; }
.outline-cap { font-size: 13.5px; color: var(--text-2); line-height: 1.4; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.outline-empty { color: var(--text-3); font-size: 13px; padding: 12px 4px; }
@media (max-width: 600px) {
  .outline-link { grid-template-columns: 64px 48px 1fr; }
  .outline-thumb { width: 64px; height: 38px; }
  .outline-cap { font-size: 13px; }
}

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

/* discussion (comments) */
.cmt-thread { display: flex; flex-direction: column; margin-bottom: 14px; }
.cmt-loading { font-size: 13px; color: var(--text-3); padding: 6px 0; }
.cmt-row { display: flex; gap: 12px; padding: 9px 0; border-bottom: 1px solid var(--border); align-items: flex-start; }
.cmt-ts { font: 500 11.5px var(--font-mono, "JetBrains Mono", monospace); color: var(--signal-text); flex: none; width: 42px; text-decoration: none; padding-top: 2px; }
.cmt-ts:hover { text-decoration: underline; }
.cmt-b { min-width: 0; }
.cmt-who { font-size: 12px; font-weight: 600; color: var(--text-2); margin-bottom: 2px; }
.cmt-tx { font-size: 13.5px; color: var(--text); line-height: 1.5; white-space: pre-wrap; word-break: break-word; }
.cmt-mention { font: 500 12.5px var(--font-mono, "JetBrains Mono", monospace); color: var(--sodium-text); background: var(--sodium-wash); border-radius: 6px; padding: 0 5px; text-decoration: none; }
.cmt-mention:hover { text-decoration: underline; }
.cmt-form textarea {
  width: 100%; resize: vertical; padding: 10px 12px;
  background: var(--panel-2); color: var(--text);
  border: 1px solid var(--border); border-radius: var(--r-sm);
  font: 13.5px 'Space Grotesk', system-ui; line-height: 1.5; outline: none;
}
.cmt-form textarea:focus { border-color: color-mix(in oklab, var(--c-signal) 50%, var(--border)); }
.cmt-form-row { display: flex; align-items: center; gap: 8px; margin-top: 8px; }
.cmt-at {
  font: 500 11.5px var(--font-mono, "JetBrains Mono", monospace); color: var(--text-3);
  background: var(--panel-2); border: 1px solid var(--border); border-radius: var(--r-pill);
  padding: 5px 10px; cursor: pointer;
}
.cmt-at:hover { color: var(--text); }
.cmt-status { flex: 1; font-size: 12px; color: var(--text-3); }
.cmt-status a { color: var(--signal-text); }
.cmt-post-btn {
  background: var(--c-signal); color: var(--on-accent);
  font: 600 13px 'Space Grotesk', system-ui;
  border: none; border-radius: var(--r-pill); padding: 7px 16px; cursor: pointer; box-shadow: var(--pop-signal);
}
.cmt-post-btn:hover { transform: translateY(-1px); }
.cmt-post-btn:disabled { opacity: .6; cursor: not-allowed; }

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

// Outline — highlight the current row as the playhead moves. Reads every outline row's
// `data-t`, keeps a sorted list of moments, and on `timeupdate` finds the last moment
// whose t is ≤ currentTime (or the first, if the user is before the first moment).
// Uses rAF + a 250 ms idle throttle so this stays free during playback.
(function () {
  const list = document.getElementById('outlineList');
  if (!list) return;
  const rows = Array.from(list.querySelectorAll('.outline-row[data-t]'));
  if (rows.length === 0) return;
  const moments = rows.map((r) => parseFloat(r.getAttribute('data-t')) || 0);
  const v = document.querySelector('video');
  if (!v) return;
  let last = -1, raf = 0, lastRun = 0;
  const update = (now) => {
    if (now - lastRun < 250) { raf = requestAnimationFrame(update); return; }
    lastRun = now;
    const t = v.currentTime || 0;
    // Binary-search the latest moment ≤ t. Two-pointer walks forward as the user
    // scrubs; falls back to -1 (nothing highlighted) only if the user rewinds before
    // the first moment, which is the correct behaviour.
    let i = -1;
    for (let j = 0; j < moments.length; j++) { if (moments[j] <= t) i = j; else break; }
    if (i === last) { raf = requestAnimationFrame(update); return; }
    rows.forEach((r, k) => r.classList.toggle('is-current', k === i));
    last = i;
    // Auto-scroll the active row into view inside the outline list, but only when the
    // user's scroll position hasn't been touched recently — don't fight someone who's
    // reading a moment further down.
    if (i >= 0) {
      const target = rows[i];
      const listRect = list.getBoundingClientRect();
      const rowRect = target.getBoundingClientRect();
      if (rowRect.top < listRect.top + 24 || rowRect.bottom > listRect.bottom - 24) {
        target.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
      }
    }
    raf = requestAnimationFrame(update);
  };
  v.addEventListener('timeupdate', () => requestAnimationFrame(update));
  v.addEventListener('seeked', () => requestAnimationFrame(update));
  raf = requestAnimationFrame(update);
})();
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

// ---- Discussion (Fathom-style @-mention comments) ----
(function () {
  const root = document.querySelector('.discussion');
  if (!root) return;
  const cmtUrl = root.getAttribute('data-cmt-url');
  const thread = document.getElementById('cmtThread');
  const input = document.getElementById('cmtInput');
  const postBtn = document.getElementById('cmtPost');
  const atBtn = document.getElementById('cmtAt');
  const statusEl = document.getElementById('cmtStatus');
  const video = document.querySelector('video');
  const STAMP = /@(\d{1,2}:)?\d{1,2}:\d{2}/g; // @m:ss or @h:mm:ss

  const fmtT = (s) => {
    s = Math.max(0, Math.floor(s || 0));
    const m = Math.floor(s / 60), ss = String(s % 60).padStart(2, '0');
    return m + ':' + ss;
  };
  const parseStamp = (str) => str.split(':').reduce((a, p) => a * 60 + Number(p), 0);

  // Append comment text as safe DOM nodes — text via textNodes, @m:ss as seek links.
  // Never innerHTML user content (this is a public page rendering untrusted input).
  function appendText(parent, text) {
    let last = 0, m;
    STAMP.lastIndex = 0;
    while ((m = STAMP.exec(text)) !== null) {
      if (m.index > last) parent.appendChild(document.createTextNode(text.slice(last, m.index)));
      const stamp = m[0].slice(1);
      const a = document.createElement('a');
      a.href = '#t=' + parseStamp(stamp);
      a.className = 'cmt-mention';
      a.textContent = '@' + stamp;
      parent.appendChild(a);
      last = m.index + m[0].length;
    }
    if (last < text.length) parent.appendChild(document.createTextNode(text.slice(last)));
  }

  function row(c) {
    const el = document.createElement('div');
    el.className = 'cmt-row';
    const ts = document.createElement('a');
    ts.className = 'cmt-ts';
    ts.href = '#t=' + (c.t || 0);
    ts.textContent = fmtT(c.t);
    const body = document.createElement('div');
    body.className = 'cmt-b';
    const who = document.createElement('div');
    who.className = 'cmt-who';
    who.textContent = c.author || 'someone';
    const txt = document.createElement('div');
    txt.className = 'cmt-tx';
    appendText(txt, c.text || '');
    body.appendChild(who); body.appendChild(txt);
    el.appendChild(ts); el.appendChild(body);
    return el;
  }

  function render(list) {
    thread.textContent = '';
    if (!list.length) {
      const e = document.createElement('div');
      e.className = 'cmt-loading';
      e.textContent = 'No comments yet — start the conversation.';
      thread.appendChild(e);
      return;
    }
    list.sort((a, b) => (a.t || 0) - (b.t || 0)).forEach((c) => thread.appendChild(row(c)));
  }

  async function load() {
    try {
      const r = await fetch(cmtUrl);
      const j = await r.json();
      render(j.comments || []);
    } catch (e) { thread.textContent = ''; }
  }

  async function post() {
    const text = (input.value || '').trim();
    if (!text) return;
    postBtn.disabled = true; statusEl.textContent = '';
    try {
      const r = await fetch(cmtUrl, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ t: video ? video.currentTime : 0, text }),
      });
      if (r.status === 401) { statusEl.innerHTML = '<a href="https://clipxd.com">Log in</a> to comment'; return; }
      if (!r.ok) { statusEl.textContent = 'Could not post (' + r.status + ')'; return; }
      const c = await r.json();
      if (thread.querySelector('.cmt-loading')) thread.textContent = '';
      // keep the thread time-ordered: reload is simplest and cheap
      input.value = '';
      await load();
    } catch (e) { statusEl.textContent = 'Could not post — network error'; }
    finally { postBtn.disabled = false; }
  }

  if (atBtn) atBtn.addEventListener('click', () => {
    const t = video ? video.currentTime : 0;
    input.value = (input.value && !input.value.endsWith(' ') ? input.value + ' ' : input.value) + '@' + fmtT(t) + ' ';
    input.focus();
  });
  if (postBtn) postBtn.addEventListener('click', post);
  if (input) input.addEventListener('keydown', (e) => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) post(); });
  load();
})();
"##;

#[cfg(test)]
mod tests {
    use super::{format_view_count, merge_browser_trace_into_clip, mint_clip_id, share_base, share_slug_for, short_tail, slugify_title, try_claim, ClipClaims};
    use std::sync::{Arc, Mutex as StdMutex};

    #[test]
    fn browser_trace_merge_grafts_events_onto_a_video_clip_without_losing_video_fields() {
        use clipxd_index::{Index, Metadata, Source};
        let tmp = std::env::temp_dir().join(format!("clipxd-trace-merge-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut idx = Index::new(
            "clp_1",
            Source::Screen,
            Metadata {
                duration: 12.5,
                resolution: [1920, 1080],
                fps: 30.0,
                created_at: "1700000000".into(),
                title: "Screen recording".into(),
                description: String::new(),
                app_focus: vec![],
                url_context: None,
                has_video: true,
            },
        );
        idx.status = clipxd_index::Status::Complete;
        std::fs::write(tmp.join("index.json"), serde_json::to_string(&idx).unwrap()).unwrap();

        let trace = r#"{
            "clipxd_trace_version": "1", "session_id": "clp_1", "captured_by": "test",
            "started_at_ms": 1700000000000, "viewport": {"w": 1280, "h": 800},
            "url": "https://example.com",
            "events": [
                {"type": "click", "t_ms": 1700000001000, "click_kind": "left", "target": "button", "label": "Go"},
                {"type": "console", "t_ms": 1700000002000, "level": "error", "text": "boom", "uncaught": true}
            ]
        }"#;
        merge_browser_trace_into_clip(&tmp, "clp_1", trace).unwrap();

        let after: Index = serde_json::from_str(&std::fs::read_to_string(tmp.join("index.json")).unwrap()).unwrap();
        // video-derived fields untouched
        assert_eq!(after.metadata.duration, 12.5);
        assert_eq!(after.metadata.resolution, [1920, 1080]);
        assert!(after.metadata.has_video);
        // trace's events grafted on and the search corpus rebuilt to include them
        assert_eq!(after.event_track.len(), 2);
        assert!(after.event_track.iter().any(|e| e.kind == "click"));
        assert!(after.event_track.iter().any(|e| e.kind == "console_error"));
        assert!(after.search.as_ref().unwrap().events.contains("boom"));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn view_count_formats_compactly_past_a_thousand() {
        assert_eq!(format_view_count(0), "0");
        assert_eq!(format_view_count(1), "1");
        assert_eq!(format_view_count(999), "999");
        assert_eq!(format_view_count(1000), "1k");
        assert_eq!(format_view_count(1200), "1.2k");
        assert_eq!(format_view_count(45_000), "45k");
    }

    #[test]
    fn share_base_uses_host_port_and_lan_ip() {
        // port comes from the Host header; the host/ip part is replaced by the detected LAN ip
        assert_eq!(share_base(Some("192.168.1.42:8787"), "192.168.1.42"), "http://192.168.1.42:8787");
        assert_eq!(share_base(Some("localhost:9000"), "10.0.0.5"), "http://10.0.0.5:9000");
        // no port / unparseable → default 8787
        assert_eq!(share_base(None, "10.0.0.5"), "http://10.0.0.5:8787");
        assert_eq!(share_base(Some("box.local"), "10.0.0.5"), "http://10.0.0.5:8787");
    }

    #[test]
    fn slugify_title_handles_the_real_world() {
        // ASCII: lowercased, separators collapsed, no leading/trailing dashes
        assert_eq!(slugify_title("Hello World"), "hello-world");
        assert_eq!(slugify_title("  --spaces--  "), "spaces");
        // Non-ASCII drops entirely — a clip titled in Hindi still produces a usable slug
        assert_eq!(slugify_title("नमस्ते दुनिया"), "");
        // The 60-char cap + final-dash trim: long titles still end on an alphanumeric char
        let long = "a".repeat(80);
        let s = slugify_title(&long);
        assert_eq!(s.len(), 60);
        assert!(!s.ends_with('-'));
        // Pure emoji titles fall through to "" (the caller fills in "clip" then)
        assert_eq!(slugify_title("🎬🔥✨"), "");
    }

    #[test]
    fn short_tail_strips_prefix_and_walks_alphanumeric() {
        // Standard clip id: drop `clp_`, take the last 4 alphanumeric chars, lowercased.
        // Reverse "1efc6ad3" → ['3','d','a','6','c','f','e','1'], take 4 alnum → ['3','d','a','6']
        // reverse back → "6ad3".
        assert_eq!(short_tail("clp_1efc6ad3"), "6ad3");
        assert_eq!(short_tail("clp_abcdef12"), "ef12");
        assert_eq!(short_tail("clp_deadbeef"), "beef");
        // Strip the prefix even when the id doesn't match (graceful degradation)
        assert_eq!(short_tail("xyz_123456"), "3456");
        // No alphanumeric in the tail position falls back to ""
        assert_eq!(short_tail("clp_"), "");
    }

    #[test]
    fn share_slug_for_roundtrips_via_short_tail() {
        // A standard title produces a slug whose short tail resolves back to the id's tail
        let slug = share_slug_for("Checkout 500 — the card declines", "clp_1efc6ad3");
        assert!(slug.starts_with("checkout-500-the-card-declines"));
        assert!(slug.ends_with("-6ad3"), "slug should end with the id tail: {slug}");
        // Emoji-only title falls back to `clip-<tail>` so the URL is still shareable
        assert_eq!(share_slug_for("🎬", "clp_abcdef12"), "clip-ef12");
        // Empty title also falls back, with the same clip- prefix
        assert_eq!(share_slug_for("", "clp_abcdef12"), "clip-ef12");
        // Title that slugifies to nothing for non-emoji reasons also gets the clip- prefix
        assert_eq!(share_slug_for("!!!", "clp_abcdef12"), "clip-ef12");
        // Run of dashes inside the title collapses to one (slug stays URL-safe)
        assert_eq!(share_slug_for("a----b", "clp_abcdef12"), "a-b-ef12");
    }

    // Regression coverage for the sweeper-vs-commit race a code review caught: two finalizers
    // (a staged commit, the /ingest?reuse= fallback, and the abandoned-session sweeper) must
    // never write the same clip_dir concurrently. `try_claim`/`ClaimGuard` is the fix; these
    // tests pin its two load-bearing properties directly, without spinning up the HTTP layer.
    #[test]
    fn second_claim_on_the_same_id_is_rejected_while_the_first_is_held() {
        let claims: ClipClaims = Arc::new(StdMutex::new(std::collections::HashSet::new()));
        let first = try_claim(&claims, "clp_dup").expect("first claim succeeds");
        assert!(try_claim(&claims, "clp_dup").is_none(), "a second concurrent finalizer must be rejected");
        // A different id is unaffected — the claim is per-id, not a global lock.
        assert!(try_claim(&claims, "clp_other").is_some());
        drop(first);
    }

    #[test]
    fn dropping_the_guard_releases_the_claim_for_the_next_finalizer() {
        let claims: ClipClaims = Arc::new(StdMutex::new(std::collections::HashSet::new()));
        {
            let _first = try_claim(&claims, "clp_seq").expect("first claim succeeds");
            assert!(try_claim(&claims, "clp_seq").is_none());
        } // _first drops here — e.g. the sweeper's promote_staged failed and returned early
        assert!(try_claim(&claims, "clp_seq").is_some(), "a released id must be claimable again");
    }

    #[test]
    fn minted_clip_ids_never_collide_across_a_realistic_burst() {
        // The old scheme (32-bit truncated nanosecond timestamp) collides with 50% probability
        // after ~65K mints — a busy box would hit this in days. Assert the new scheme doesn't,
        // across a burst far larger than any single box will mint between restarts.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50_000 {
            assert!(seen.insert(mint_clip_id()), "mint_clip_id produced a duplicate");
        }
    }
}
