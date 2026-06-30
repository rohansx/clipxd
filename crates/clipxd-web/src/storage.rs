//! Storage abstraction — local filesystem (today) or S3-compatible object storage
//! (Hetzner Object Storage, Cloudflare R2, MinIO, Garage, AWS S3).
//!
//! Layout keys:
//!   <prefix>/<clip_id>/index.json
//!   <prefix>/<clip_id>/video.mp4        (or video.webm / source.mp4)
//!   <prefix>/<clip_id>/zoom.json
//!   <prefix>/<clip_id>/events.json
//!   <prefix>/<clip_id>/frames/00001.png
//!   <prefix>/<clip_id>/frames/00002.png
//!
//! `prefix` defaults to empty. The clip-id is treated as the directory; on S3 this
//! becomes a single namespace prefix in the bucket.
//!
//! `CLIPXD_STORAGE=s3://bucket[/prefix]?endpoint=...&region=auto` selects S3 mode and
//! `CLIPXD_S3_ENDPOINT` / `CLIPXD_S3_REGION` / `CLIPXD_S3_ACCESS_KEY` /
//! `CLIPXD_S3_SECRET_KEY` are the auth knobs.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use aws_sdk_s3::config::Credentials;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client;
use std::path::{Path, PathBuf};
use tokio::sync::OnceCell;

#[derive(Clone, Debug)]
pub enum StorageKind {
    Local { root: PathBuf },
    S3 {
        bucket: String,
        prefix: String,
        endpoint: Option<String>,
        region: String,
        /// Lazily initialised on first `make_storage` call. Holding it in a OnceCell means
        /// a successful build is cached for the lifetime of the process — important on S3
        /// because the credential provider does a network roundtrip on init.
        client: OnceCell<S3Client>,
    },
}

impl StorageKind {
    pub fn from_env(local_default: &Path) -> Self {
        let raw = match std::env::var("CLIPXD_STORAGE") {
            Ok(s) if !s.is_empty() => s,
            _ => return StorageKind::Local { root: local_default.to_path_buf() },
        };
        if raw == "local" || raw.starts_with("file://") {
            return StorageKind::Local { root: local_default.to_path_buf() };
        }
        if let Some(rest) = raw.strip_prefix("s3://") {
            let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
            let mut parts = path.splitn(2, '/');
            let bucket = parts.next().unwrap_or("").to_string();
            let prefix = parts.next().unwrap_or("").trim_end_matches('/').to_string();
            let mut endpoint: Option<String> = None;
            let mut region = String::from("auto");
            for kv in query.split('&').filter(|s| !s.is_empty()) {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "endpoint" => endpoint = Some(urldecode(v)),
                        "region" => region = urldecode(v),
                        _ => {}
                    }
                }
            }
            // Per-env overrides win over the URL-embedded values (less typing at the env level).
            if let Ok(v) = std::env::var("CLIPXD_S3_ENDPOINT") {
                if !v.is_empty() { endpoint = Some(v); }
            }
            if let Ok(v) = std::env::var("CLIPXD_S3_REGION") {
                if !v.is_empty() { region = v; }
            }
            if !bucket.is_empty() {
                eprintln!(
                    "INFO CLIPXD_STORAGE=s3 enabled (bucket={bucket}, prefix={prefix}, region={region}, endpoint={})",
                    endpoint.as_deref().unwrap_or("(default AWS)")
                );
                return StorageKind::S3 { bucket, prefix, endpoint, region, client: OnceCell::new() };
            }
        }
        eprintln!(
            "WARN CLIPXD_STORAGE={raw:?} is unrecognised; expected 'local' or 's3://bucket[/prefix]?endpoint=...&region=...'. Falling back to local."
        );
        StorageKind::Local { root: local_default.to_path_buf() }
    }

    /// Build a boxed Storage instance. We always allocate (cheap) so the returned
    /// `Box<dyn Storage>` can outlive any borrow of `&self`.
    pub async fn make_storage(&self) -> Result<Box<dyn Storage>> {
        Ok(match self {
            StorageKind::Local { root } => Box::new(LocalStorage { root: root.clone() }),
            StorageKind::S3 { bucket, prefix, endpoint, region, client } => {
                let c = client
                    .get_or_try_init(|| async {
                        let access = std::env::var("CLIPXD_S3_ACCESS_KEY")
                            .context("CLIPXD_S3_ACCESS_KEY is required for s3:// CLIPXD_STORAGE")?;
                        let secret = std::env::var("CLIPXD_S3_SECRET_KEY")
                            .context("CLIPXD_S3_SECRET_KEY is required for s3:// CLIPXD_STORAGE")?;
                        let creds = Credentials::new(access, secret, None, None, "clipxd");
                        let mut cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
                            .region(aws_config::Region::new(region.clone()))
                            .credentials_provider(creds);
                        if let Some(ep) = endpoint {
                            cfg = cfg.endpoint_url(ep);
                        }
                        let shared = cfg.load().await;
                        Ok::<_, anyhow::Error>(S3Client::new(&shared))
                    })
                    .await
                    .map_err(|e| anyhow!("init s3 client: {e:#}"))?;
                Box::new(S3Storage { bucket: bucket.clone(), prefix: prefix.clone(), client: c.clone() })
            }
        })
    }
}

