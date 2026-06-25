import { useEffect, useMemo, useRef, useState } from "react";
import { clip as sampleClip, fmt, type Clip, type ClipQA } from "./sample";
import { askClip, fetchClip, fetchZoom, getConn, videoUrl, type Conn, type ZoomKeyframe } from "./api";
import { download, editAt, newEdit, newRegion, regionAt, toProject, type EditKind, type EditRegion, type ZoomRegion } from "./regions";
import { RegionTrack } from "./RegionTrack";

const MODES = ["Screen", "Window", "Region", "Browser"] as const;

function kfAt(zoom: ZoomKeyframe[], t: number): ZoomKeyframe | null {
  if (!zoom.length) return null;
  let best = zoom[0];
  for (const k of zoom) if (Math.abs(k.t - t) < Math.abs(best.t - t)) best = k;
  return best;
}

export default function App() {
  const conn = useMemo(getConn, []);
  const [data, setData] = useState<Clip>(sampleClip);
  const [zoom, setZoom] = useState<ZoomKeyframe[]>([]);
  const [live, setLive] = useState(false);
  const [t, setT] = useState(conn ? 0 : 9.0);
  const [mode, setMode] = useState<(typeof MODES)[number]>("Screen");
  const videoRef = useRef<HTMLVideoElement>(null);

  // ── editor regions: manual zoom (override) + cut/speed edits ──
  const [regions, setRegions] = useState<ZoomRegion[]>([]);
  const [edits, setEdits] = useState<EditRegion[]>([]);
  const [history, setHistory] = useState<{ z: ZoomRegion[]; e: EditRegion[] }[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const snapshot = () => setHistory((h) => [...h, { z: regions, e: edits }]);
  const addRegion = () => { snapshot(); setRegions((rs) => [...rs, newRegion(t, 1.5)]); };
  const addEdit = (kind: EditKind) => { snapshot(); setEdits((es) => [...es, newEdit(kind, t, 1.0)]); };
  const del = () => { if (!selected) return; snapshot(); setRegions((rs) => rs.filter((r) => r.id !== selected)); setEdits((es) => es.filter((e) => e.id !== selected)); setSelected(null); };
  const undo = () => setHistory((h) => { if (!h.length) return h; const p = h[h.length - 1]; setRegions(p.z); setEdits(p.e); return h.slice(0, -1); });
  const exportProj = () => download(`${data.id}.clipxd.json`, toProject(data.id, regions, edits));

  // raf reads the video clock and applies edits live: trim → skip the span, speed → ramp rate
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

  useEffect(() => {
    if (!conn) return;
    fetchClip(conn).then((c) => { setData(c); setLive(true); }).catch((e) => console.warn("clipxd-web unreachable:", e));
    fetchZoom(conn).then(setZoom);
  }, [conn]);

  const hasVideo = live && zoom.length > 0;
  const seek = (to: number) => { if (videoRef.current && hasVideo) videoRef.current.currentTime = to; setT(to); };
  const activeEpisode = useMemo(() => data.episodes.find((e) => t >= e.start && t <= e.end), [data, t]);
  const manual = regionAt(regions, t);
  const speed = editAt(edits, t, "speed");

  // ── Record: capture the screen in-browser (getDisplayMedia, works under Wayland via the
  //    portal) → POST the webm to /ingest → server indexes it → reload on the new clip ──
  const apiBase = conn?.api ?? "http://127.0.0.1:8787";
  const [rec, setRec] = useState<"idle" | "recording" | "processing">("idle");
  const recRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  const startRec = async () => {
    try {
      const stream = await navigator.mediaDevices.getDisplayMedia({ video: { frameRate: 30 }, audio: false });
      chunksRef.current = [];
      let mr: MediaRecorder;
      try { mr = new MediaRecorder(stream, { mimeType: "video/webm" }); } catch { mr = new MediaRecorder(stream); }
      mr.ondataavailable = (e) => { if (e.data.size) chunksRef.current.push(e.data); };
      mr.onstop = async () => {
        stream.getTracks().forEach((tr) => tr.stop());
        setRec("processing");
        const blob = new Blob(chunksRef.current, { type: "video/webm" });
        try {
          const r = await fetch(`${apiBase}/ingest`, { method: "POST", headers: { "content-type": "video/webm" }, body: blob });
          const j = await r.json();
          if (j.id) { window.location.href = `${location.pathname}?clip=${j.id}&api=${encodeURIComponent(apiBase)}`; return; }
        } catch (e) { console.warn("ingest failed:", e); }
        setRec("idle");
      };
      stream.getVideoTracks()[0].addEventListener("ended", () => { if (mr.state !== "inactive") mr.stop(); });
      mr.start();
      recRef.current = mr;
      setRec("recording");
    } catch (e) { console.warn("screen capture canceled/failed:", e); setRec("idle"); }
  };
  const stopRec = () => recRef.current?.stop();

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="logo">clip<span className="x">xd</span></span>
          <span className="tagline">Record once. Humans watch it. <b>Agents read it.</b></span>
        </div>
        <div className="modes">
          {MODES.map((m) => <button key={m} className={"mode" + (m === mode ? " on" : "")} onClick={() => setMode(m)}>{m}</button>)}
        </div>
        <span className={"conn " + (live ? "on" : "")}>{live ? (hasVideo ? "● live + auto-zoom" : "● live") : "○ sample"}</span>
        <button
          className={"record" + (rec === "recording" ? " rec-on" : "")}
          onClick={rec === "recording" ? stopRec : rec === "idle" ? startRec : undefined}
          disabled={rec === "processing"}
          title="Capture your screen in the browser → it becomes a queryable clip"
        >
          <span className="dot" /> {rec === "recording" ? "Stop" : rec === "processing" ? "Indexing…" : "Record"}
        </button>
      </header>

      <main className="stage">
        <section className="editor">
          <Preview clip={data} conn={live ? conn : null} zoom={zoom} t={t} manualScale={manual?.scale} speedRate={speed?.rate} videoRef={videoRef} episode={activeEpisode?.label} />
          <div className="toolbar">
            <button onClick={addRegion}>+ Zoom</button>
            <button onClick={() => addEdit("trim")}>✂ Cut</button>
            <button onClick={() => addEdit("speed")}>⏩ Speed</button>
            <button onClick={del} disabled={!selected}>Delete</button>
            <button onClick={undo} disabled={!history.length}>↶ Undo</button>
            <span className="tb-spacer" />
            <button className="export" onClick={exportProj}>⤓ Export .clipxd</button>
          </div>
          <Timeline clip={data} t={t} onSeek={seek} />
          <RegionTrack
            regions={regions} duration={data.duration} selected={selected} laneLabel="manual zoom"
            onSelect={setSelected} onDragStart={snapshot} onChange={setRegions}
            renderLabel={(r) => `⌕ ${r.scale.toFixed(1)}×`}
            hint="“+ Zoom” adds a region at the playhead; drag to move, drag the edge to resize"
          />
          <RegionTrack
            regions={edits} duration={data.duration} selected={selected} laneLabel="cut / speed" minLen={0.3}
            onSelect={setSelected} onDragStart={snapshot} onChange={setEdits}
            renderLabel={(r) => (r.kind === "trim" ? "✂ cut" : `⏩ ${r.rate}× speed`)}
            regionClass={(r) => r.kind}
            hint="“✂ Cut” skips a span on playback; “⏩ Speed” ramps it 2×"
          />
          <div className="statusbar">
            <span><b>{data.title}</b></span>
            <span>{data.resolution[0]}×{data.resolution[1]} · {fmt(data.duration)} · source: {data.source} · {regions.length} zoom · {edits.length} edit region(s)</span>
            <span className="agentic">● {data.events.length} events · {data.onScreenText.length} on-screen text · agent-queryable</span>
          </div>
        </section>
        <Agent clip={data} conn={live ? conn : null} onSeek={seek} />
      </main>
    </div>
  );
}

