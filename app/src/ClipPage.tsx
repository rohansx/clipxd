import { useEffect, useRef, useState } from "react";
import { useClip } from "./useClipData";
import { queryClip, renderClip, downloadBlob, shareLink, apiBase, reEnrichClip } from "./api";
import { fmt, type QueryAnswer, type Index } from "./types";
import { editAt, newEdit, newRegion, regionAt, toProject, type EditKind, type EditRegion, type ZoomRegion } from "./regions";
import { Seo } from "./seo";
import { getLastClip, onLastClipChange, type LastClip } from "./lastClip";

/** The clip's phase-2 enrich produced no semantic annotations. The cap/OCR/transcriber
 *  was offline at recording time — surface a re-enrich CTA instead of looking "done". */
function emptyIndex(i: Index): boolean {
  return (i.transcript?.length ?? 0) === 0
      && (i.on_screen_text?.length ?? 0) === 0
      && (i.visual_timeline?.length ?? 0) === 0;
}
import { WatchBody } from "./WatchBody";
import { ReadBody } from "./ReadBody";
import { ShareModal } from "./ShareModal";
import type { SeekRequest } from "./App";

type Seam = "watch" | "split" | "read";

const GRID_COLS: Record<Seam, string> = {
  watch: "2fr 0.82fr",
  split: "1.25fr 1fr",
  read: "0.72fr 1.5fr",
};

interface ClipPageProps {
  id: string | null;
  seekTo: SeekRequest | null;
  showToast: (m: string) => void;
}

