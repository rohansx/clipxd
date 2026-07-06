// The clipxd-web client. In production the SPA and the backend share an origin (Caddy
// reverse-proxies /api/* to clipxd-web on :8787 and serves the SPA at the same origin), so
// `apiBase()` defaults to `location.origin` and the cookie goes back-and-forth automatically.
// For local dev we keep the old default of `http://localhost:8787` so the SPA on :5173 can
// still talk to a backend on :8787 — override with `?api=…` to point at a remote host.

import type { Index, ZoomKeyframe, ClipSummary, QueryAnswer, TextHit, Comment } from "./types";

// One Bearer token kept in module-local storage (lives in memory; cleared on reload).
let TOKEN: string | null = null;
const TOKEN_KEY = "clipxd.token";
try {
  TOKEN = localStorage.getItem(TOKEN_KEY);
} catch {
  /* storage may be unavailable (private mode etc.) */
}
export function setToken(t: string | null): void {
  TOKEN = t;
  try {
    if (t) localStorage.setItem(TOKEN_KEY, t);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* storage may be unavailable */
  }
}

const LOCAL_DEFAULT_API = "http://localhost:8787";

/**
 * fetch with auth attached. Same-origin (production behind Caddy) sends the httpOnly session
 * cookie automatically; cross-origin (local dev → :8787) uses the Bearer token instead. We do
 * NOT force `credentials:"include"` because the backend's permissive CORS returns `*`, which the
 * browser rejects together with credentials — Bearer covers the cross-origin case cleanly.
 */
function af(url: string, opts: RequestInit = {}): Promise<Response> {
  const headers = new Headers(opts.headers);
  if (TOKEN) headers.set("Authorization", `Bearer ${TOKEN}`);
  return fetch(url, { ...opts, headers });
}

/**
 * The backend origin.
 *   - Production (clipxd.com served by Caddy): same-origin, cookie auth.
 *   - Local dev (npm run dev on :5173): defaults to localhost:8787 (cross-origin, Bearer).
 *   - Override: `?api=https://staging.example.com` to point at a remote backend.
 */
export function apiBase(): string {
  const u = new URL(location.href);
  const override = u.searchParams.get("api");
  if (override) return override;
  // Same-origin (port 80/443, or hostname-only) — production behind Caddy.
  if (u.port === "" || u.port === "80" || u.port === "443") return u.origin;
  // Local dev (vite, rsbuild, etc. on a non-standard port) — assume :8787 backend.
  return LOCAL_DEFAULT_API;
}

/** A clip id deep-linked via `?clip=<id>` (used to open straight onto a clip on load). */
export function initialClipId(): string | null {
  const u = new URL(location.href);
  // 1) ?clip=<id> still wins (back-compat with earlier share links)
  const fromQuery = u.searchParams.get("clip");
  if (fromQuery) return fromQuery;
  // 2) /u/<username>/clip/<id> form: server-rendered page, SPA is served too (catch-all
  //    fallback), so when we land on the share URL the SPA should auto-open the clip.
  const m = u.pathname.match(/^\/u\/[^/]+\/clip\/([A-Za-z0-9_-]+)\/?$/);
  if (m) return m[1];
  return null;
}

export interface NetInfo {
  lan_ip: string;
  share_base: string;
  public_base: string | null;
  /** Authed: caller's chosen URL slug. null when unauthed or no slug picked. */
  username?: string | null;
  /** Authed: `https://HOST/u/<username>/clip/`. undefined when no slug. */
  user_share_base?: string;
}

async function jsonOrThrow<T>(r: Response, what: string): Promise<T> {
  if (!r.ok) throw new Error(`${what}: HTTP ${r.status}`);
  return (await r.json()) as T;
}

// ---- auth ----

export interface AuthUser {
  id: number;
  email: string;
  name: string | null;
  /** Optional URL slug for share links. Picked at signup or claimed later via /auth/username. */
  username: string | null;
  github: boolean;
}

export interface AuthStatus {
  /** false → server has no auth (local/LAN mode); true → hosted multi-tenant. */
  authEnabled: boolean;
  user: AuthUser | null;
}

/** GET /auth/me — distinguishes auth-off (404), logged-out (401), logged-in (200). */
export async function fetchAuthStatus(base = apiBase()): Promise<AuthStatus> {
  try {
    const r = await af(`${base}/auth/me`);
    if (r.status === 404) return { authEnabled: false, user: null };
    if (r.status === 401) return { authEnabled: true, user: null };
    if (r.ok) return { authEnabled: true, user: (await r.json()) as AuthUser };
    return { authEnabled: true, user: null };
  } catch {
    return { authEnabled: false, user: null };
  }
}

