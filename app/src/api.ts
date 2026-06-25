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

export async function askClip(c: Conn, q: string): Promise<{ a: string; cites: number[] }> {
  const r = await fetch(`${c.api}/clip/${c.id}/query?q=${encodeURIComponent(q)}`);
  const j = await r.json();
  return { a: j.text ?? "(no answer)", cites: j.citations ?? [] };
}
