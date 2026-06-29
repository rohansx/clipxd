// The clipxd-web client. One backend origin (default http://localhost:8787, override with
// `?api=`). All requests carry the session cookie (same-origin prod) AND a Bearer token
// (cross-origin dev) so auth works in both. Shapes come from ./types (the Rust index schema).

import type { ClipSummary, Index, QueryAnswer, TextHit, ZoomKeyframe } from "./types";

const DEFAULT_API = "http://localhost:8787";
const TOKEN_KEY = "clipxd_token";

let TOKEN: string | null = (() => {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
})();

export function setToken(t: string | null): void {
  TOKEN = t;
  try {
    if (t) localStorage.setItem(TOKEN_KEY, t);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* storage may be unavailable */
  }
}

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

/** The backend origin. Honors `?api=` for cross-host testing; otherwise localhost:8787. */
export function apiBase(): string {
  const u = new URL(location.href);
  return u.searchParams.get("api") || DEFAULT_API;
}

/** A clip id deep-linked via `?clip=<id>` (used to open straight onto a clip on load). */
export function initialClipId(): string | null {
  return new URL(location.href).searchParams.get("clip");
}

export interface NetInfo {
  lan_ip: string;
  share_base: string;
  public_base: string | null;
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
  return { id: j.id, email: j.email, name: j.name, github: j.github };
}

export const signup = (email: string, password: string, name?: string, base = apiBase()) =>
  authPost("/auth/signup", { email, password, name }, base);

export const login = (email: string, password: string, base = apiBase()) =>
  authPost("/auth/login", { email, password }, base);

export async function logout(base = apiBase()): Promise<void> {
  await af(`${base}/auth/logout`, { method: "POST" }).catch(() => {});
  setToken(null);
}

/** Where to send the browser to start GitHub OAuth (full-page navigation). */
export const githubLoginUrl = (base = apiBase()) => `${base}/auth/github`;

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
  return `${base}/clip/${id}/frames/00001.png`;
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

export function downloadBlob(name: string, blob: Blob): void {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  URL.revokeObjectURL(a.href);
}
