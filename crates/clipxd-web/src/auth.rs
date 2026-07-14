//! Multi-tenant auth for the hosted tier: local accounts (email + password, argon2) and GitHub
//! OAuth, with JWT sessions (HS256) carried in an httpOnly cookie. A small SQLite store holds
//! users and per-clip ownership so each account sees only its own library — while the public,
//! unguessable share links stay open (that's a separate, intentional surface).
//!
//! ## Username routing
//!
//! Each user picks a unique `username` at signup (3-30 chars, [a-z0-9_-]). The share-link
//! canonical URL is `https://HOST/u/:username/clip/:id`. The bare `/clip/:id` redirects to
//! that form once we know the owner.

use anyhow::{Context, Result};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::http::HeaderMap;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub const COOKIE: &str = "clipxd_session";
const SESSION_DAYS: u64 = 30;

/// Validate a username (called both at signup and at route-binding time).
/// Rules: 3-30 chars, [a-z0-9_-], no leading/trailing dash, no reserved words.
pub fn validate_username(s: &str) -> Result<String> {
    if s.is_empty() || s.len() > 30 {
        anyhow::bail!("username must be 1-30 characters");
    }
    let trimmed = s.trim_matches(|c: char| c == '-' || c == '_');
    if trimmed.len() < 3 {
        anyhow::bail!("username must be at least 3 characters (after stripping dashes/underscores)");
    }
    let mut all_ok = true;
    for c in s.chars() {
        let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_';
        if !ok { all_ok = false; break; }
    }
    if !all_ok {
        anyhow::bail!("username may only contain lowercase letters, digits, '-' and '_'");
    }
    if matches!(s, "u" | "auth" | "clip" | "clips" | "api" | "admin" | "login" | "logout" | "settings" | "library" | "mcp" | "ingest" | "import" | "net" | "share" | "user" | "users") {
        anyhow::bail!("username is reserved");
    }
    Ok(s.to_string())
}

/// A user record. `pw_hash` is null for GitHub-only accounts; `github_id` is null for password-only.
#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub username: Option<String>,
    pub pw_hash: Option<String>,
    pub github_id: Option<i64>,
    pub name: Option<String>,
}

/// The authenticated principal extracted from a request's JWT.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: i64,
    pub email: String,
}

/// Presence/absence of a user's BYOK keys + their caption mode — the shape the settings UI
/// reads. Never carries the actual key values (see [`Db::key_status`]).
#[derive(Debug, Clone, Serialize)]
pub struct KeyStatus {
    pub has_nvidia: bool,
    pub has_gemini: bool,
    pub has_moondream: bool,
    pub caption_mode: String,
}

