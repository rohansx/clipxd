/**
 * Per-tab state for "the clip you just made (or tried to make)".
 *
 * History:
 *   v1 — single happy-path record
 *   v2 — added status to survive a refresh during upload (record
 *        immediately on stop, update on commit success/failure)
 *
 * Storage is per-origin; multiple users sharing a browser profile will each
 * see their own pending clip. Cleared automatically once the clip reports
 * `ready` or after 24 h, whichever comes first.
 */

const KEY = "clipxd:lastClip";
const MAX_AGE_MS = 24 * 60 * 60 * 1000;

export type LastClipStatus = "saving" | "indexing" | "ready" | "failed";

export interface LastClip {
  /** Backend clip id when known. For a still-saving record the
   *  placeholder id is "pending_…" and `status === "saving"`. */
  id: string;
  /** Canonical share URL. May point at the placeholder id; gets rewritten
   *  to the real id once the upload resolves. */
  url: string;
  username: string | null;
  createdAt: number;
  status: LastClipStatus;
  /** Human-readable failure reason — only set when `status === "failed"`. */
  error?: string;
  /** Updated whenever the recorder reports progress. Lets the banner
   *  survive a refresh even mid-upload. */
  updatedAt: number;
}

function read(): LastClip | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as LastClip;
    if (
      typeof parsed?.id !== "string" ||
      typeof parsed?.url !== "string" ||
      typeof parsed?.createdAt !== "number" ||
      typeof parsed?.updatedAt !== "number" ||
      typeof parsed?.status !== "string"
    )
      return null;
    // Auto-prune "saving"-records older than 30 min — clearly stuck.
    if (parsed.status === "saving" && Date.now() - parsed.updatedAt > 30 * 60 * 1000) {
      localStorage.removeItem(KEY);
      return null;
    }
    if (Date.now() - parsed.createdAt > MAX_AGE_MS) {
      localStorage.removeItem(KEY);
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

/** Build the canonical share URL for a clip, matching App.tsx's afterCreate. */
export function shareUrlFor(id: string, username: string | null): string {
  return username
    ? `${location.origin}/u/${username}/clip/${id}`
    : `${location.origin}/?clip=${id}`;
}

/** Record the moment the user clicked Stop. The id is a placeholder —
 *  the server returns a real id later via {@link recordLastClipReady}. */
export function recordLastClipPending(stopId: string, username: string | null): LastClip {
  const id = `pending_${stopId}`;
  const lc: LastClip = {
    id,
    url: shareUrlFor(id, username),
    username,
    createdAt: Date.now(),
    updatedAt: Date.now(),
    status: "saving",
  };
  try { localStorage.setItem(KEY, JSON.stringify(lc)); } catch { /* noop */ }
  window.dispatchEvent(new CustomEvent("clipxd:lastClip", { detail: lc }));
  return lc;
}

/** Update the pending record with the server-issued id (or any new id). */
export function recordLastClipReady(id: string, username: string | null): LastClip {
  const existing = read();
  const lc: LastClip = {
    id,
    url: shareUrlFor(id, username),
    username,
    createdAt: existing?.createdAt ?? Date.now(),
    updatedAt: Date.now(),
    status: "indexing",
  };
  try { localStorage.setItem(KEY, JSON.stringify(lc)); } catch { /* noop */ }
  window.dispatchEvent(new CustomEvent("clipxd:lastClip", { detail: lc }));
  return lc;
}

/** Mark the pending record as ready (server has indexed the clip — its
 *  status has reached `complete` / `partial`). Drops the record so the
 *  banner doesn't keep nagging. */
export function recordLastClipDone(): void {
  localStorage.removeItem(KEY);
  window.dispatchEvent(new CustomEvent("clipxd:lastClip", { detail: null }));
}

/** Mark the pending record as failed and surface the reason to the user. */
export function recordLastClipFailed(reason: string): void {
  const existing = read();
  if (!existing) return;
  const lc: LastClip = { ...existing, status: "failed", error: reason, updatedAt: Date.now() };
  try { localStorage.setItem(KEY, JSON.stringify(lc)); } catch { /* noop */ }
  window.dispatchEvent(new CustomEvent("clipxd:lastClip", { detail: lc }));
}

/** Manual dismiss — banner ✕. */
export function clearLastClip(): void {
  localStorage.removeItem(KEY);
  window.dispatchEvent(new CustomEvent("clipxd:lastClip", { detail: null }));
}

/** Subscribe to local + cross-tab changes. Returns an unsubscribe fn. */
export function onLastClipChange(cb: (next: LastClip | null) => void): () => void {
  const local = () => cb(read());
  window.addEventListener("clipxd:lastClip", local);
  const storage = (e: StorageEvent) => {
    if (e.key === KEY) local();
  };
  window.addEventListener("storage", storage);
  queueMicrotask(local);
  return () => {
    window.removeEventListener("clipxd:lastClip", local);
    window.removeEventListener("storage", storage);
  };
}

export function getLastClip(): LastClip | null {
  return read();
}
