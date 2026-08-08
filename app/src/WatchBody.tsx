import { useRef, useState } from "react";
import { videoUrl } from "./api";
import { fmt, type Index, type ZoomKeyframe } from "./types";
import { RegionTrack } from "./RegionTrack";
import { SubtitleLayer } from "./SubtitleStyle";
import { cropRect, type EditKind, type EditRegion, type ZoomRegion } from "./regions";
import { CAMERA_BG_PRESETS } from "./CameraConfig";

interface WatchBodyProps {
  id: string;
  index: Index;
  zoom: ZoomKeyframe[];
  t: number;
  dur: number;
  hasVideo: boolean;
  videoRef: React.RefObject<HTMLVideoElement>;
  developing: boolean;
  /** The manual zoom region covering the playhead, if any — the whole region, not just its
      scale, because the preview transform needs its focus point too. */
  manualZoom?: ZoomRegion;
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

// Backgrounds the render step understands. Each one is mapped through `bgPresetById` to
// the same canvas draw code the camera bubble uses (see `CameraConfig.ts`) so what you
// pick here is exactly what gets baked into the rendered MP4 — no previews that disagree
// with the output, no "select aurora, get ocean".
const WALLPAPERS = CAMERA_BG_PRESETS.filter((p) =>
  ["aurora", "dusk", "ocean", "violet", "noir", "mint"].includes(p.id),
);

/** The crop rectangle + focus point drawn over the (unzoomed) frame while a zoom region is
 *  selected. It is a GEOMETRY preview and its wording stays inside that promise: "the crop at
 *  full zoom", nothing about the wallpaper, the browser mockup, the shadow or the burned captions
 *  the server adds. It used to read "what fills the render", which overpromised: the render
 *  composites this rectangle *inside* the mockup rather than letting it fill the output.
 *
 *  Inside the region the rectangle is what EVERY frame crops to — there is no ramp at the edges.
 *  beautify.rs overwrites scale/cx/cy on every keyframe whose `t` lands in [start,end] and takes
 *  one keyframe per source frame, so the crop STEPS at the boundary. Measured on a 2 s static
 *  source with region [1.0,2.0] at 2× (static, so any picture change is the zoom): frames at
 *  t=0.80 and t=0.95 match at 75 dB PSNR, t=0.95 vs t=1.05 is 5 dB — a step — and the first frame
 *  inside (t=1.05) matches t=1.50 and t=1.90 at 81 dB, i.e. full crop from the first frame in.
 *  (An earlier version of this comment claimed the edges ramp the zoom in. They do not.) */
function FocusOverlay({ r }: { r: ZoomRegion }) {
  const c = cropRect(r);
  const pc = (v: number) => `${v * 100}%`;
  return (
    <div className="focus-overlay" aria-hidden>
      <div className="focus-rect" style={{ left: pc(c.x), top: pc(c.y), width: pc(c.w), height: pc(c.h) }} />
      <div className="focus-dot" style={{ left: pc(r.cx), top: pc(r.cy) }} />
      <div className="focus-hint">click the frame to aim the {r.scale.toFixed(1)}× zoom — dashed box = the crop at full zoom</div>
    </div>
  );
}

function kfAt(zoom: ZoomKeyframe[], t: number): ZoomKeyframe | null {
  if (!zoom.length) return null;
  let best = zoom[0];
  for (const k of zoom) if (Math.abs(k.t - t) < Math.abs(best.t - t)) best = k;
  return best;
}

/** Show exactly the source rectangle the renderer will crop (see `cropRect`). With a 0 0 origin,
 *  `scale(1/w) translate(-x,-y)` maps the normalised rect [x,x+w] onto the whole element — unlike
 *  the `transform-origin: cx cy` form this replaces, which drifted from the render as soon as the
 *  focus left the centre. Geometry only: no wallpaper, no mockup chrome, no burned captions. */
function cropStyle(z: { scale: number; cx: number; cy: number }): React.CSSProperties {
  const r = cropRect(z);
  return { transformOrigin: "0 0", transform: `scale(${1 / r.w}) translate(${-r.x * 100}%, ${-r.y * 100}%)` };
}

const MIN_LEN = 0.2; // matches RegionTrack's minLen — a region can't be shortened past this
const clamp01 = (v: number) => Math.min(1, Math.max(0, v));

export function WatchBody(p: WatchBodyProps) {
  const { index, zoom, t, dur, hasVideo, videoRef, developing, manualZoom, speedRate, seek } = p;
  const kf = kfAt(zoom, t);

  // The selected manual-zoom region, if the selection is one (the same `selected` id is shared
  // with the cut/speed lane). Selecting it puts the frame in focus-picking mode.
  const sel = p.regions.find((r) => r.id === p.selected) ?? null;

  // While a zoom region is selected the frame is deliberately shown UNZOOMED: you have to see
  // the whole frame to pick a point in it, and the click→normalised-coordinate mapping is only
  // 1:1 when nothing is scaled. The dashed rectangle is the preview of what will be cropped.
  const live = sel ? null : manualZoom ?? (kf && kf.scale > 1.02 ? kf : null);
  const vstyle: React.CSSProperties | undefined = live ? cropStyle(live) : undefined;

  // The live scene-caption badge is gone: it was machine narration of a video the viewer is
  // already watching, and it sat squarely on top of the player controls (its 42px started at
  // bottom:12px, inside the 53px control bar, wiping out the seek track). The same caption is
  // still reachable three better ways — the seek-bar marker tooltips, the Moments tab, and the
  // outline — none of which fight the controls. Deleting it also retires the 140-char clamp and
  // the line-clamp that existed only to contain it.
  // Suppressed in focus mode: the frame is unzoomed there, so a "✎ manual 2.4×" badge over it
  // would be claiming a zoom the viewer can plainly see isn't applied. The overlay states the
  // scale instead, next to the rectangle it belongs to.
  const zoomLabel = sel ? null : manualZoom ? `✎ manual ${manualZoom.scale.toFixed(1)}×` : kf && kf.scale > 1.05 ? `◎ ${kf.scale.toFixed(1)}× auto-zoom` : null;

  const frameRef = useRef<HTMLDivElement>(null);
  const [playing, setPlaying] = useState(false);
  const [muted, setMuted] = useState(false);
  const [vdur, setVdur] = useState(0);

  // Effective duration for the seek bar. A recording assembled from streamed MediaRecorder
  // chunks has no duration header, so the <video> element reports `Infinity` and the index's
  // own metadata.duration may be 0 on clips indexed before that was probed. Prefer the index
  // value; fall back to whatever the browser resolves (see onLoadedMetadata).
  const effDur = dur > 0 ? dur : vdur;
  const shownT = effDur ? Math.min(t, effDur) : t; // clamp the brief spike during the resolve trick
  const pct = effDur ? (shownT / effDur) * 100 : 0;

  // Nudge the browser to compute a real duration for an Infinity-duration WebM, which also
  // makes it seekable. Seek to a huge time; once the real duration is known, snap back to 0.
  const onLoadedMetadata = (e: React.SyntheticEvent<HTMLVideoElement>) => {
    const v = e.currentTarget;
    if (v.duration === Infinity || Number.isNaN(v.duration)) {
      const onUpdate = () => {
        if (Number.isFinite(v.duration)) {
          v.removeEventListener("timeupdate", onUpdate);
          setVdur(v.duration);
          v.currentTime = 0;
        }
      };
      v.addEventListener("timeupdate", onUpdate);
      v.currentTime = 1e101;
    } else {
      setVdur(v.duration);
    }
  };

  const togglePlay = () => {
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) void v.play();
    else v.pause();
  };

