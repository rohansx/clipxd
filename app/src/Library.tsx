import { motion } from "framer-motion";
import { memo, useState } from "react";
import { thumbUrl } from "./api";
import { fmt, type ClipSummary, type ClipSource } from "./types";
import { vMount, vStagger, usePrefersReducedMotion } from "./motion";

interface LibraryProps {
  clips: ClipSummary[] | null;
  filter: string;
  onOpen: (id: string) => void;
  onPasteImport: (url?: string) => void;
}

const SOURCE_TINT: Record<ClipSource, string> = {
  browser: "linear-gradient(135deg,#5FD3B2,#2E8C8A)",
  screen: "linear-gradient(135deg,#FF9E7D,#C9618A)",
  import: "linear-gradient(135deg,#A99BFF,#6E6FB0)",
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

/** A single clip card. Memoised so the grid stays snappy while filters apply or
 *  unrelated clips are hovered. */
const ClipCard = memo(function ClipCard({ c, onOpen }: { c: ClipSummary; onOpen: (id: string) => void }) {
  const [thumbOk, setThumbOk] = useState(true);
  return (
    <motion.button
      className="clip-card lift"
      onClick={() => onOpen(c.id)}
      whileHover={{ y: -4 }}
      transition={{ type: "spring", stiffness: 320, damping: 26 }}
    >
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
    </motion.button>
  );
});

export function Library({ clips, filter, onOpen, onPasteImport }: LibraryProps) {
  const reduced = usePrefersReducedMotion();
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
            <button
              type="button"
              key={f}
              className={"filter-pill" + (src === f ? " on" : "")}
              aria-pressed={src === f}
              onClick={() => setSrc(f)}
            >
              {f === "all" ? "All" : f[0].toUpperCase() + f.slice(1)}
            </button>
          ))}
        </div>
      </div>

      <div className="paste-bar">
        <span className="lead">paste link ↓</span>
        <input
          value={pasteUrl}
          onChange={(e) => setPasteUrl(e.target.value)}
          placeholder="loom.com/share/… · cap.so/… · youtube.com/… · any video link"
          onKeyDown={(e) => e.key === "Enter" && onPasteImport(pasteUrl.trim() || undefined)}
        />
        <button
          className="btn-signal btn-pill"
          onClick={() => onPasteImport(pasteUrl.trim() || undefined)}
        >
          Read it
        </button>
      </div>

      {clips == null && <div className="empty">loading clips…</div>}
      {clips != null && shown.length === 0 && (
        <div className="empty">
          {clips.length === 0
            ? "No clips yet — hit Record or Import to make one."
            : "No clips match that filter."}
        </div>
      )}
      {shown.length > 0 && (
        <motion.div
          className="clip-grid"
          variants={vStagger(0.04)}
          initial={reduced ? false : "hidden"}
          animate="shown"
          key={src + ":" + filter}
        >
          {shown.map((c) => (
            <motion.div key={c.id} variants={vMount}>
              <ClipCard c={c} onOpen={onOpen} />
            </motion.div>
          ))}
        </motion.div>
      )}
    </div>
  );
}
