import { useState } from "react";
import { thumbUrl } from "./api";
import { fmt, type ClipSummary, type ClipSource } from "./types";

interface LibraryProps {
  clips: ClipSummary[] | null;
  filter: string;
  onOpen: (id: string) => void;
  onPasteImport: (url?: string) => void;
}

const SOURCE_TINT: Record<ClipSource, string> = {
  browser: "#42427E",
  screen: "#1E6360",
  import: "#6E3A4E",
};

function relTime(created: string): string {
  const secs = Number(created);
  if (!Number.isFinite(secs) || secs <= 0) return "indexed";
  const ageMs = Date.now() - secs * 1000;
  const mins = Math.round(ageMs / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return `${Math.round(hrs / 24)}d ago`;
}

function badges(c: ClipSummary): string[] {
  const b: string[] = [];
  if (c.counts.transcript > 0) b.push("transcript");
  if (c.counts.on_screen_text > 0) b.push(`ocr·${c.counts.on_screen_text}`);
  if (c.counts.events > 0) b.push(`events·${c.counts.events}`);
  if (c.counts.visual > 0) b.push(`scenes·${c.counts.visual}`);
  return b;
}

function ClipCard({ c, onOpen }: { c: ClipSummary; onOpen: (id: string) => void }) {
  const [thumbOk, setThumbOk] = useState(true);
  return (
    <button className="clip-card lift" onClick={() => onOpen(c.id)}>
      <div className="clip-thumb" style={{ background: SOURCE_TINT[c.source] ?? "#241f2b" }}>
        {thumbOk && <img src={thumbUrl(c.id)} alt="" onError={() => setThumbOk(false)} loading="lazy" />}
        <div className="play">▶</div>
        <span className="src">{c.source}</span>
        <span className="dur">{fmt(c.metadata.duration)}</span>
      </div>
      <div className="clip-body">
        <div className="clip-name">{c.metadata.title || c.id}</div>
        <div className="clip-meta">
          {c.status === "enriching" ? (
            <>
              <span className="spin" style={{ width: 9, height: 9 }} />
              <span style={{ color: "var(--sodium-text)" }}>indexing…</span>
            </>
          ) : (
            <>
              <span className="ok">●</span>indexed
            </>
          )}
          <span>·</span>
          <span>{relTime(c.metadata.created_at)}</span>
        </div>
        <div className="clip-badges">
          {badges(c).map((b) => (
            <span key={b} className="clip-badge">
              {b}
            </span>
          ))}
        </div>
      </div>
    </button>
  );
}

export function Library({ clips, filter, onOpen, onPasteImport }: LibraryProps) {
  const [src, setSrc] = useState<"all" | ClipSource>("all");
  const [pasteUrl, setPasteUrl] = useState("");
  const filters: ("all" | ClipSource)[] = ["all", "browser", "screen", "import"];

  const shown = (clips ?? [])
    .filter((c) => src === "all" || c.source === src)
    .filter((c) => !filter.trim() || (c.metadata.title || c.id).toLowerCase().includes(filter.toLowerCase()));

  return (
    <div className="view">
      <div className="view-head">
        <div>
          <h1 className="view-title">Library</h1>
          <p className="view-sub">
            {clips == null ? "loading…" : `${clips.length} clips · every one queryable from its link`}
          </p>
        </div>
        <div className="filters">
          {filters.map((f) => (
            <button type="button" key={f} className={"filter" + (src === f ? " on" : "")} aria-pressed={src === f} onClick={() => setSrc(f)}>
              {f === "all" ? "All" : f[0].toUpperCase() + f.slice(1)}
            </button>
          ))}
        </div>
      </div>

      <div className="import-bar">
        <span className="lead">paste link ↓</span>
        <input
          value={pasteUrl}
          onChange={(e) => setPasteUrl(e.target.value)}
          placeholder="loom.com/share/… · cap.so/… · youtube.com/… · any video link"
          onKeyDown={(e) => e.key === "Enter" && onPasteImport(pasteUrl.trim() || undefined)}
        />
        <button className="btn-signal" onClick={() => onPasteImport(pasteUrl.trim() || undefined)} style={{ borderRadius: 0 }}>
          Read it
        </button>
      </div>

      <div className="clip-grid">
        {clips == null && <div className="empty">loading clips…</div>}
        {clips != null && shown.length === 0 && (
          <div className="empty">
            {clips.length === 0
              ? "No clips yet — hit Record or Import to make one."
              : "No clips match that filter."}
          </div>
        )}
        {shown.map((c) => (
          <ClipCard key={c.id} c={c} onOpen={onOpen} />
        ))}
      </div>
    </div>
  );
}