export function ClipPage({ id, seekTo, showToast }: ClipPageProps) {
  const { index, zoom, loading, error } = useClip(id);
  const videoRef = useRef<HTMLVideoElement>(null);
  const [t, setT] = useState(0);
  const [seam, setSeam] = useState<Seam>("split");
  const [developing, setDeveloping] = useState(false);

  // editor regions: manual zoom (overrides auto) + cut/speed edits, with undo history
  const [regions, setRegions] = useState<ZoomRegion[]>([]);
  const [edits, setEdits] = useState<EditRegion[]>([]);
  const [history, setHistory] = useState<{ z: ZoomRegion[]; e: EditRegion[] }[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const snapshot = () => setHistory((h) => [...h, { z: regions, e: edits }]);

  const [bg, setBg] = useState("aurora");
  const [rendering, setRendering] = useState(false);
  const [answer, setAnswer] = useState<QueryAnswer | null>(null);
  const [asking, setAsking] = useState(false);
  const [q, setQ] = useState("what happens in this clip and what's the key moment");
  const [shareUrl, setShareUrl] = useState<string | null>(null);

  const dur = index?.metadata.duration ?? 0;
  const hasVideo = !!index?.metadata.has_video;

  // "develop" scan sweep whenever a new clip opens
  useEffect(() => {
    if (!id) return;
    setDeveloping(true);
    setRegions([]);
    setEdits([]);
    setHistory([]);
    setSelected(null);
    setAnswer(null);
    setT(0);
    const h = window.setTimeout(() => setDeveloping(false), 1500);
    return () => window.clearTimeout(h);
  }, [id]);

  // rAF: read the video clock; apply trim (skip span) + speed (ramp rate) edits live
  const editsRef = useRef<EditRegion[]>([]);
  editsRef.current = edits;
  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const v = videoRef.current;
      if (v) {
        const es = editsRef.current;
        if (!v.paused) {
          const trim = es.find((e) => e.kind === "trim" && v.currentTime >= e.start && v.currentTime < e.end);
          if (trim) v.currentTime = trim.end;
        }
        const sp = es.find((e) => e.kind === "speed" && v.currentTime >= e.start && v.currentTime <= e.end);
        const rate = sp ? sp.rate : 1;
        if (v.playbackRate !== rate) v.playbackRate = rate;
        setT(v.currentTime);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, []);

  const seek = (to: number) => {
    if (videoRef.current && hasVideo) videoRef.current.currentTime = to;
    setT(to);
  };

  // topbar-search seek requests
  useEffect(() => {
    if (seekTo) seek(seekTo.t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [seekTo?.nonce]);

  const ask = async (question = q) => {
    if (!id || !question.trim()) return;
    setAsking(true);
    try {
      setAnswer(await queryClip(id, question));
    } catch {
      showToast("Couldn't reach the agent — is the backend running?");
    } finally {
      setAsking(false);
    }
  };

  const doRender = async () => {
    if (!id) return;
    setRendering(true);
    try {
      const blob = await renderClip(id, { format: "mp4", mockup: true, bg, project: toProject(id, regions, edits) });
      downloadBlob(`${id}.mp4`, blob);
      showToast("Rendered video downloaded");
    } catch {
      showToast("Render failed — is the backend running?");
    } finally {
      setRendering(false);
    }
  };

  const onShare = async () => {
    if (!id) return;
    const url = await shareLink(id);
    setShareUrl(url);
    try {
      await navigator.clipboard.writeText(url);
      showToast("Share link copied");
    } catch {
      /* modal still shows the URL + QR */
    }
  };

  const copyIndex = async () => {
    if (!index) return;
    await navigator.clipboard.writeText(JSON.stringify(index, null, 2)).catch(() => {});
    showToast("Index copied for an agent");
  };
  const copyMcp = async () => {
    if (!id) return;
    await navigator.clipboard.writeText(`${apiBase()}/clip/${id}/index.json`).catch(() => {});
    showToast("MCP/index URL copied");
  };
  const downloadJson = () => {
    if (!index || !id) return;
    downloadBlob(`${id}.index.json`, new Blob([JSON.stringify(index, null, 2)], { type: "application/json" }));
    showToast("clip.json downloaded");
  };

  // Track "is this the clip you just made?" so the indexing banner can
  // show across the watch body.  Three signals count:
  //   1. Localstorage has an unsaved entry (`pending_*`) — server hasn't
  //      committed yet, so `id` won't match.  We trust the local status.
  //   2. Localstorage has a saved id matching the URL — server committed.
  //   3. The server says the clip is still enriching.
  // The banner mounts if ANY of these hold.
  //
  // ⚠️  Hooks must be called in the same order on every render — do NOT
  // put them after the conditional `return`s below.  Previously the
  // `useState`/`useEffect` for `lastClip` lived after these returns, and
  // they skipped on the first render (when the URL+id was set but
  // `loading && !index` short-circuited the JSX) then re-ran on the second
  // render (`loading` became false).  React's Strict Mode sees the
  // out-of-order calls and refuses to render the component at all.  Both
  // `useState` for `lastClip` and `useEffect` for the listener belong up
  // here, above the returns.
  const [lastClip, setLastClip] = useState<LastClip | null>(getLastClip);
  useEffect(() => onLastClipChange(setLastClip), []);

  if (!id) return <div className="view"><div className="empty">No clip selected.</div></div>;
  if (loading && !index) return <div className="view"><div className="empty"><span className="spin" /> developing the index…</div></div>;
  if (error || !index) return <div className="view"><div className="empty">Couldn't load this clip — {error ?? "unknown error"}.</div></div>;

  const manual = regionAt(regions, t);
  const speed = editAt(edits, t, "speed");

  const justRecorded =
    !!lastClip && (
      lastClip.status === "saving" ||
      (lastClip.id === id && index.status === "enriching")
    );

  return (
    <div className="clip-page">
      {index && (
        <Seo
          title={index.metadata.title || "Clip"}
          description={`Watch the recording and ask the agent about it. clip: ${id}.`}
          path={`/clip/${id}`}
          noindex
          jsonLd={{
            "@context": "https://schema.org",
            "@type": "VideoObject",
            name: index.metadata.title || "Clip",
            description: index.summary.tldr || "A clip on clipxd",
            uploadDate: index.metadata.created_at,
            duration: `PT${Math.max(1, Math.round(index.metadata.duration))}S`,
            thumbnailUrl: `/clip/${id}/frames/00001.png`,
            contentUrl: `/clip/${id}/video`,
            encodingFormat: "video/webm",
            isAccessibleForFree: true,
            publisher: { "@type": "Organization", name: "clipxd", url: "https://clipxd.com/" },
          }}
        />
      )}
      {justRecorded && (
        <div className="clip-indexing-banner" role="status" aria-live="polite">
          <span className="spin" />
          <div>
            <b>Indexing this clip…</b>
            <span>building transcript, OCR, captions, and event track — this updates live, no need to refresh.</span>
          </div>
        </div>
      )}
      <div className="clip-titlebar">
        <h1>{index.metadata.title || id}</h1>
        <span className="clip-url mono">/clip/{id}</span>
        {index.status === "enriching" && (
          <span className="pill" style={{ color: "var(--sodium-text)", borderColor: "color-mix(in oklab,var(--sodium) 40%,transparent)" }} title="The video is ready and shareable now; transcript, on-screen text and captions are still being built and will appear automatically.">
            <span className="spin" style={{ width: 10, height: 10 }} /> indexing…
          </span>
        )}
        {index.status === "complete" && emptyIndex(index) && (
          <button
            className="pill pill-reenrich"
            onClick={async () => {
              try {
                await reEnrichClip(id);
                showToast("Re-enriching — transcript / OCR / captions will appear when ready");
              } catch (e) {
                showToast(e instanceof Error ? e.message : "re-enrich failed");
              }
            }}
            title="Captions are empty (the captioner / OCR / transcriber were offline when this clip was processed). Click to run them again."
          >
            <span className="dot" style={{ background: "var(--text-3)" }} /> captions empty — re-enrich ↻
          </button>
        )}
        <div className="seam-toggle">
          <button className={seam === "watch" ? "on-watch" : ""} onClick={() => setSeam("watch")}>
            <span className="dot sodium" />
            Watch
          </button>
          <button className={seam === "split" ? "on-split" : ""} onClick={() => setSeam("split")} aria-label="Split view" aria-pressed={seam === "split"}>
            <span aria-hidden>⟷</span>
          </button>
          <button className={seam === "read" ? "on-read" : ""} onClick={() => setSeam("read")}>
            <span className="dot signal" />
            Read
          </button>
        </div>
        <button className="btn-share-pill" onClick={onShare}>
          Share link
        </button>
      </div>

      <div className="dual-body" style={{ gridTemplateColumns: GRID_COLS[seam] }}>
        <WatchBody
          id={id}
          index={index}
          zoom={zoom}
          t={t}
          dur={dur}
          hasVideo={hasVideo}
          videoRef={videoRef}
          developing={developing}
          manualScale={manual?.scale}
          speedRate={speed?.rate}
          seek={seek}
          regions={regions}
          edits={edits}
          selected={selected}
          setSelected={setSelected}
          setRegions={setRegions}
          setEdits={setEdits}
          snapshot={snapshot}
          addRegion={() => {
            snapshot();
            setRegions((rs) => [...rs, newRegion(t, 1.5)]);
          }}
          addEdit={(k: EditKind) => {
            snapshot();
            setEdits((es) => [...es, newEdit(k, t, 1.0)]);
          }}
          del={() => {
            if (!selected) return;
            snapshot();
            setRegions((rs) => rs.filter((r) => r.id !== selected));
            setEdits((es) => es.filter((e) => e.id !== selected));
            setSelected(null);
          }}
          undo={() =>
            setHistory((h) => {
              if (!h.length) return h;
              const p = h[h.length - 1];
              setRegions(p.z);
              setEdits(p.e);
              return h.slice(0, -1);
            })
          }
          canUndo={history.length > 0}
          bg={bg}
          setBg={setBg}
          rendering={rendering}
          doRender={doRender}
          exportProject={() =>
            downloadBlob(`${id}.clipxd.json`, new Blob([JSON.stringify(toProject(id, regions, edits), null, 2)], { type: "application/json" }))
          }
        />
        <ReadBody index={index} t={t} seek={seek} />
      </div>

      <div className="agent-rail">
        <span className="led-on" style={{ flex: "none" }} />
        <div className="ask">
          <input
            className="input"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && ask()}
            placeholder="Ask an agent about this clip…"
          />
          <button className="btn-signal" onClick={() => ask()} disabled={asking} style={{ borderRadius: 0 }}>
            {asking ? <span className="spin" /> : "Ask"}
          </button>
        </div>
        <button className="btn-mono" onClick={copyIndex}>
          Copy index
        </button>
        <button className="btn-mono" onClick={copyMcp}>
          MCP url
        </button>
        <button className="btn-mono" onClick={downloadJson}>
          ⤓ clip.json
        </button>
      </div>

      {answer && (
        <div className="agent-answer">
          <p>{answer.text}</p>
          {answer.citations.length > 0 && (
            <div className="cites">
              <span className="lead">grounded in:</span>
              {answer.citations.map((c, i) => (
                <button key={i} className="cite" onClick={() => seek(c)}>
                  ↪ {fmt(c)}
                </button>
              ))}
              <span className="lead" style={{ marginLeft: 4 }}>
                watched_video: <span style={{ color: "var(--danger)" }}>false</span>
              </span>
            </div>
          )}
        </div>
      )}

      {shareUrl && <ShareModal url={shareUrl} onClose={() => setShareUrl(null)} />}
    </div>
  );
}