/// SQLite-backed user + clip-ownership store. Cloned cheaply (Arc); ops are short and synchronous.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("open db {}", path.display()))?;
        // Schema. CREATE TABLE IF NOT EXISTS for first-run, then idempotent ALTER for upgrades.
        // The `username` column arrived later — backfill-safe (nullable on read).
        conn.execute_batch(
            r#"
            PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS users (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                email      TEXT UNIQUE NOT NULL,
                pw_hash    TEXT,
                github_id  INTEGER UNIQUE,
                name       TEXT,
                username   TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS clips (
                clip_id    TEXT PRIMARY KEY,
                owner_id   INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_clips_owner ON clips(owner_id);
            "#,
        )?;
        // Idempotent column add (returns harmless "duplicate column" error, swallowed).
        let _ = conn.execute("ALTER TABLE users ADD COLUMN username TEXT", []);
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_username ON users(username) WHERE username IS NOT NULL;",
        )?;
        // BYOK: a user's own NVIDIA/Gemini/Moondream keys, used instead of the server's shared
        // ones for their own clips — so their usage lands on their own account/quota, not ours.
        // `caption_mode` picks between the server's captioner and fully client-side captioning
        // (WebGPU Moondream in the browser/extension — no network call to anyone). Stored
        // plaintext, same trust boundary as the server's own keys in its env file (filesystem
        // permissions, not app-level encryption) — never echoed back over HTTP, only
        // presence/absence booleans (see `Db::key_status`).
        for col in ["nvidia_api_key", "gemini_api_key", "moondream_api_key"] {
            let _ = conn.execute(&format!("ALTER TABLE users ADD COLUMN {col} TEXT"), []);
        }
        let _ = conn.execute("ALTER TABLE users ADD COLUMN caption_mode TEXT NOT NULL DEFAULT 'server'", []);
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Insert a password user with an already-validated username.
    /// Returns Err("username taken") if the index collides.
    pub fn create_password_user(&self, email: &str, pw_hash: &str, name: Option<&str>, username: Option<&str>) -> Result<User> {
        let username = username.map(validate_username).transpose()?;
        let c = self.lock();
        c.execute(
            "INSERT INTO users (email, pw_hash, name, username, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![email, pw_hash, name, username, now()],
        ).map_err(|e| {
            if let rusqlite::Error::SqliteFailure(ref sf, _) = e {
                if sf.code == rusqlite::ErrorCode::ConstraintViolation {
                    return anyhow::anyhow!("username taken");
                }
            }
            anyhow::anyhow!(e)
        })?;
        let id = c.last_insert_rowid();
        Ok(User { id, email: email.to_string(), username, pw_hash: Some(pw_hash.to_string()), github_id: None, name: name.map(String::from) })
    }

    pub fn find_by_email(&self, email: &str) -> Result<Option<User>> {
        Ok(self
            .lock()
            .query_row("SELECT id, email, username, pw_hash, github_id, name FROM users WHERE email = ?1", [email], row_to_user)
            .optional()?)
    }

    pub fn find_by_id(&self, id: i64) -> Result<Option<User>> {
        Ok(self
            .lock()
            .query_row("SELECT id, email, username, pw_hash, github_id, name FROM users WHERE id = ?1", [id], row_to_user)
            .optional()?)
    }

    pub fn find_by_username(&self, username: &str) -> Result<Option<User>> {
        Ok(self
            .lock()
            .query_row("SELECT id, email, username, pw_hash, github_id, name FROM users WHERE username = ?1", [username], row_to_user)
            .optional()?)
    }

    /// Set username on an existing user (after they've picked one) — for the OAuth backfill path
    /// where the user signs in via GitHub before picking a slug.
    pub fn set_username(&self, user_id: i64, username: &str) -> Result<()> {
        let username = validate_username(username)?;
        let c = self.lock();
        c.execute(
            "UPDATE users SET username = ?1 WHERE id = ?2",
            rusqlite::params![username, user_id],
        ).map_err(|e| {
            if let rusqlite::Error::SqliteFailure(ref sf, _) = e {
                if sf.code == rusqlite::ErrorCode::ConstraintViolation {
                    return anyhow::anyhow!("username taken");
                }
            }
            anyhow::anyhow!(e)
        })?;
        Ok(())
    }

    /// Presence/absence of each BYOK key + the caption mode — what the settings UI shows.
    /// Never returns the actual key values (see [`Db::llm_keys`]/[`Db::moondream_key`], which
    /// are for internal use only, right before making the call they're for).
    pub fn key_status(&self, user_id: i64) -> Result<KeyStatus> {
        let c = self.lock();
        c.query_row(
            "SELECT nvidia_api_key IS NOT NULL, gemini_api_key IS NOT NULL, moondream_api_key IS NOT NULL, caption_mode FROM users WHERE id = ?1",
            [user_id],
            |r| Ok(KeyStatus { has_nvidia: r.get(0)?, has_gemini: r.get(1)?, has_moondream: r.get(2)?, caption_mode: r.get(3)? }),
        ).map_err(|e| anyhow::anyhow!(e))
    }

    /// Set (or, with `None`, clear) one of the BYOK keys / the caption mode. `field` is one of
    /// `nvidia_api_key` / `gemini_api_key` / `moondream_api_key` / `caption_mode` — a fixed,
    /// internally-controlled set (never built from request input), so interpolating it into the
    /// column name here is safe from injection.
    fn set_field(&self, user_id: i64, field: &str, value: Option<&str>) -> Result<()> {
        self.lock().execute(&format!("UPDATE users SET {field} = ?1 WHERE id = ?2"), rusqlite::params![value, user_id])?;
        Ok(())
    }
    pub fn set_nvidia_key(&self, user_id: i64, key: Option<&str>) -> Result<()> { self.set_field(user_id, "nvidia_api_key", key) }
    pub fn set_gemini_key(&self, user_id: i64, key: Option<&str>) -> Result<()> { self.set_field(user_id, "gemini_api_key", key) }
    pub fn set_moondream_key(&self, user_id: i64, key: Option<&str>) -> Result<()> { self.set_field(user_id, "moondream_api_key", key) }
    pub fn set_caption_mode(&self, user_id: i64, mode: &str) -> Result<()> {
        anyhow::ensure!(matches!(mode, "server" | "local"), "caption_mode must be 'server' or 'local'");
        self.set_field(user_id, "caption_mode", Some(mode))
    }

    /// The user's own NVIDIA/Gemini keys for making an LLM call on their behalf — `None` for
    /// either means "use the server's own env-configured key instead" (see `llm::complete`).
    pub fn llm_keys(&self, user_id: i64) -> Result<(Option<String>, Option<String>)> {
        Ok(self.lock().query_row(
            "SELECT nvidia_api_key, gemini_api_key FROM users WHERE id = ?1",
            [user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?.unwrap_or((None, None)))
    }

    /// The user's own Moondream cloud API key (`x-moondream-auth`) — `None` means "use the
    /// server's own `CLIPXD_TOKEN` instead".
    pub fn moondream_key(&self, user_id: i64) -> Result<Option<String>> {
        Ok(self.lock().query_row("SELECT moondream_api_key FROM users WHERE id = ?1", [user_id], |r| r.get(0)).optional()?.flatten())
    }

    /// Find or create the user for a GitHub identity. Links to an existing account by email.
    pub fn upsert_github(&self, github_id: i64, email: &str, name: Option<&str>) -> Result<User> {
        let c = self.lock();
        if let Some(u) = c
            .query_row("SELECT id, email, username, pw_hash, github_id, name FROM users WHERE github_id = ?1", [github_id], row_to_user)
            .optional()?
        {
            return Ok(u);
        }
        // link by email if a password account already exists, else create a fresh GitHub account
        if let Some(existing) = c
            .query_row("SELECT id, email, username, pw_hash, github_id, name FROM users WHERE email = ?1", [email], row_to_user)
            .optional()?
        {
            c.execute("UPDATE users SET github_id = ?1, name = COALESCE(name, ?2) WHERE id = ?3", rusqlite::params![github_id, name, existing.id])?;
            return Ok(User { github_id: Some(github_id), ..existing });
        }
        c.execute(
            "INSERT INTO users (email, github_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![email, github_id, name, now()],
        )?;
        let id = c.last_insert_rowid();
        Ok(User { id, email: email.to_string(), username: None, pw_hash: None, github_id: Some(github_id), name: name.map(String::from) })
    }

    pub fn set_clip_owner(&self, clip_id: &str, owner_id: i64) -> Result<()> {
        self.lock().execute(
            "INSERT OR REPLACE INTO clips (clip_id, owner_id, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![clip_id, owner_id, now()],
        )?;
        Ok(())
    }

    pub fn clip_owner(&self, clip_id: &str) -> Result<Option<i64>> {
        Ok(self.lock().query_row("SELECT owner_id FROM clips WHERE clip_id = ?1", [clip_id], |r| r.get(0)).optional()?)
    }

    pub fn clips_for_owner(&self, owner_id: i64) -> Result<std::collections::HashSet<String>> {
        let c = self.lock();
        let mut stmt = c.prepare("SELECT clip_id FROM clips WHERE owner_id = ?1")?;
        let ids = stmt.query_map([owner_id], |r| r.get::<_, String>(0))?.filter_map(Result::ok).collect();
        Ok(ids)
    }

    /// Resolve a clip by the short suffix used in branded share links
    /// (`clipxd.com/u/<you>/<title-slug>-<SHORT>`).  The clip id itself stays the
    /// unguessable secret — `<SHORT>` is only the last 4 chars of the id, exposed
    /// so the URL reads like a title rather than `clp_1efc6ad3` while still being
    /// unique enough to disambiguate same-titled clips owned by the same user.
    /// Returns `Some(id)` when exactly one owned clip's id ends with `short`, `None`
    /// otherwise (no match, or ambiguous — both resolve to 404 so the secret id
    /// never leaks).  The clip is unguessable without the prefix.
    pub fn find_clip_by_short(&self, owner_id: i64, short: &str) -> Result<Option<String>> {
        let c = self.lock();
        let mut stmt = c.prepare("SELECT clip_id FROM clips WHERE owner_id = ?1")?;
        let mut hits: Vec<String> = stmt
            .query_map([owner_id], |r| r.get::<_, String>(0))?
            .filter_map(Result::ok)
            .filter(|id| id.len() >= short.len() && id[id.len() - short.len()..] == *short)
            .collect();
        // If two clips collide on the same 4-char tail (vanishingly unlikely with the
        // current id format), bail — both URLs 404 rather than the wrong clip silently
        // resolving, so guessing tails is a dead end.
        if hits.len() != 1 { return Ok(None); }
        Ok(Some(hits.remove(0)))
    }
}

fn row_to_user(r: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        email: r.get(1)?,
        username: r.get(2)?,
        pw_hash: r.get(3)?,
        github_id: r.get(4)?,
        name: r.get(5)?,
    })
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

// ---- password hashing (argon2) ----

pub fn hash_password(pw: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default().hash_password(pw.as_bytes(), &salt).map_err(|e| anyhow::anyhow!("hash: {e}"))?.to_string())
}

