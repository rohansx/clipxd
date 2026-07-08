import { useRef, useState } from "react";
import { videoUrl } from "./api";
import { fmt, type Index, type ZoomKeyframe } from "./types";
import { RegionTrack } from "./RegionTrack";
import { SubtitleLayer } from "./SubtitleStyle";
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
  // Cap the live caption badge's length — an older clip's caption (predating the server-side
  // repetition-collapse fix) or just a genuinely long one otherwise overflows the whole video
  // (no natural max-height for text sitting over playback controls).
  const rawCaption = moment && moment.d < 1.2 ? moment.m.caption : null;
  const caption = rawCaption && rawCaption.length > 140 ? rawCaption.slice(0, 140).trimEnd() + "…" : rawCaption;
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
              {caption && <div className="cap-badge">{caption}</div>}
              {/* Styled subtitle layer: the user's chosen design + the indexing-time emphasis.
                  Only renders when a transcript exists AND the user has picked a design. */}
              {hasVideo && <SubtitleLayer index={index} t={t} />}
              {/* single glass control bar — the whole player chrome, overlaid Loom-style */}
              <div className="player-bar">
                <button className="pbtn" onClick={togglePlay} title={playing ? "Pause" : "Play"} aria-label={playing ? "Pause" : "Play"}>
                  {playing ? "❚❚" : "▶"}
                </button>
                <span className="ptime mono">{fmt(shownT)}</span>
                <div className="pseek" onClick={onScrub} role="slider" aria-label="Seek" aria-valuenow={Math.round(shownT)} aria-valuemax={Math.round(effDur)}>
                  <div className="pseek-fill" style={{ width: `${pct}%` }} />
                  {index.visual_timeline.map((m, i) => (
                    <span key={i} className="pseek-mark" title={m.caption} style={{ left: `${effDur ? (m.t / effDur) * 100 : 0}%` }} />
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