async function authPost(path: string, body: unknown, base = apiBase()): Promise<AuthUser> {
  const r = await af(`${base}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!r.ok) {
    const msg = await r.text().catch(() => "");
    throw new Error(msg || `HTTP ${r.status}`);
  }
  const j = (await r.json()) as AuthUser & { token?: string };
  if (j.token) setToken(j.token);
  return { id: j.id, email: j.email, name: j.name, username: j.username ?? null, github: j.github };
}

export const signup = (
  email: string,
  password: string,
  name?: string,
  username?: string,
  base = apiBase()
) => authPost("/auth/signup", { email, password, name, username }, base);

/** POST /auth/username — claim a slug for the currently-authenticated user (e.g. a GitHub-only account). */
export const setUsername = (username: string, base = apiBase()) =>
  authPost("/auth/username", { username }, base);

export const login = (email: string, password: string, base = apiBase()) =>
  authPost("/auth/login", { email, password }, base);

export async function logout(base = apiBase()): Promise<void> {
  await af(`${base}/auth/logout`, { method: "POST" }).catch(() => {});
  setToken(null);
}

/** Where to send the browser to start GitHub OAuth (full-page navigation). */
export const githubLoginUrl = (base = apiBase()) => `${base}/auth/github`;

// ---- BYOK settings (per-user NVIDIA/Gemini/Moondream keys + caption mode) ----

/** Presence/absence of the caller's BYOK keys + their caption mode. Never carries key values —
 *  see `saveKeys`/`fetchKeyStatus` (the server enforces this; there is no endpoint that returns
 *  the actual stored key). */
export interface KeyStatus {
  has_nvidia: boolean;
  has_gemini: boolean;
  has_moondream: boolean;
  caption_mode: "server" | "local";
}

/** Any subset of BYOK keys to set (a string) or clear (`null`), plus an optional caption_mode
 *  change. Fields omitted are left untouched server-side. */
export interface KeysUpdate {
  nvidia_api_key?: string | null;
  gemini_api_key?: string | null;
  moondream_api_key?: string | null;
  caption_mode?: "server" | "local";
}

/** GET /settings/keys — the caller's key presence/absence + caption mode. Requires auth. */
export async function fetchKeyStatus(base = apiBase()): Promise<KeyStatus> {
  return jsonOrThrow<KeyStatus>(await af(`${base}/settings/keys`), "settings/keys");
}

/** POST /settings/keys — set/clear any subset of BYOK keys and/or caption_mode.
 *  Returns the caller's fresh `KeyStatus`. Throws with the server's message on failure
 *  (e.g. an invalid caption_mode). */
export async function saveKeys(update: KeysUpdate, base = apiBase()): Promise<KeyStatus> {
  const r = await af(`${base}/settings/keys`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(update),
  });
  if (!r.ok) {
    const msg = await r.text().catch(() => "");
    throw new Error(msg || `HTTP ${r.status}`);
  }
  return (await r.json()) as KeyStatus;
}

// ---- clips ----

/** GET /clips — the library list (your clips, in auth mode), newest first. */
export async function fetchClips(base = apiBase()): Promise<ClipSummary[]> {
  try {
    const j = await jsonOrThrow<{ clips: ClipSummary[] }>(await af(`${base}/clips`), "clips");
    return j.clips ?? [];
  } catch {
    return [];
  }
}

export async function fetchIndex(id: string, base = apiBase()): Promise<Index> {
  return jsonOrThrow<Index>(await af(`${base}/clip/${id}/index.json`), `clip ${id}`);
}

/** Record one view of the clip; returns the new total. Fire-and-forget on the caller's
 *  side (a failed view count shouldn't block or error the page). */
export async function bumpViewCount(id: string, base = apiBase()): Promise<number> {
  const r = await af(`${base}/clip/${id}/view`, { method: "POST" });
  const j = (await r.json()) as { views?: number };
  return j.views ?? 0;
}

export async function fetchComments(id: string, base = apiBase()): Promise<Comment[]> {
  try {
    const r = await af(`${base}/clip/${id}/comments`);
    const j = (await r.json()) as { comments?: Comment[] };
    return j.comments ?? [];
  } catch {
    return [];
  }
}

/** Post a comment anchored to `t` seconds. Throws with a readable message on failure
 *  (e.g. 401 when login is required) so the caller can surface it. */
export async function postComment(id: string, t: number, text: string, base = apiBase()): Promise<Comment> {
  const r = await af(`${base}/clip/${id}/comments`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ t, text }),
  });
  if (!r.ok) {
    const msg = r.status === 401 ? "Log in to comment" : `Couldn't post (${r.status})`;
    throw new Error(msg);
  }
  return (await r.json()) as Comment;
}

