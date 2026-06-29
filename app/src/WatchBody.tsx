import { videoUrl } from "./api";
import { fmt, type Index, type ZoomKeyframe } from "./types";
import { RegionTrack } from "./RegionTrack";
import type { EditKind, EditRegion, ZoomRegion } from "./regions";

interface WatchBodyProps {
  id: string;
  index: Index;
  zoom: ZoomKeyframe[];
  t: number;
  dur: number;
  hasVideo: boolean;
  videoRef: React.RefObject<HTMLVideoElement>;
  developing: boolean;
  manualScale?: number;
  speedRate?: number;
  seek: (t: number) => void;
  regions: ZoomRegion[];
  edits: EditRegion[];
  selected: string | null;
  setSelected: (id: string | null) => void;
  setRegions: (rs: ZoomRegion[]) => void;
  setEdits: (es: EditRegion[]) => void;
  snapshot: () => void;
  addRegion: () => void;
  addEdit: (k: EditKind) => void;
  del: () => void;
  undo: () => void;
  canUndo: boolean;
  bg: string;
  setBg: (b: string) => void;
  rendering: boolean;
  doRender: () => void;
  exportProject: () => void;
}

const WALLPAPERS = ["aurora", "dusk", "ocean", "violet", "noir", "gradient"];

function kfAt(zoom: ZoomKeyframe[], t: number): ZoomKeyframe | null {
  if (!zoom.length) return null;
  let best = zoom[0];
  for (const k of zoom) if (Math.abs(k.t - t) < Math.abs(best.t - t)) best = k;
  return best;
}

