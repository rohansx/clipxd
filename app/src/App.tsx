import { useEffect, useMemo, useState } from "react";
import { clip as sampleClip, fmt, type Clip, type ClipQA } from "./sample";
import { askClip, fetchClip, getConn, type Conn } from "./api";

const MODES = ["Screen", "Window", "Region", "Browser"] as const;

export default function App() {
  const conn = useMemo(getConn, []);
  const [data, setData] = useState<Clip>(sampleClip);
  const [live, setLive] = useState(false);
  const [t, setT] = useState(conn ? 0 : 9.0);
  const [mode, setMode] = useState<(typeof MODES)[number]>("Screen");

  useEffect(() => {
    if (!conn) return;
    fetchClip(conn)
      .then((c) => { setData(c); setLive(true); })
      .catch((e) => console.warn("clipxd-web unreachable, using sample:", e));
  }, [conn]);

  const activeEpisode = useMemo(() => data.episodes.find((e) => t >= e.start && t <= e.end), [data, t]);

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="logo">clip<span className="x">xd</span></span>
          <span className="tagline">Record once. Humans watch it. <b>Agents read it.</b></span>
        </div>
        <div className="modes">
          {MODES.map((m) => (
            <button key={m} className={"mode" + (m === mode ? " on" : "")} onClick={() => setMode(m)}>{m}</button>
          ))}
        </div>
        <span className={"conn " + (live ? "on" : "")}>{live ? "● live" : "○ sample"}</span>
        <button className="record" title="Live capture lands on Mac/Win + Linux PipeWire; this build runs on a file source.">
          <span className="dot" /> Record
        </button>
      </header>

      <main className="stage">
        <section className="editor">
          <Preview clip={data} episode={activeEpisode?.label} />
          <Timeline clip={data} t={t} onSeek={setT} />
          <div className="statusbar">
            <span><b>{data.title}</b></span>
            <span>{data.resolution[0]}×{data.resolution[1]} · {fmt(data.duration)} · source: {data.source}</span>
            <span className="agentic">● {data.events.length} events · {data.onScreenText.length} on-screen text · agent-queryable</span>
          </div>
        </section>
        <Agent clip={data} conn={live ? conn : null} onSeek={setT} />
      </main>
    </div>
  );
}

function Preview({ clip, episode }: { clip: Clip; episode?: string }) {
  const err = clip.onScreenText.find((o) => /error|fail|500/i.test(o.text))?.text ?? "ERROR: Payment failed (500)";
  return (
    <div className="preview">
      <div className={"frame" + (episode ? " zoomed" : "")}>
        <div className="chrome"><i /><i /><i /><span>{clip.source} · {clip.title}</span></div>
        <div className="content">
          <h1>Checkout</h1>
          <div className="total">Total: USD 42.00</div>
          <div className="toast">{err}</div>
        </div>
      </div>
      {episode && <div className="zoom-badge">◎ {episode.slice(0, 48)}</div>}
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
        {clip.episodes.map((ep, i) => (
          <div key={i} className="episode" title={ep.label} style={{ left: pct(ep.start), width: pct(ep.end - ep.start) }} />
        ))}
        {clip.events.map((ev, i) => (
          <div key={i} className={"marker " + ev.kind} title={`${fmt(ev.t)} — ${ev.text}`} style={{ left: pct(ev.t) }} />
        ))}
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
      try {
        const { a, cites } = await askClip(conn, q);
        setAnswer({ q, a, cites });
      } finally { setBusy(false); }
      return;
    }
    const terms = q.toLowerCase().split(/\W+/).filter(Boolean);
    const best = clip.qa
      .map((qa) => ({ qa, score: terms.filter((w) => qa.q.toLowerCase().includes(w)).length }))
      .sort((a, b) => b.score - a.score)[0];
    setAnswer(best && best.score > 0 ? best.qa : { q, a: "No matching content found in this clip's index.", cites: [] });
  };

  // On a live connection, run the default query immediately so the panel shows the REAL answer.
  useEffect(() => { if (conn) void ask(); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [conn]);

  return (
    <aside className="agent">
      <div className="agent-head">
        <span className="badge">MOAT</span>
        <h2>Ask this clip</h2>
        <p>Answered from the index{conn ? " (live, over MCP/HTTP)" : ""} — no pixels, no downloading the video.</p>
      </div>
      <div className="ask">
        <input value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && ask()}
          placeholder="Ask anything about this recording…" />
        <button onClick={ask} disabled={busy}>{busy ? "…" : "Ask"}</button>
      </div>
      {answer && (
        <div className="answer">
          <p>{answer.a}</p>
          {answer.cites.length > 0 && (
            <div className="cites">
              {answer.cites.map((c) => (
                <button key={c} className="cite" onClick={() => onSeek(c)}>↪ {fmt(c)}</button>
              ))}
            </div>
          )}
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