export async function fetchZoom(id: string, base = apiBase()): Promise<ZoomKeyframe[]> {
  try {
    const r = await af(`${base}/clip/${id}/zoom.json`);
    return r.ok ? ((await r.json()) as ZoomKeyframe[]) : [];
  } catch {
    return [];
  }
}

export async function queryClip(id: string, q: string, base = apiBase()): Promise<QueryAnswer> {
  const r = await af(`${base}/clip/${id}/query?q=${encodeURIComponent(q)}`);
  const j = (await r.json()) as Partial<QueryAnswer>;
  return { text: j.text ?? "(no answer)", citations: j.citations ?? [] };
}

export async function searchClip(id: string, q: string, base = apiBase()): Promise<TextHit[]> {
  if (!q.trim()) return [];
  try {
    const r = await af(`${base}/clip/${id}/search?q=${encodeURIComponent(q)}`);
    return r.ok ? ((await r.json()) as TextHit[]) : [];
  } catch {
    return [];
  }
}

export function videoUrl(id: string, base = apiBase()): string {
  return `${base}/clip/${id}/video`;
}

export function frameUrl(id: string, name: string, base = apiBase()): string {
  return `${base}/clip/${id}/frames/${name}`;
}

export function thumbUrl(id: string, base = apiBase()): string {
  return `${base}/clip/${id}/thumbnail`;
}

export async function fetchNet(base = apiBase()): Promise<NetInfo | null> {
  try {
    return await jsonOrThrow<NetInfo>(await af(`${base}/net`), "net");
  } catch {
    return null;
  }
}

export async function shareLink(id: string, base = apiBase()): Promise<string> {
  const net = await fetchNet(base);
  // Prefer the owner's username-canonical form so the link carries their brand.
  if (net?.user_share_base) return `${net.user_share_base}/${id}`;
  const origin = (net?.public_base && net.public_base) || (net?.share_base && net.share_base) || base;
  return `${origin}/clip/${id}`;
}

export async function importUrl(url: string, base = apiBase()): Promise<string> {
  const r = await af(`${base}/import`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ url }),
  });
  if (!r.ok) {
    const msg = await r.text().catch(() => "");
    throw new Error(`import failed: HTTP ${r.status}${msg ? ` — ${msg}` : ""}`);
  }
  const j = (await r.json()) as { id: string };
  return j.id;
}

export async function ingest(blob: Blob, base = apiBase()): Promise<string> {
  const r = await af(`${base}/ingest`, { method: "POST", headers: { "content-type": "video/webm" }, body: blob });
  const j = await jsonOrThrow<{ id: string }>(r, "ingest");
  return j.id;
}

export interface RenderOpts {
  format?: "mp4" | "gif" | "webm";
  mockup?: boolean;
  bg?: string;
  project?: unknown;
}

export async function renderClip(id: string, opts: RenderOpts = {}, base = apiBase()): Promise<Blob> {
  const q = new URLSearchParams({
    format: opts.format ?? "mp4",
    mockup: String(opts.mockup ?? true),
    bg: opts.bg ?? "aurora",
  });
  const r = await af(`${base}/clip/${id}/render?${q}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(opts.project ?? {}),
  });
  if (!r.ok) throw new Error(`render: HTTP ${r.status}`);
  return r.blob();
}

export async function postCursor(
  id: string,
  track: { cursors: { t: number; x: number; y: number }[]; clicks: { t: number; x: number; y: number }[]; keys: unknown[] },
  base = apiBase(),
): Promise<void> {
  await af(`${base}/clip/${id}/cursor`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(track),
  });
}

/** Re-run Phase 2 (captioning/OCR/transcription) on an existing clip. Idempotent. */
export async function reEnrichClip(id: string, base = apiBase()): Promise<void> {
  await af(`${base}/clip/${id}/re-enrich`, { method: "POST" });
}

export function downloadBlob(name: string, blob: Blob): void {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  URL.revokeObjectURL(a.href);
}