export function WatchBody(p: WatchBodyProps) {
  const { index, zoom, t, dur, hasVideo, videoRef, developing, manualScale, speedRate, seek } = p;
  const kf = kfAt(zoom, t);
  const vstyle: React.CSSProperties | undefined = manualScale
    ? { transformOrigin: "50% 50%", transform: `scale(${manualScale})` }
    : kf && kf.scale > 1.02
    ? { transformOrigin: `${kf.cx * 100}% ${kf.cy * 100}%`, transform: `scale(${kf.scale})` }
    : undefined;

  // nearest captioned moment to the playhead → live caption
  const moment = index.visual_timeline.reduce<{ m: (typeof index.visual_timeline)[number]; d: number } | null>(
    (acc, m) => {
      const d = Math.abs(m.t - t);
      return !acc || d < acc.d ? { m, d } : acc;
    },
    null,
  );
  const caption = moment && moment.d < 1.2 ? moment.m.caption : null;
  const zoomLabel = manualScale ? `✎ manual ${manualScale.toFixed(1)}×` : kf && kf.scale > 1.05 ? `◎ ${kf.scale.toFixed(1)}× auto-zoom` : null;

  const togglePlay = () => {
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) void v.play();
    else v.pause();
  };

  const onScrub = (e: React.MouseEvent) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    seek(((e.clientX - r.left) / r.width) * dur);
  };

  return (
    <div className="watch-body">
      <div className="stage-shell">
        {developing && (
          <div className="scan-sweep">
            <span />
          </div>
        )}
        <div className={"vframe" + (hasVideo ? "" : " mock")}>
          {zoomLabel && <div className="zoom-badge">{zoomLabel}</div>}
          {speedRate ? <div className="zoom-badge" style={{ left: "auto", right: 12 }}>⏩ {speedRate}× speed</div> : null}
          {hasVideo ? (
            <video ref={videoRef} src={videoUrl(p.id)} controls playsInline style={vstyle} />
          ) : (
            <div style={{ padding: 40, color: "#888", fontFamily: "var(--font-mono)", textAlign: "center" }}>no video stream — index only</div>
          )}
          {caption && <div className="cap-badge">{caption}</div>}
        </div>
      </div>

      {/* scrubber with salient markers */}
      <div className="scrubber">
        <button className="pp" onClick={togglePlay} title="Play / pause">
          ▶
        </button>
        <div className="scrub-track" onClick={onScrub}>
          <div className="scrub-fill" style={{ width: `${dur ? (t / dur) * 100 : 0}%` }} />
          {index.visual_timeline.map((m, i) => (
            <span key={i} className="scrub-mark" title={m.caption} style={{ left: `${dur ? (m.t / dur) * 100 : 0}%` }} />
          ))}
          <div className="scrub-play" style={{ left: `${dur ? (t / dur) * 100 : 0}%` }} />
        </div>
        <span className="scrub-time">
          {fmt(t)} / {fmt(dur)}
        </span>
      </div>
      <div className="salient-note">
        <span className="ok">●</span> salient markers — veyo emitted {index.visual_timeline.length} captioned moments
      </div>

      {/* chapters */}
      {(index.summary.chapters?.length ?? 0) > 0 && (
        <div className="chapters">
          <div className="lbl">CHAPTERS</div>
          {index.summary.chapters!.map((ch, i) => (
            <button key={i} className="chapter" onClick={() => seek(ch.start)}>
              <span className="t">{fmt(ch.start)}</span>
              <span className="x">{ch.title}</span>
            </button>
          ))}
        </div>
      )}

      {/* editor power-features: manual zoom, cut/speed, render, export */}
      <div className="editor-controls">
        <div className="ec-head">CINEMATIC EDITOR — manual zoom overrides auto · cut/speed ramp · render to MP4</div>
        <div className="toolbar">
          <button className="btn" onClick={p.addRegion} style={{ borderRadius: 0 }}>
            + Zoom
          </button>
          <button className="btn" onClick={() => p.addEdit("trim")} style={{ borderRadius: 0 }}>
            ✂ Cut
          </button>
          <button className="btn" onClick={() => p.addEdit("speed")} style={{ borderRadius: 0 }}>
            ⏩ Speed
          </button>
          <button className="btn" onClick={p.del} disabled={!p.selected} style={{ borderRadius: 0 }}>
            Delete
          </button>
          <button className="btn" onClick={p.undo} disabled={!p.canUndo} style={{ borderRadius: 0 }}>
            ↶ Undo
          </button>
          <span className="sp" />
          <select className="wp-select" value={bgValue(p.bg)} onChange={(e) => p.setBg(e.target.value)} title="Background wallpaper for the render">
            {WALLPAPERS.map((w) => (
              <option key={w} value={w}>
                {w[0].toUpperCase() + w.slice(1)}
              </option>
            ))}
          </select>
          <button className="btn-signal" onClick={p.doRender} disabled={p.rendering} style={{ borderRadius: 0 }}>
            {p.rendering ? <span className="spin" /> : "▶ Render MP4"}
          </button>
          <button className="btn-mono" onClick={p.exportProject}>
            ⤓ .clipxd
          </button>
        </div>
        <RegionTrack
          regions={p.regions}
          duration={dur}
          selected={p.selected}
          laneLabel="manual zoom"
          onSelect={p.setSelected}
          onDragStart={p.snapshot}
          onChange={p.setRegions}
          renderLabel={(r) => `⌕ ${r.scale.toFixed(1)}×`}
          hint="“+ Zoom” adds a region at the playhead; drag to move, drag the edge to resize"
        />
        <RegionTrack
          regions={p.edits}
          duration={dur}
          selected={p.selected}
          laneLabel="cut / speed"
          minLen={0.3}
          onSelect={p.setSelected}
          onDragStart={p.snapshot}
          onChange={p.setEdits}
          renderLabel={(r) => (r.kind === "trim" ? "✂ cut" : `⏩ ${r.rate}×`)}
          regionClass={(r) => r.kind}
          hint="“✂ Cut” skips a span; “⏩ Speed” ramps it 2×"
        />
      </div>
    </div>
  );
}

function bgValue(b: string): string {
  return WALLPAPERS.includes(b) ? b : "aurora";
}