function Preview({ clip, conn, zoom, t, manualScale, speedRate, videoRef, episode }: {
  clip: Clip; conn: Conn | null; zoom: ZoomKeyframe[]; t: number; manualScale?: number; speedRate?: number;
  videoRef: React.RefObject<HTMLVideoElement>; episode?: string;
}) {
  const hasVideo = conn && zoom.length > 0;
  const kf = kfAt(zoom, t);
  const err = clip.onScreenText.find((o) => /error|fail|500/i.test(o.text))?.text ?? "ERROR: Payment failed (500)";
  const vstyle = manualScale
    ? { transformOrigin: "50% 50%", transform: `scale(${manualScale})` }
    : kf
    ? { transformOrigin: `${kf.cx * 100}% ${kf.cy * 100}%`, transform: `scale(${kf.scale})` }
    : undefined;

  return (
    <div className="preview">
      {hasVideo ? (
        <div className="vwrap"><video ref={videoRef} src={videoUrl(conn!)} controls style={vstyle} /></div>
      ) : (
        <div className={"frame" + (episode || manualScale ? " zoomed" : "")}>
          <div className="chrome"><i /><i /><i /><span>{clip.source} · {clip.title}</span></div>
          <div className="content"><h1>Checkout</h1><div className="total">Total: USD 42.00</div><div className="toast">{err}</div></div>
        </div>
      )}
      {manualScale ? (
        <div className="zoom-badge manual">✎ manual zoom {manualScale.toFixed(1)}×</div>
      ) : kf && kf.scale > 1.05 ? (
        <div className="zoom-badge">◎ {kf.scale.toFixed(1)}× auto-zoom</div>
      ) : null}
      {speedRate ? <div className="speed-badge">⏩ {speedRate}× speed</div> : null}
    </div>
  );
}