  const toggleMute = () => {
    const v = videoRef.current;
    if (!v) return;
    v.muted = !v.muted;
    setMuted(v.muted);
  };

  const toggleFullscreen = () => {
    const el = frameRef.current;
    if (!el) return;
    if (document.fullscreenElement) void document.exitFullscreen();
    else void el.requestFullscreen?.();
  };

  // `snap: false` for the scale slider only: its onChange fires on every pixel of the drag, and
  // snapshotting each one would bury the undo stack (~40 no-op steps per drag). The slider takes
  // its single snapshot on pointer/key down instead — the same one-per-gesture rule RegionTrack
  // follows with onDragStart.
  const patchSel = (patch: Partial<ZoomRegion>, snap = true) => {
    if (!sel) return;
    if (snap) p.snapshot();
    p.setRegions(p.regions.map((r) => (r.id === sel.id ? { ...r, ...patch } : r)));
  };

  // With a zoom region selected the frame is a focus picker, not a play/pause target: click
  // where the interesting thing is and the render zooms there instead of at the frame centre.
  // Deselect (click the lane background) and the click goes back to play/pause.
  const onFrameClick = (e: React.MouseEvent<HTMLVideoElement>) => {
    if (!sel) return togglePlay();
    const r = e.currentTarget.getBoundingClientRect();
    patchSel({ cx: clamp01((e.clientX - r.left) / r.width), cy: clamp01((e.clientY - r.top) / r.height) });
  };