pub fn verify_password(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| Argon2::default().verify_password(pw.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

// ---- JWT sessions (HS256) ----

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: i64,
    email: String,
    exp: usize,
}

pub fn issue_jwt(secret: &str, user: &User) -> Result<String> {
    let exp = (now() as u64 + SESSION_DAYS * 86400) as usize;
    let claims = Claims { sub: user.id, email: user.email.clone(), exp };
    Ok(encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))?)
}

fn verify_jwt(secret: &str, token: &str) -> Option<AuthUser> {
    let data = decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &Validation::new(Algorithm::HS256)).ok()?;
    Some(AuthUser { id: data.claims.sub, email: data.claims.email })
}

/// Pull the JWT from the `clipxd_session` cookie or an `Authorization: Bearer` header and verify it.
pub fn authenticate(secret: &str, headers: &HeaderMap) -> Option<AuthUser> {
    if let Some(tok) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if let Some(u) = verify_jwt(secret, tok.trim()) {
            return Some(u);
        }
    }
    let cookies = headers.get(axum::http::header::COOKIE).and_then(|v| v.to_str().ok())?;
    let token = cookies.split(';').filter_map(|kv| kv.trim().split_once('=')).find(|(k, _)| *k == COOKIE).map(|(_, v)| v)?;
    verify_jwt(secret, token)
}

