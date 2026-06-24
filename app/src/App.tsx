import { useMemo, useState } from "react";
import { clip, fmt, type ClipQA } from "./sample";

const MODES = ["Screen", "Window", "Region", "Browser"] as const;

export default function App() {
  const [t, setT] = useState(9.0);
  const [mode, setMode] = useState<(typeof MODES)[number]>("Screen");

  const activeEpisode = useMemo(
    () => clip.episodes.find((e) => t >= e.start && t <= e.end),
    [t],
  );

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="logo">clip<span className="x">xd</span></span>
          <span className="tagline">Record once. Humans watch it. <b>Agents read it.</b></span>
        </div>
        <div className="modes">
          {MODES.map((m) => (
            <button key={m} className={"mode" + (m === mode ? " on" : "")} onClick={() => setMode(m)}>
              {m}
            </button>
          ))}
        </div>
        <button className="record" title="Live capture lands on Mac/Win + Linux PipeWire; this build runs on a file source.">
          <span className="dot" /> Record
        </button>
      </header>

      <main className="stage">
        <section className="editor">
          <Preview episode={activeEpisode?.label} />
          <Timeline t={t} onSeek={setT} />
          <div className="statusbar">
            <span><b>{clip.title}</b></span>
            <span>{clip.resolution[0]}×{clip.resolution[1]} · {fmt(clip.duration)} · source: {clip.source}</span>
            <span className="agentic">● {clip.events.length} events · {clip.onScreenText.length} on-screen text · agent-queryable</span>
          </div>
        </section>

        <Agent onSeek={setT} />
      </main>
    </div>
  );
}

function Preview({ episode }: { episode?: string }) {
  return (
    <div className="preview">
      <div className={"frame" + (episode ? " zoomed" : "")}>
        <div className="chrome"><i /><i /><i /><span>app.example.com/checkout</span></div>
        <div className="content">
          <h1>Checkout</h1>
          <div className="total">Total: USD 42.00</div>
          <div className="toast">ERROR: Payment failed (500)</div>
        </div>
      </div>
      {episode && <div className="zoom-badge">◎ {episode}</div>}
    </div>
  );
}

function Timeline({ t, onSeek }: { t: number; onSeek: (t: number) => void }) {
  const pct = (x: number) => `${(x / clip.duration) * 100}%`;
  return (
    <div className="timeline">
      <div className="track" onClick={(e) => {
        const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
        onSeek(((e.clientX - r.left) / r.width) * clip.duration);
      }}>
        {clip.episodes.map((ep, i) => (
          <div key={i} className="episode" title={ep.label}
            style={{ left: pct(ep.start), width: pct(ep.end - ep.start) }} />
        ))}
        {clip.events.map((ev, i) => (
          <div key={i} className={"marker " + ev.kind} title={`${fmt(ev.t)} — ${ev.text}`} style={{ left: pct(ev.t) }} />
        ))}
        <div className="playhead" style={{ left: pct(t) }} />
      </div>
      <div className="legend">
        <span className="chip ep">auto-zoom</span>
        <span className="chip click">click</span>
        <span className="chip network">network</span>
        <span className="chip console_error">error</span>
        <span className="time">{fmt(t)} / {fmt(clip.duration)}</span>
      </div>
    </div>
  );
}

function Agent({ onSeek }: { onSeek: (t: number) => void }) {
  const [q, setQ] = useState(clip.qa[0].q);
  const [answer, setAnswer] = useState<ClipQA | null>(clip.qa[0]);

  const ask = () => {
    const terms = q.toLowerCase().split(/\W+/).filter(Boolean);
    const best = clip.qa
      .map((qa) => ({ qa, score: terms.filter((w) => qa.q.toLowerCase().includes(w)).length }))
      .sort((a, b) => b.score - a.score)[0];
    setAnswer(best && best.score > 0 ? best.qa : { q, a: "No matching content found in this clip's index.", cites: [] });
  };

  return (
    <aside className="agent">
      <div className="agent-head">
        <span className="badge">MOAT</span>
        <h2>Ask this clip</h2>
        <p>Answered from the index — no pixels, no downloading the video.</p>
      </div>
      <div className="ask">
        <input value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && ask()}
          placeholder="Ask anything about this recording…" />
        <button onClick={ask}>Ask</button>
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
        {clip.events.map((ev, i) => (
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