  const onScrub = (e: React.MouseEvent) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    seek(Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)) * effDur);
  };

  return (
    <div className="watch-body">
      <div className="stage-shell">
        {developing && (
          <div className="scan-sweep">
            <span />
          </div>
        )}
        <div ref={frameRef} className={"vframe" + (hasVideo ? " has-player" : " mock") + (sel ? " focusing" : "")}>
          {zoomLabel && <div className="zoom-badge">{zoomLabel}</div>}
          {speedRate ? <div className="zoom-badge" style={{ left: "auto", right: 12 }}>⏩ {speedRate}× speed</div> : null}
          {hasVideo ? (
            <>
              <video
                ref={videoRef}
                src={videoUrl(p.id)}
                playsInline
                style={vstyle}
                onClick={onFrameClick}
                onLoadedMetadata={onLoadedMetadata}
                onPlay={() => setPlaying(true)}
                onPause={() => setPlaying(false)}
                onVolumeChange={(e) => setMuted((e.currentTarget as HTMLVideoElement).muted)}
              />
              {sel && <FocusOverlay r={sel} />}
              {/* single glass control bar — the whole player chrome, overlaid Loom-style */}
              <div className="player-bar">
                <button className="pbtn" onClick={togglePlay} title={playing ? "Pause" : "Play"} aria-label={playing ? "Pause" : "Play"}>
                  {playing ? "❚❚" : "▶"}
                </button>
                <span className="ptime mono">{fmt(shownT)}</span>
                <div className="pseek" onClick={onScrub} role="slider" aria-label="Seek" aria-valuenow={Math.round(shownT)} aria-valuemax={Math.round(effDur)}>
                  <div className="pseek-fill" style={{ width: `${pct}%` }} />
                  {index.visual_timeline.map((m, i) => (
                    <span key={i} className="pseek-mark" title={m.label || m.caption} style={{ left: `${effDur ? (m.t / effDur) * 100 : 0}%` }} />
                  ))}
                  <div className="pseek-head" style={{ left: `${pct}%` }} />
                </div>
                <span className="ptime mono">{fmt(effDur)}</span>
                <button className="pbtn" onClick={toggleMute} title={muted ? "Unmute" : "Mute"} aria-label={muted ? "Unmute" : "Mute"}>
                  {muted ? "🔇" : "🔊"}
                </button>
                <button className="pbtn" onClick={toggleFullscreen} title="Fullscreen" aria-label="Fullscreen">
                  ⛶
                </button>
              </div>
            </>
          ) : (
            <div style={{ padding: 40, color: "#888", fontFamily: "var(--font-mono)", textAlign: "center" }}>no video stream — index only</div>
          )}
          {/* Styled subtitles render for both video and voice-only clips — a voice-only clip's
              value IS its transcript + styled captions. */}
          <SubtitleLayer index={index} t={t} />
        </div>
      </div>

      <div className="salient-note">
        <span className="ok">●</span> {index.visual_timeline.length} key moment{index.visual_timeline.length === 1 ? "" : "s"} marked on the timeline — click a marker to jump
      </div>

      {/* Chapters rail removed.  Two reasons: (1) the chapter list was the LLM's
          guess at section breaks from a few-second scan, which is rarely the right
          unit for an index the user is going to scrub.  The caption-timestamped
          moments already on the seek bar + the Moments tab in the right rail (when
          opened) cover the same ground at a finer grain.  (2) The rail ate ~80px
          of vertical real-estate under the player on every clip page; the editor
          toolbar below already lives in the same gap, and the toolbar is the one a
          user is much more likely to need than a flat chapter list. */}

      {/* Editor power-features: manual zoom, cut/speed, render, export. Collapsed by default —
          this is an authoring surface, and most visits to a clip page are someone watching, not
          editing. Native <details> so there's no state, no hook, and no JS; the wrapper (rather
          than putting <details> on .editor-controls itself) keeps that element's flex layout
          exactly as it was. */}
      <details className="editor-details">
        <summary className="editor-summary">Cinematic editor — zoom, cut, speed, render</summary>
      <div className="editor-controls">
        <div className="ec-head">manual zoom overrides auto · cut/speed ramp · render to MP4</div>
        <div className="toolbar">
          <button className="btn" onClick={p.addRegion}>
            + Zoom
          </button>
          <button className="btn" onClick={() => p.addEdit("trim")}>
            ✂ Cut
          </button>
          <button className="btn" onClick={() => p.addEdit("speed")}>
            ⏩ Speed
          </button>
          <button className="btn" onClick={p.del} disabled={!p.selected}>
            Delete
          </button>
          <button className="btn" onClick={p.undo} disabled={!p.canUndo}>
            ↶ Undo
          </button>
          <span className="sp" />
          <button className="btn-signal" onClick={p.doRender} disabled={p.rendering}>
            {p.rendering ? <span className="spin" /> : "▶ Render MP4"}
          </button>
          <button className="btn-mono" onClick={p.exportProject}>
            ⤓ .clipxd
          </button>
        </div>
        {/* Wallpaper picker — swatches instead of a `<select>`, so the user picks the
            scene by its actual look (matching what gets baked into the MP4) rather than
            by its name. Same `CAMERA_BG_PRESETS` the camera bubble uses, so the two
            surfaces never disagree. */}
        <div className="wp-picker" role="radiogroup" aria-label="Render background">
          <span className="wp-label">Background</span>
          {WALLPAPERS.map((w) => (
            <button
              key={w.id}
              role="radio"
              aria-checked={p.bg === w.id}
              className={"wp-swatch" + (p.bg === w.id ? " on" : "")}
              style={{ background: w.css }}
              onClick={() => p.setBg(w.id)}
              title={w.label}
            >
              <span className="wp-swatch-lbl">{w.label}</span>
            </button>
          ))}
        </div>
        {/* Zoom inspector — the two things a region carries that a timeline block can't show:
            how far it zooms, and WHERE. Only mounted for a selected zoom region, so the toolbar
            stays the same size for everyone else. */}
        {sel && (
          <div className="zoom-inspect">
            <span className="wp-label">zoom</span>
            <input
              className="zi-range"
              type="range"
              min={1.1}
              max={4}
              step={0.1}
              value={sel.scale}
              aria-label="Zoom scale"
              onPointerDown={p.snapshot}
              // Only the keys that actually move the slider — snapshotting on Tab would leave an
              // Undo press that visibly does nothing.
              onKeyDown={(e) => (e.key.startsWith("Arrow") || e.key === "Home" || e.key === "End") && p.snapshot()}
              onChange={(e) => patchSel({ scale: Number(e.currentTarget.value) }, false)}
            />
            <span className="mono zi-val">{sel.scale.toFixed(1)}×</span>
            {/* Terse on purpose: the "click the frame" instruction lives on the frame itself,
                where the click has to happen, and the row has to survive the narrow split view. */}
            <span className="mono zi-dim" title="Click the video to move the zoom's focus point">
              focus {Math.round(sel.cx * 100)}%,{Math.round(sel.cy * 100)}%
            </span>
            <button className="btn-mono" onClick={() => patchSel({ cx: 0.5, cy: 0.5 })} disabled={sel.cx === 0.5 && sel.cy === 0.5}>
              centre
            </button>
            <span className="sp" />
            <span className="mono zi-dim">
              {sel.start.toFixed(2)}s → {sel.end.toFixed(2)}s
            </span>
            {/* Frame-accurate extents: scrub to the moment, press the edge. Dragging a block by
                hand can't land on a specific frame at these lane widths. */}
            <button
              className="btn-mono"
              title="Move this region's start to the playhead"
              disabled={t >= sel.end - MIN_LEN}
              onClick={() => patchSel({ start: Math.max(0, Math.min(t, sel.end - MIN_LEN)) })}
            >
              ⟦ start
            </button>
            <button
              className="btn-mono"
              title="Move this region's end to the playhead"
              disabled={t <= sel.start + MIN_LEN}
              onClick={() => patchSel({ end: Math.min(effDur || t, Math.max(t, sel.start + MIN_LEN)) })}
            >
              end ⟧
            </button>
          </div>
        )}
        <RegionTrack
          regions={p.regions}
          duration={effDur}
          selected={p.selected}
          laneLabel="manual zoom"
          playhead={shownT}
          onSelect={p.setSelected}
          onDragStart={p.snapshot}
          onChange={p.setRegions}
          renderLabel={(r) => `⌕ ${r.scale.toFixed(1)}×`}
          hint="“+ Zoom” adds a region at the playhead; select it to aim and size the zoom"
        />
        <RegionTrack
          regions={p.edits}
          duration={effDur}
          selected={p.selected}
          laneLabel="cut / speed"
          minLen={0.3}
          playhead={shownT}
          onSelect={p.setSelected}
          onDragStart={p.snapshot}
          onChange={p.setEdits}
          renderLabel={(r) => (r.kind === "trim" ? "✂ cut" : `⏩ ${r.rate}×`)}
          regionClass={(r) => r.kind}
          hint="“✂ Cut” skips a span; “⏩ Speed” ramps it 2×"
        />
      </div>
      </details>
    </div>
  );
}
