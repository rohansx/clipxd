/**
 * Per-tab state for "the clip you just made".
 *
 * The previous flow was: stop → flash a 3 s toast → navigate to the clip page.
 * Two things broke:
 *   1. the user often missed the link (toast too brief, no on-page surface),
 *   2. a refresh threw the indexing status away entirely.
 *
 * Fix: persist `{ id, url, createdAt }` to localStorage so we can render it
 * prominently in Recording (a sticky "Link ready" card), in Library (a
 * pinned "your latest clip is still indexing…" banner that follows the user
 * across refreshes), and in ClipPage itself (the indexing pill becomes a
 * banner across the watch body).
 *
 * Storage is per-origin; multiple users sharing a browser profile will each
 * see their own pending clip. Cleared automatically once the clip reports
 * `status: "complete"` (or after 24 h, whichever comes first).
 */

const KEY = "clipxd:lastClip";
const MAX_AGE_MS = 24 * 60 * 60 * 1000;

export interface LastClip {
  id: string;
  url: string;
  username: string | null;
  createdAt: number; // ms epoch
}

function read(): LastClip | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as LastClip;
    if (
      typeof parsed?.id !== "string" ||
      typeof parsed?.url !== "string" ||
      typeof parsed?.createdAt !== "number"
    )
      return null;
    if (Date.now() - parsed.createdAt > MAX_AGE_MS) {
      localStorage.removeItem(KEY);
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

/** Build the canonical share URL for a clip, matching App.tsx's `afterCreate`. */
export function shareUrlFor(id: string, username: string | null): string {
  return username
    ? `${location.origin}/u/${username}/clip/${id}`
    : `${location.origin}/?clip=${id}`;
}

export function recordLastClip(id: string, username: string | null): LastClip {
  const lc: LastClip = { id, url: shareUrlFor(id, username), username, createdAt: Date.now() };
  try { localStorage.setItem(KEY, JSON.stringify(lc)); } catch { /* noop */ }
  // Tell every component on the page so they can re-render.
  window.dispatchEvent(new CustomEvent("clipxd:lastClip", { detail: lc }));
  return lc;
}

export function clearLastClip(): void {
  try { localStorage.removeItem(KEY); } catch { /* noop */ }
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
  // Fire once on subscription so the caller sees the current value
  // after the first paint instead of after the first change.
  queueMicrotask(local);
  return () => {
    window.removeEventListener("clipxd:lastClip", local);
    window.removeEventListener("storage", storage);
  };
}

export function getLastClip(): LastClip | null {
  return read();
}
