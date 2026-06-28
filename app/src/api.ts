// Live data from clipxd-web. Open the app as `?clip=<id>&api=<base>` to load a real clip
// (default api http://localhost:8787); with no `clip` param it falls back to sample data.

import type { Clip } from "./sample";

export type Conn = { api: string; id: string };

export function getConn(): Conn | null {
  const u = new URL(location.href);
  const id = u.searchParams.get("clip");
  if (!id) return null;
  return { api: u.searchParams.get("api") || "http://localhost:8787", id };
}

export async function fetchClip(c: Conn): Promise<Clip> {
  const r = await fetch(`${c.api}/clip/${c.id}/index.json`);
  if (!r.ok) throw new Error(`clip ${c.id}: HTTP ${r.status}`);
  const idx = await r.json();
  return {
    id: idx.id,
    title: idx.metadata?.title ?? c.id,
    source: idx.source,
    duration: idx.metadata?.duration ?? 0,
    resolution: idx.metadata?.resolution ?? [1280, 720],
    episodes: (idx.visual_timeline ?? []).map((m: any) => ({
      start: Math.max(0, m.t - 0.3),
      end: m.t + 0.4,
      label: m.caption,
    })),
    events: (idx.event_track ?? []).map((e: any) => ({ t: e.t, kind: e.kind, text: e.text ?? e.kind })),
    onScreenText: (idx.on_screen_text ?? []).map((o: any) => ({ t: o.start, text: o.text })),
    transcript: (idx.transcript ?? []).map((s: any) => ({ start: s.start, end: s.end, text: s.text })),
    qa: [],
  };
}

export type ClipSummary = {
  id: string;
  title: string;
  source: string;
  duration: number;
  counts: { events: number; on_screen_text: number; transcript: number; visual: number };
};

export async function fetchClips(apiBase: string): Promise<ClipSummary[]> {
  try {
    const r = await fetch(`${apiBase}/clips`);
    if (!r.ok) return [];
    const j = await r.json();
    return (j.clips ?? []).map((c: any) => ({
      id: c.id,
      title: c.metadata?.title ?? c.id,
      source: c.source,
      duration: c.metadata?.duration ?? 0,
      counts: c.counts ?? { events: 0, on_screen_text: 0, transcript: 0, visual: 0 },
    }));
  } catch {
    return [];
  }
}

export type ZoomKeyframe = { t: number; scale: number; cx: number; cy: number };

export async function fetchZoom(c: Conn): Promise<ZoomKeyframe[]> {
  try {
    const r = await fetch(`${c.api}/clip/${c.id}/zoom.json`);
    return r.ok ? await r.json() : [];
  } catch {
    return [];
  }
}

export function videoUrl(c: Conn): string {
  return `${c.api}/clip/${c.id}/video`;
}

/// A shareable link to this clip's watch+ask page. Asks the server which base to use: the public
/// tunnel origin (CLIPXD_PUBLIC_BASE, e.g. a Tailscale-Funnel https URL) if one is active, else
/// the LAN base (so it at least works for others on the network, not the 127.0.0.1 the editor was
/// opened with). Falls back to the api base if the server can't report one.
export async function shareLink(c: Conn): Promise<string> {
  try {
    const r = await fetch(`${c.api}/net`);
    if (r.ok) {
      const j = await r.json();
      const base = (typeof j.public_base === "string" && j.public_base)
        ? j.public_base
        : (typeof j.share_base === "string" && j.share_base ? j.share_base : null);
      if (base) return `${base}/clip/${c.id}`;
    }
  } catch {
    // fall through to the api base
  }
  return `${c.api}/clip/${c.id}`;
}

export async function askClip(c: Conn, q: string): Promise<{ a: string; cites: number[] }> {
  const r = await fetch(`${c.api}/clip/${c.id}/query?q=${encodeURIComponent(q)}`);
  const j = await r.json();
  return { a: j.text ?? "(no answer)", cites: j.citations ?? [] };
}
