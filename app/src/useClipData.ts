import { useEffect, useState } from "react";
import { fetchClips, fetchIndex, fetchZoom } from "./api";
import type { ClipSummary, Index, ZoomKeyframe } from "./types";

/** The library list. `reload()` re-fetches (after a record/import). While any clip is still
 *  `enriching`, it polls every 3s so cards flip from "indexing…" to indexed on their own. */
export function useClips(): { clips: ClipSummary[] | null; reload: () => void } {
  const [clips, setClips] = useState<ClipSummary[] | null>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    let live = true;
    let poll: number | undefined;
    const tick = () => {
      fetchClips().then((c) => {
        if (!live) return;
        setClips(c);
        const enriching = c.some((x) => x.status === "enriching");
        if (enriching && poll === undefined) poll = window.setInterval(tick, 3000);
        else if (!enriching && poll !== undefined) {
          window.clearInterval(poll);
          poll = undefined;
        }
      });
    };
    tick();
    return () => {
      live = false;
      if (poll !== undefined) window.clearInterval(poll);
    };
  }, [n]);
  return { clips, reload: () => setN((x) => x + 1) };
}

export interface ClipData {
  index: Index | null;
  zoom: ZoomKeyframe[];
  loading: boolean;
  error: string | null;
}

/**
 * One clip's full index + zoom track, fetched whenever `id` changes. While the clip is still
 * `enriching` (async ingest — video saved, index filling in), it re-polls every 2.5s so the
 * transcript / OCR / captions appear live without a manual refresh.
 */
export function useClip(id: string | null): ClipData {
  const [data, setData] = useState<ClipData>({ index: null, zoom: [], loading: !!id, error: null });
  useEffect(() => {
    if (!id) {
      setData({ index: null, zoom: [], loading: false, error: null });
      return;
    }
    let live = true;
    let poll: number | undefined;
    // Drop the previous clip's data immediately so the loading guard (`loading && !index`)
    // engages and we never paint clip A's index over clip B's video.
    setData({ index: null, zoom: [], loading: true, error: null });

    const load = (first: boolean) => {
      Promise.all([fetchIndex(id), fetchZoom(id)])
        .then(([index, zoom]) => {
          if (!live) return;
          setData({ index, zoom, loading: false, error: null });
          if (index.status === "enriching" && poll === undefined) {
            poll = window.setInterval(() => load(false), 2500);
          } else if (index.status !== "enriching" && poll !== undefined) {
            window.clearInterval(poll);
            poll = undefined;
          }
        })
        .catch((e: unknown) => {
          if (live && first) setData({ index: null, zoom: [], loading: false, error: e instanceof Error ? e.message : "failed to load clip" });
        });
    };
    load(true);

    return () => {
      live = false;
      if (poll !== undefined) window.clearInterval(poll);
    };
  }, [id]);
  return data;
}
