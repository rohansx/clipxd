import { useRef, useState } from "react";
import { videoUrl } from "./api";
import { fmt, type Index, type ZoomKeyframe } from "./types";
import { RegionTrack } from "./RegionTrack";
import { SubtitleLayer } from "./SubtitleStyle";
import type { EditKind, EditRegion, ZoomRegion } from "./regions";
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

// Backgrounds the render step understands. Each one is mapped through `bgPresetById` to
// the same canvas draw code the camera bubble uses (see `CameraConfig.ts`) so what you
// pick here is exactly what gets baked into the rendered MP4 — no previews that disagree
// with the output, no "select aurora, get ocean".
const WALLPAPERS = CAMERA_BG_PRESETS.filter((p) =>
  ["aurora", "dusk", "ocean", "violet", "noir", "mint"].includes(p.id),
);

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

  // The live scene-caption badge is gone: it was machine narration of a video the viewer is
  // already watching, and it sat squarely on top of the player controls (its 42px started at
  // bottom:12px, inside the 53px control bar, wiping out the seek track). The same caption is
  // still reachable three better ways — the seek-bar marker tooltips, the Moments tab, and the
  // outline — none of which fight the controls. Deleting it also retires the 140-char clamp and
  // the line-clamp that existed only to contain it.
  const zoomLabel = manualScale ? `✎ manual ${manualScale.toFixed(1)}×` : kf && kf.scale > 1.05 ? `◎ ${kf.scale.toFixed(1)}× auto-zoom` : null;

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
        <div ref={frameRef} className={"vframe" + (hasVideo ? " has-player" : " mock")}>
          {zoomLabel && <div className="zoom-badge">{zoomLabel}</div>}
          {speedRate ? <div className="zoom-badge" style={{ left: "auto", right: 12 }}>⏩ {speedRate}× speed</div> : null}
          {hasVideo ? (
            <>
              <video
                ref={videoRef}
                src={videoUrl(p.id)}
                playsInline
                style={vstyle}
                onClick={togglePlay}
                onLoadedMetadata={onLoadedMetadata}
                onPlay={() => setPlaying(true)}
                onPause={() => setPlaying(false)}
                onVolumeChange={(e) => setMuted((e.currentTarget as HTMLVideoElement).muted)}
              />
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
      </details>
    </div>
  );
}