/// Build the `Set-Cookie` value for a freshly issued session (httpOnly; Secure only on HTTPS).
pub fn session_cookie(jwt: &str, secure: bool) -> String {
    let base = format!("{COOKIE}={jwt}; HttpOnly; Path=/; SameSite=Lax; Max-Age={}", SESSION_DAYS * 86400);
    if secure {
        format!("{base}; Secure")
    } else {
        base
    }
}

/// The `Set-Cookie` value that clears the session.
pub fn clear_cookie(secure: bool) -> String {
    let base = format!("{COOKIE}=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0");
    if secure {
        format!("{base}; Secure")
    } else {
        base
    }
}

// ---- GitHub OAuth ----

#[derive(Clone)]
pub struct GithubCfg {
    pub client_id: String,
    pub client_secret: String,
}

impl GithubCfg {
    /// Read the OAuth app credentials from the environment (secret never lives in code/git).
    pub fn from_env() -> Option<Self> {
        let client_id = std::env::var("GITHUB_CLIENT_ID").ok().filter(|s| !s.is_empty())?;
        let client_secret = std::env::var("GITHUB_CLIENT_SECRET").ok().filter(|s| !s.is_empty())?;
        Some(Self { client_id, client_secret })
    }

    /// The URL to redirect a user to, to start the OAuth dance.
    pub fn authorize_url(&self, redirect_uri: &str, state: &str) -> String {
        format!(
            "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={}&allow_signup=true",
            urlencoding::encode(&self.client_id),
            urlencoding::encode(redirect_uri),
            urlencoding::encode("read:user user:email"),
            urlencoding::encode(state),
        )
    }
}

/// A verified GitHub identity (after the code exchange).
pub struct GithubIdentity {
    pub github_id: i64,
    pub email: String,
    pub name: Option<String>,
}

/// Exchange an OAuth `code` for an access token, then fetch the user's id + primary email.
pub async fn exchange_github_code(cfg: &GithubCfg, code: &str, redirect_uri: &str) -> Result<GithubIdentity> {
    let client = reqwest::Client::builder().user_agent("clipxd").build()?;

    #[derive(Deserialize)]
    struct Token {
        access_token: Option<String>,
    }
    let token: Token = client
        .post("https://github.com/login/oauth/access_token")
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({
            "client_id": cfg.client_id,
            "client_secret": cfg.client_secret,
            "code": code,
            "redirect_uri": redirect_uri,
        }))
        .send()
        .await?
        .json()
        .await?;
    let access = token.access_token.context("github did not return an access token")?;

    #[derive(Deserialize)]
    struct GhUser {
        id: i64,
        name: Option<String>,
        login: String,
        email: Option<String>,
    }
    let user: GhUser = client
        .get("https://api.github.com/user")
        .bearer_auth(&access)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?
        .json()
        .await?;

    // /user.email is often null unless public — fetch the verified primary from /user/emails.
    let email = match user.email {
        Some(e) => e,
        None => {
            #[derive(Deserialize)]
            struct Email {
                email: String,
                primary: bool,
                verified: bool,
            }
            let emails: Vec<Email> = client
                .get("https://api.github.com/user/emails")
                .bearer_auth(&access)
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .send()
                .await?
                .json()
                .await
                .unwrap_or_default();
            emails
                .into_iter()
                .find(|e| e.primary && e.verified)
                .map(|e| e.email)
                .unwrap_or_else(|| format!("{}@users.noreply.github.com", user.login))
        }
    };

    Ok(GithubIdentity { github_id: user.id, email, name: user.name.or(Some(user.login)) })
}

