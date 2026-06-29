import { useEffect, useRef, useState } from "react";
import { searchClip } from "./api";
import { fmt, type TextHit } from "./types";
import type { CloudView } from "./App";

interface SearchBoxProps {
  cloudView: CloudView;
  clipId: string | null;
  filter: string;
  onFilter: (q: string) => void;
  onSeek: (t: number) => void;
}

/**
 * The topbar search. On the clip page it runs full-text search over that clip
 * (transcript + OCR + captions) and a hit seeks the video. Elsewhere it filters
 * the library grid by title (via the shared `filter` state).
 */
export function SearchBox({ cloudView, clipId, filter, onFilter, onSeek }: SearchBoxProps) {
  const onClip = cloudView === "clip" && !!clipId;
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<TextHit[]>([]);
  const [open, setOpen] = useState(false);
  const boxRef = useRef<HTMLDivElement>(null);

  // debounce clip search
  useEffect(() => {
    if (!onClip || !clipId || !q.trim()) {
      setHits([]);
      return;
    }
    let cancelled = false;
    const h = window.setTimeout(() => {
      searchClip(clipId, q).then((r) => {
        if (cancelled) return; // a newer clip/query superseded this fetch
        setHits(r.slice(0, 20));
        setOpen(true);
      });
    }, 180);
    return () => {
      cancelled = true;
      window.clearTimeout(h);
    };
  }, [q, clipId, onClip]);

  // close dropdown on outside click
  useEffect(() => {
    const onDoc = (e: MouseEvent) => {
      if (boxRef.current && !boxRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, []);

  const value = onClip ? q : filter;
  const placeholder = onClip ? "Search this clip — transcript, on-screen text, captions…" : "Filter the library by title…";

  return (
    <div className="search" ref={boxRef}>
      <span className="ico" aria-hidden>⌕</span>
      <input
        value={value}
        placeholder={placeholder}
        aria-label={placeholder}
        onChange={(e) => (onClip ? setQ(e.target.value) : onFilter(e.target.value))}
        onFocus={() => onClip && hits.length && setOpen(true)}
      />
      {onClip && open && hits.length > 0 && (
        <div className="search-results">
          {hits.map((h, i) => (
            <button
              type="button"
              key={i}
              className="search-hit"
              onClick={() => {
                onSeek(h.t);
                setOpen(false);
              }}
            >
              <span className="t">{fmt(h.t)}</span>
              <span className="s">{h.stream === "on_screen_text" ? "ocr" : h.stream}</span>
              <span className="x">{h.text}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
