import { useRef } from "react";
import type { Seg } from "./regions";

// A timeline lane of draggable segments: drag a block to move it, drag either edge to resize,
// click to select. Generic over any {id,start,end} so it serves both the manual-zoom lane and
// the trim/speed lane. Immutable — every change calls onChange with a new array (the parent
// snapshots for undo).
export function RegionTrack<T extends Seg>({
  regions, duration, selected, laneLabel, minLen = 0.2, playhead, onSelect, onDragStart, onChange, renderLabel, regionClass, hint,
}: {
  regions: T[];
  duration: number;
  selected: string | null;
  laneLabel: string;
  minLen?: number;
  /** Current playback time, drawn as a line across the lane. Without it you are editing
      regions blind — nothing in the lane says where "now" is. */
  playhead?: number;
  onSelect: (id: string | null) => void;
  onDragStart: () => void;
  onChange: (rs: T[]) => void;
  renderLabel: (r: T) => string;
  regionClass?: (r: T) => string;
  hint?: string;
}) {
  type Mode = "move" | "resize" | "resizeL";
  const drag = useRef<{ id: string; mode: Mode; x0: number; r0: T } | null>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const dur = duration || 1;
  const pct = (x: number) => `${(x / dur) * 100}%`;

  const onDown = (e: React.PointerEvent, r: T, mode: Mode) => {
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
      // Left edge: the only way to trim the *front* of a region without dragging the whole
      // block off its end point and putting it back.
      if (d.mode === "resizeL") {
        return { ...r, start: Math.max(0, Math.min(d.r0.end - minLen, d.r0.start + dt)) };
      }
      return { ...r, end: Math.max(d.r0.start + minLen, Math.min(dur, d.r0.end + dt)) };
    }));
  };

  const onUp = () => { drag.current = null; };

  return (
    // Deselect only when the click landed on the lane BACKGROUND. `onPointerDown` on a block
    // stops pointer-event propagation, but a click is a separate event that still bubbles here
    // — so every click that selected a region immediately cleared it again (selection never
    // stuck for longer than a frame, which is why Delete stayed disabled after clicking a
    // block). Verified in Chromium: pointerdown→select, then the bubbled click→deselect.
    <div className="regiontrack" ref={trackRef} onPointerMove={onMove} onPointerUp={onUp} onClick={(e) => e.target === e.currentTarget && onSelect(null)}>
      <span className="rlane-label">{laneLabel}</span>
      {regions.map((r) => (
        <div
          key={r.id}
          className={"region " + (regionClass?.(r) ?? "") + (r.id === selected ? " sel" : "")}
          style={{ left: pct(r.start), width: pct(r.end - r.start) }}
          onPointerDown={(e) => onDown(e, r, "move")}
        >
          <span className="rhandle rhandle-l" onPointerDown={(e) => onDown(e, r, "resizeL")} />
          <span className="rlabel">{renderLabel(r)}</span>
          <span className="rhandle" onPointerDown={(e) => onDown(e, r, "resize")} />
        </div>
      ))}
      {!regions.length && hint && <span className="rhint">{hint}</span>}
      {playhead !== undefined && <span className="rplayhead" style={{ left: pct(Math.min(playhead, dur)) }} />}
    </div>
  );
}