#[async_trait]
pub trait Storage: Send + Sync {
    async fn read_object(&self, key: &str) -> Result<Option<Vec<u8>>>;
    async fn write_object(&self, key: &str, body: Vec<u8>, content_type: &str) -> Result<()>;
    async fn delete_prefix(&self, prefix: &str) -> Result<()>;
    /// Best-effort: list all keys under a prefix (mostly for cleanup / orphans).
    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>> { Ok(Vec::new()) }
}

pub struct LocalStorage {
    pub root: PathBuf,
}

#[async_trait]
impl Storage for LocalStorage {
    async fn read_object(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let p = self.root.join(sanitize(key));
        match std::fs::read(&p) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    async fn write_object(&self, key: &str, body: Vec<u8>, _ct: &str) -> Result<()> {
        let p = self.root.join(sanitize(key));
        if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(&p, &body).with_context(|| format!("write {}", p.display()))?;
        Ok(())
    }
    async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        let p = self.root.join(sanitize(prefix));
        if p.is_dir() { std::fs::remove_dir_all(&p).with_context(|| format!("rm {}", p.display()))?; }
        Ok(())
    }
    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>> {
        let dir = self.root.join(sanitize(prefix));
        let mut keys = Vec::new();
        if !dir.exists() { return Ok(keys); }
        for entry in walk_dir(&dir, &dir) {
            keys.push(entry);
        }
        Ok(keys)
    }
}

pub struct S3Storage {
    pub bucket: String,
    pub prefix: String,
    pub client: S3Client,
}

#[async_trait]
impl Storage for S3Storage {
    async fn read_object(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let full = self.full_key(key);
        let r = self.client.get_object().bucket(&self.bucket).key(&full).send().await;
        match r {
            Ok(out) => {
                let b = out.body.collect().await?.into_bytes().to_vec();
                Ok(Some(b))
            }
            Err(e) => {
                if let aws_sdk_s3::error::SdkError::ServiceError(se) = &e {
                    if matches!(se.err(), aws_sdk_s3::operation::get_object::GetObjectError::NoSuchKey(_)) {
                        return Ok(None);
                    }
                }
                Err(e.into())
            }
        }
    }
    async fn write_object(&self, key: &str, body: Vec<u8>, content_type: &str) -> Result<()> {
        let full = self.full_key(key);
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&full)
            .body(ByteStream::from(body))
            .content_type(content_type)
            .send()
            .await?;
        Ok(())
    }
    async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        let full = format!("{}{}", self.prefix.trim_end_matches('/'), sanitize(prefix));
        let mut continuation: Option<String> = None;
        loop {
            let mut req = self.client.list_objects_v2().bucket(&self.bucket).prefix(&full);
            if let Some(t) = &continuation { req = req.continuation_token(t); }
            let out = req.send().await?;
            let Some(contents) = out.contents else { break; };
            for obj in contents {
                if let Some(k) = obj.key {
                    let _ = self.client.delete_object().bucket(&self.bucket).key(k).send().await;
                }
            }
            continuation = out.next_continuation_token;
            if continuation.is_none() { break; }
        }
        Ok(())
    }
    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>> {
        let full = format!("{}{}", self.prefix.trim_end_matches('/'), sanitize(prefix));
        let mut keys = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            let mut req = self.client.list_objects_v2().bucket(&self.bucket).prefix(&full);
            if let Some(t) = &continuation { req = req.continuation_token(t); }
            let out = req.send().await?;
            let Some(contents) = out.contents else { break; };
            for obj in contents {
                if let Some(k) = obj.key {
                    // Strip the prefix so callers get the logical key (relative to the prefix).
                    keys.push(k.strip_prefix(&format!("{}/", self.prefix.trim_end_matches('/'))).unwrap_or(&k).to_string());
                }
            }
            continuation = out.next_continuation_token;
            if continuation.is_none() { break; }
        }
        Ok(keys)
    }
}

impl S3Storage {
    fn full_key(&self, key: &str) -> String {
        let p = sanitize(key);
        if self.prefix.is_empty() { p } else { format!("{}/{}", self.prefix.trim_end_matches('/'), p) }
    }
}

/// Reject path traversal — `..`, leading `/`, etc. The clip-id and frame-name are already
/// validated upstream; this is belt-and-braces.
fn sanitize(s: &str) -> String {
    s.replace('\\', "/")
        .trim_start_matches('/')
        .split('/')
        .filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn walk_dir(root: &Path, here: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(here) {
        for entry in rd.flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(&path);
            if path.is_dir() {
                out.extend(walk_dir(root, &path));
            } else if let Some(s) = rel.to_str() {
                out.push(s.to_string());
            }
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    urlencoding::decode(s).map(|c| c.into_owned()).unwrap_or_else(|_| s.to_string())
}