/// A random URL-safe token (OAuth state / CSRF).
pub fn random_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Bundled auth configuration + store, attached to `AppState` when `CLIPXD_AUTH=1`.
#[derive(Clone)]
pub struct AuthState {
    pub db: Db,
    pub jwt_secret: Arc<String>,
    pub github: Option<GithubCfg>,
    /// Public origin (e.g. `https://clips.example.com`) for OAuth redirect_uri + post-login redirect.
    pub app_base: Arc<String>,
    pub cookie_secure: bool,
}

impl AuthState {
    pub fn from_env(clips_dir: &Path) -> Result<Self> {
        let db = Db::open(&clips_dir.join("clipxd.db"))?;
        let jwt_secret = std::env::var("CLIPXD_JWT_SECRET")
            .ok()
            .filter(|s| s.len() >= 16)
            .context("CLIPXD_AUTH=1 requires CLIPXD_JWT_SECRET (>= 16 chars)")?;
        let app_base = std::env::var("CLIPXD_PUBLIC_BASE").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "http://localhost:8787".to_string());
        let cookie_secure = app_base.starts_with("https://");
        Ok(Self { db, jwt_secret: Arc::new(jwt_secret), github: GithubCfg::from_env(), app_base: Arc::new(app_base), cookie_secure })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh `Db` backed by a throwaway file under the system temp dir, unique per test (by
    /// name) so parallel `cargo test` runs never collide on the same SQLite file.
    fn test_db(name: &str) -> (Db, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("clipxd-auth-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        let db = Db::open(&path).expect("open test db");
        (db, dir)
    }

    #[test]
    fn key_status_reflects_set_and_clear() {
        let (db, dir) = test_db("key-status");
        let user = db.create_password_user("byok@example.com", "hash", None, None).unwrap();

        // fresh user: nothing set, default caption_mode
        let status = db.key_status(user.id).unwrap();
        assert!(!status.has_nvidia);
        assert!(!status.has_gemini);
        assert!(!status.has_moondream);
        assert_eq!(status.caption_mode, "server");

        // set all three keys
        db.set_nvidia_key(user.id, Some("nvidia-secret")).unwrap();
        db.set_gemini_key(user.id, Some("gemini-secret")).unwrap();
        db.set_moondream_key(user.id, Some("moondream-secret")).unwrap();
        let status = db.key_status(user.id).unwrap();
        assert!(status.has_nvidia);
        assert!(status.has_gemini);
        assert!(status.has_moondream);

        // clear just nvidia — the others stay set
        db.set_nvidia_key(user.id, None).unwrap();
        let status = db.key_status(user.id).unwrap();
        assert!(!status.has_nvidia);
        assert!(status.has_gemini);
        assert!(status.has_moondream);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_caption_mode_rejects_an_invalid_mode() {
        let (db, dir) = test_db("caption-mode-invalid");
        let user = db.create_password_user("mode@example.com", "hash", None, None).unwrap();

        let err = db.set_caption_mode(user.id, "cloud").unwrap_err();
        assert!(err.to_string().contains("caption_mode must be"), "{err}");
        // rejected write must not have changed anything
        assert_eq!(db.key_status(user.id).unwrap().caption_mode, "server");

        db.set_caption_mode(user.id, "local").unwrap();
        assert_eq!(db.key_status(user.id).unwrap().caption_mode, "local");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn llm_keys_default_to_none_for_a_user_who_never_set_any() {
        let (db, dir) = test_db("llm-keys-default");
        let user = db.create_password_user("nokeys@example.com", "hash", None, None).unwrap();

        let (nvidia, gemini) = db.llm_keys(user.id).unwrap();
        assert_eq!(nvidia, None);
        assert_eq!(gemini, None);
        assert_eq!(db.moondream_key(user.id).unwrap(), None);

        std::fs::remove_dir_all(&dir).ok();
    }
}
