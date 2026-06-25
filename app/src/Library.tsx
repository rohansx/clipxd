import { useEffect, useState } from "react";
import { fetchClips, type ClipSummary } from "./api";
import { fmt } from "./sample";

// A grid of every recording in the clips dir — thumbnails + what's indexed in each (transcript
// / on-screen text / salient moments). Click a card to open that clip in the editor.
export function Library({ apiBase, currentId, onClose }: { apiBase: string; currentId?: string; onClose: () => void }) {
  const [clips, setClips] = useState<ClipSummary[] | null>(null);
  useEffect(() => { fetchClips(apiBase).then(setClips); }, [apiBase]);

  const open = (id: string) => { window.location.href = `${location.pathname}?clip=${id}&api=${encodeURIComponent(apiBase)}`; };

  return (
    <div className="lib-overlay" onClick={onClose}>
      <div className="lib" onClick={(e) => e.stopPropagation()}>
        <div className="lib-head">
          <b>Library</b>
          <span>{clips ? `${clips.length} clip${clips.length === 1 ? "" : "s"}` : "loading…"}</span>
          <span className="tb-spacer" />
          <button onClick={onClose}>✕</button>
        </div>
        <div className="lib-grid">
          {(clips ?? []).map((c) => (
            <button key={c.id} className={"lib-card" + (c.id === currentId ? " on" : "")} onClick={() => open(c.id)}>
              <div className="lib-thumb">
                <img src={`${apiBase}/clip/${c.id}/frames/00001.png`} alt="" loading="lazy"
                  onError={(e) => ((e.currentTarget as HTMLImageElement).style.opacity = "0")} />
              </div>
              <div className="lib-meta">
                <div className="lib-title">{c.title}</div>
                <div className="lib-sub">{c.source} · {fmt(c.duration)}</div>
                <div className="lib-tags">
                  <span title="transcript segments">🗣 {c.counts.transcript}</span>
                  <span title="on-screen text">📝 {c.counts.on_screen_text}</span>
                  <span title="salient moments">◎ {c.counts.visual}</span>
                </div>
              </div>
            </button>
          ))}
          {clips && !clips.length && <div className="lib-empty">No clips yet — hit Record to make one.</div>}
        </div>
      </div>
    </div>
  );
}
