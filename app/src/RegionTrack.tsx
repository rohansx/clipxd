import { useRef } from "react";
import type { Seg } from "./regions";

// A timeline lane of draggable segments: drag a block to move it, drag its right edge to
// resize, click to select. Generic over any {id,start,end} so it serves both the manual-zoom
// lane and the trim/speed lane. Immutable — every change calls onChange with a new array
// (the parent snapshots for undo).
export function RegionTrack<T extends Seg>({
  regions, duration, selected, laneLabel, minLen = 0.2, onSelect, onDragStart, onChange, renderLabel, regionClass, hint,
}: {
  regions: T[];
  duration: number;
  selected: string | null;
  laneLabel: string;
  minLen?: number;
  onSelect: (id: string | null) => void;
  onDragStart: () => void;
  onChange: (rs: T[]) => void;
  renderLabel: (r: T) => string;
  regionClass?: (r: T) => string;
  hint?: string;
}) {
  const drag = useRef<{ id: string; mode: "move" | "resize"; x0: number; r0: T } | null>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const dur = duration || 1;
  const pct = (x: number) => `${(x / dur) * 100}%`;

  const onDown = (e: React.PointerEvent, r: T, mode: "move" | "resize") => {
    e.stopPropagation();
    (e.target as Element).setPointerCapture(e.pointerId);
    onDragStart(); // snapshot once per drag for undo
    drag.current = { id: r.id, mode, x0: e.clientX, r0: { ...r } };
    onSelect(r.id);
  };

  const onMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d || !trackRef.current) return;
    const w = trackRef.current.getBoundingClientRect().width || 1;
    const dt = ((e.clientX - d.x0) / w) * dur;
    onChange(regions.map((r) => {
      if (r.id !== d.id) return r;
      if (d.mode === "move") {
        const len = d.r0.end - d.r0.start;
        const s = Math.max(0, Math.min(dur - len, d.r0.start + dt));
        return { ...r, start: s, end: s + len };
      }
      return { ...r, end: Math.max(d.r0.start + minLen, Math.min(dur, d.r0.end + dt)) };
    }));
  };

  const onUp = () => { drag.current = null; };

  return (
    <div className="regiontrack" ref={trackRef} onPointerMove={onMove} onPointerUp={onUp} onClick={() => onSelect(null)}>
      <span className="rlane-label">{laneLabel}</span>
      {regions.map((r) => (
        <div
          key={r.id}
          className={"region " + (regionClass?.(r) ?? "") + (r.id === selected ? " sel" : "")}
          style={{ left: pct(r.start), width: pct(r.end - r.start) }}
          onPointerDown={(e) => onDown(e, r, "move")}
        >
          <span className="rlabel">{renderLabel(r)}</span>
          <span className="rhandle" onPointerDown={(e) => onDown(e, r, "resize")} />
        </div>
      ))}
      {!regions.length && hint && <span className="rhint">{hint}</span>}
    </div>
  );
}