function Timeline({ clip, t, onSeek }: { clip: Clip; t: number; onSeek: (t: number) => void }) {
  const dur = clip.duration || 1;
  const pct = (x: number) => `${(x / dur) * 100}%`;
  return (
    <div className="timeline">
      <div className="track" onClick={(e) => {
        const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
        onSeek(((e.clientX - r.left) / r.width) * dur);
      }}>
        {clip.episodes.map((ep, i) => <div key={i} className="episode" title={ep.label} style={{ left: pct(ep.start), width: pct(ep.end - ep.start) }} />)}
        {clip.events.map((ev, i) => <div key={i} className={"marker " + ev.kind} title={`${fmt(ev.t)} — ${ev.text}`} style={{ left: pct(ev.t) }} />)}
        <div className="playhead" style={{ left: pct(t) }} />
      </div>
      <div className="legend">
        <span className="chip ep">auto-zoom / salient</span>
        <span className="chip click">click</span>
        <span className="chip network">network</span>
        <span className="chip console_error">error</span>
        <span className="time">{fmt(t)} / {fmt(dur)}</span>
      </div>
    </div>
  );
}

function Agent({ clip, conn, onSeek }: { clip: Clip; conn: Conn | null; onSeek: (t: number) => void }) {
  const [q, setQ] = useState(clip.qa[0]?.q ?? "what error showed up and what was the user doing right before it");
  const [answer, setAnswer] = useState<ClipQA | null>(clip.qa[0] ?? null);
  const [busy, setBusy] = useState(false);

  const ask = async () => {
    if (conn) {
      setBusy(true);
      try { const { a, cites } = await askClip(conn, q); setAnswer({ q, a, cites }); } finally { setBusy(false); }
      return;
    }
    const terms = q.toLowerCase().split(/\W+/).filter(Boolean);
    const best = clip.qa.map((qa) => ({ qa, score: terms.filter((w) => qa.q.toLowerCase().includes(w)).length })).sort((a, b) => b.score - a.score)[0];
    setAnswer(best && best.score > 0 ? best.qa : { q, a: "No matching content found in this clip's index.", cites: [] });
  };

  useEffect(() => { if (conn) void ask(); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [conn]);

  return (
    <aside className="agent">
      <div className="agent-head">
        <span className="badge">MOAT</span>
        <h2>Ask this clip</h2>
        <p>Answered from the index{conn ? " (live, over MCP/HTTP)" : ""} — no pixels, no downloading the video.</p>
      </div>
      <div className="ask">
        <input value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && ask()} placeholder="Ask anything about this recording…" />
        <button onClick={ask} disabled={busy}>{busy ? "…" : "Ask"}</button>
      </div>
      {answer && (
        <div className="answer">
          <p>{answer.a}</p>
          {answer.cites.length > 0 && <div className="cites">{answer.cites.map((c) => <button key={c} className="cite" onClick={() => onSeek(c)}>↪ {fmt(c)}</button>)}</div>}
        </div>
      )}
      <div className="events">
        <div className="events-title">event track</div>
        {clip.events.slice(0, 12).map((ev, i) => (
          <button key={i} className="event" onClick={() => onSeek(ev.t)}>
            <span className={"k " + ev.kind}>{ev.kind}</span>
            <span className="et">{ev.text}</span>
            <span className="tt">{fmt(ev.t)}</span>
          </button>
        ))}
      </div>
    </aside>
  );
}
