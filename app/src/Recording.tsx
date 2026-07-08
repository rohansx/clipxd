import { AnimatePresence, motion } from "framer-motion";
import { useCallback, useEffect, useRef, useState } from "react";
import { useScreenRecorder, type RecordMode } from "./useScreenRecorder";
import { Prompter } from "./Prompter";
import {
  CAMERA_PRESETS,
  DEFAULT_CAMERA_CONFIG,
  filterCss,
  previewBackgroundCss,
  CAMERA_BG_PRESETS,
  loadCameraConfig,
  saveCameraConfig,
  type CameraConfig,
  type CameraBgKind,
} from "./CameraConfig";
import { apiBase } from "./api";
import { usePrefersReducedMotion } from "./motion";
import {
  recordLastClipPending,
  recordLastClipReady,
  recordLastClipRecording,
  touchLastClipRecording,
  recordLastClipDone,
  shareUrlFor,
  clearLastClip,
  getLastClip,
  onLastClipChange,
  type LastClip,
} from "./lastClip";

interface RecordingProps {
  onClipReady: (id: string) => void;
  showToast: (m: string) => void;
  /** SPA navigation when the user picks "Open clip →" from the link card. */
  onOpenClip: (id: string) => void;
  /** Re-fire "Ready" → onClipReady, e.g. when the user clicks Retry. */
  onRetry: (id: string) => void;
}

const MIC_BARS = Array.from({ length: 56 }, (_, i) => ({
  h: 14 + Math.round(Math.abs(Math.sin(i * 0.7) * Math.cos(i * 0.3)) * 40),
  delay: (i % 12) * 0.08,
}));

const HINTS = [
  { icon: "●", label: "veyo salience gate", detail: "emits a frame only when the scene changes" },
  { icon: "◎", label: "cursor-follow auto-zoom", detail: "zoom tracks your pointer + clicks" },
  { icon: "▦", label: "OCR + captions", detail: "on-screen text + scene captions, timestamped" },
  { icon: "◈", label: "agent-queryable", detail: "ask the clip the moment it finishes" },
];

export function Recording({ onClipReady, showToast, onOpenClip, onRetry }: RecordingProps) {
  const reduced = usePrefersReducedMotion();
  const base = apiBase();
  const [camera, setCamera] = useState(false);
  const [showPrompter, setShowPrompter] = useState(false);
  const [mode, setMode] = useState<RecordMode>("screen");
  const [cameraCfg, setCameraCfg] = useState<CameraConfig>(loadCameraConfig);
  const [showCamSettings, setShowCamSettings] = useState(false);
  const [camStream, setCamStream] = useState<MediaStream | null>(null);
  const [secs, setSecs] = useState(0);
  const [copied, setCopied] = useState<"idle" | "copied" | "failed">("idle");

  // The recorder hook fires three callbacks: onPending immediately on Stop
  // (so a refresh mid-upload still has a banner), onClipReady on success,
  // onError after both stage-commit + ingest-fallback timed out.
  const handleReady = useCallback(
    (id: string) => {
      const username = localStorage.getItem("clipxd:username");
      // Best-effort clipboard write; the link card below covers the
      // "user missed it" case via a visible URL + Copy button.
      try { navigator.clipboard.writeText(shareUrlFor(id, username)); } catch {}
      recordLastClipReady(id, username);
      setCopied("copied");
      onClipReady(id);
    },
    [onClipReady],
  );
  const handlePending = useCallback(
    (stopId: string) => {
      const username = localStorage.getItem("clipxd:username");
      recordLastClipPending(stopId, username);
    },
    [],
  );
  // Instant link: the server minted the real clip id at record start — the share URL is
  // already live (status: recording), so surface a copyable link card immediately.
  const handleRecordingLink = useCallback((id: string) => {
    const username = localStorage.getItem("clipxd:username");
    recordLastClipRecording(id, username);
  }, []);
  const handleError = useCallback((reason: string) => {
    const existing = getLastClip();
    if (existing) {
      try {
        const failed: LastClip = { ...existing, status: "failed", error: reason, updatedAt: Date.now() };
        localStorage.setItem("clipxd:lastClip", JSON.stringify(failed));
        window.dispatchEvent(new CustomEvent("clipxd:lastClip", { detail: failed }));
      } catch {}
    }
    setCopied("failed");
    showToast(reason);
  }, [showToast]);

  const { state, countdown, start, stop, skipCountdown, cancelCountdown } = useScreenRecorder(base, {
    onRecordingLink: handleRecordingLink,
    onPending: handlePending,
    onClipReady: handleReady,
    onError: handleError,
  });

  useEffect(() => saveCameraConfig(cameraCfg), [cameraCfg]);

  // camera preview stream (screen mode only — voice mode has no camera)
  useEffect(() => {
    if (!camera || mode === "voice") {
      setCamStream(null);
      return;
    }
    let stream: MediaStream | null = null;
    let cancelled = false;
    navigator.mediaDevices
      .getUserMedia({ video: { width: 640, height: 480 }, audio: false })
      .then((s) => {
        if (cancelled) {
          s.getTracks().forEach((t) => t.stop());
          return;
        }
        stream = s;
        setCamStream(s);
      })
      .catch(() => {
        setCamera(false);
        showToast("Camera unavailable — check permissions");
      });
    return () => {
      cancelled = true;
      stream?.getTracks().forEach((t) => t.stop());
    };
  }, [camera, showToast]);

  // rec clock — also keeps the live "recording" localStorage record fresh so its
  // staleness pruning never fires on a long recording that's genuinely still running.
  useEffect(() => {
    if (state !== "recording") {
      setSecs(0);
      return;
    }
    const h = window.setInterval(() => {
      setSecs((s) => s + 1);
      touchLastClipRecording();
    }, 1000);
    return () => window.clearInterval(h);
  }, [state]);

  const clock = `${String(Math.floor(secs / 60)).padStart(2, "0")}:${String(secs % 60).padStart(2, "0")}`;
  const counting = state === "counting";
  const recording = state === "recording";
  const processing = state === "processing";
  const failed = state === "failed";

  // The "just made a clip" card. Survives across view switches via localStorage.
  const [lastClip, setLastClipState] = useState(getLastClip);
  useEffect(() => onLastClipChange(setLastClipState), []);

  // When the user starts recording again, retire the link card so it doesn't
  // race a brand-new recording-id landing on top of it.
  const retire = () => {
    clearLastClip();
    setCopied("idle");
  };

  // Stop, then go straight to the clip. The instant link already resolved the moment
  // recording started (handleRecordingLink), so there's no reason to make the user sit on
  // this page watching an "uploading" pill when the destination already works — the video
  // finishes assembling and the index fills in live on the page they land on.
  const stopAndOpen = () => {
    stop();
    if (lastClip && !lastClip.id.startsWith("pending_")) onOpenClip(lastClip.id);
  };

  const copyAgain = async () => {
    if (!lastClip) return;
    try {
      await navigator.clipboard.writeText(lastClip.url);
      setCopied("copied");
      window.setTimeout(() => setCopied("idle"), 1400);
    } catch {
      setCopied("failed");
      window.setTimeout(() => setCopied("idle"), 1800);
    }
  };

  /**
   * Retry the failed upload using the standard /ingest endpoint. Useful when
   * the user came back after a server blip; we still have the MediaStream's
   * blob in memory if this MediaRecorder is alive, otherwise we instruct
   * the user to start a new recording.
   */
  const retryUpload = async () => {
    const last = getLastClip();
    if (!last || last.id.startsWith("pending_")) return;
    showToast("Retrying upload…");
    try {
      // We don't have access to the original chunks here (Recorder hook owns
      // them) — so retry just touches the server with the last-known id, which
      // works if the clip file is still on disk. Otherwise we tell the user
      // to record again.
      const id = last.id;
      const r = await fetch(`${apiBase()}/clips`, { credentials: "include" });
      if (r.ok) {
        const data = await r.json().catch(() => ({})) as { clips?: { id: string; status: string }[] };
        const found = data.clips?.find((c) => c.id === id);
        if (found) {
          if (found.status === "complete" || found.status === "partial") recordLastClipDone();
          onRetry(id);
          return;
        }
      }
      // Clip isn't on the server yet — give up; clear + tell user.
      clearLastClip();
      showToast("Couldn't reach the server. Try recording again.");
    } catch {
      showToast("Retry failed: server unreachable. Try recording again.");
    }
  };

  // Instant link: the server minted the clip id at record start and the share URL resolves
  // (status: recording on the stub) — show a copyable live-link card. Keyed on the stored
  // status, NOT on live recorder state: after a mid-recording refresh the MediaRecorder is
  // gone but the link still exists (the sweeper publishes whatever chunks landed), and the
  // card is the only surface telling the user that.
  const showLiveLink = lastClip?.status === "recording" && !processing && !failed;
  const liveLinkInterrupted = showLiveLink && !recording;

  // The post-commit link card. In "indexing" the server has committed the video and Phase 2
  // is filling the index in; in "ready" the local card is gone (banner cleared via
  // recordLastClipDone).
  const showLinkReady =
    lastClip &&
    !lastClip.id.startsWith("pending_") &&
    lastClip.status !== "recording" &&
    lastClip.status !== "saving" &&
    !recording &&
    !processing &&
    !failed;

  // The "still uploading" pill — fires AS SOON AS Stop is clicked, before the commit
  // returns. With an instant link the id is already real, so the URL stays visible (the
  // link even plays the staged video while the commit finishes); against an older server
  // it's a placeholder "pending_…" id and we only show the reassurance text.
  const showSavingPill = lastClip?.status === "saving";
  const savingHasRealId = showSavingPill && lastClip !== null && !lastClip.id.startsWith("pending_");

  return (
    <div className="recording">
      <div className="rec-left">
        <div className="rec-status">
          <span className="rec-badge">
            <span className="led" />
            {recording
              ? "REC"
              : counting
              ? `STARTING · ${countdown}`
              : processing
              ? "UPLOADING"
              : failed
              ? "FAILED"
              : "READY"}
          </span>
          <span className="rec-clock">{recording ? clock : "00:00"}</span>
          <span className="rec-hint">{mode === "voice" ? "voice only · mic · captions on" : "screen · 1080p · auto-zoom on"}{camera ? " · camera" : ""}</span>
        </div>

        {/* Live-link card — the instant link. Mounts the moment recording starts: the
            server minted the real clip id at stage-open, so the share URL already
            resolves (it shows a "recording…" page and plays the staged video live). */}
        <AnimatePresence>
          {showLiveLink && lastClip && (
            <motion.div
              key="liveLink"
              className="link-ready"
              initial={reduced ? false : { opacity: 0, y: -12, scale: 0.98 }}
              animate={{ opacity: 1, y: 0, scale: 1, transition: { type: "spring", stiffness: 320, damping: 28 } }}
              exit={{ opacity: 0, y: -8, transition: { duration: 0.18 } }}
            >
              <div className="link-ready-head">
                <span className="led" style={{ width: 8, height: 8 }} />
                <b>{liveLinkInterrupted ? "Recording interrupted · link still live" : "Recording · link is live"}</b>
                {liveLinkInterrupted && (
                  <button
                    type="button"
                    className="link-ready-x"
                    onClick={() => retire()}
                    aria-label="Dismiss link card"
                    title="Dismiss"
                  >
                    ✕
                  </button>
                )}
              </div>
              <input
                className="input mono"
                readOnly
                value={lastClip.url}
                onFocus={(e) => e.currentTarget.select()}
                onClick={(e) => e.currentTarget.select()}
              />
              <div className="link-ready-row">
                <button
                  type="button"
                  className="btn-signal btn-pill"
                  onClick={copyAgain}
                  style={{ padding: "0 18px" }}
                >
                  {copied === "copied" ? "✓ Copied" : copied === "failed" ? "Press ⌘C" : "Copy link"}
                </button>
                <span className="link-ready-hint">
                  {liveLinkInterrupted
                    ? "This tab lost its recorder (refresh?). Everything captured so far publishes to this link automatically."
                    : "Share it now — the page fills in live and the full video lands when you stop."}
                </span>
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        {/* Saving pill — visible the moment Stop is clicked, before the
            commit completes.  Persists across refreshes so a user who
            navigates away mid-upload still sees what's happening. */}
        <AnimatePresence>
          {showSavingPill && (
            <motion.div
              key="savingPill"
              className="link-ready saving"
              initial={reduced ? false : { opacity: 0, y: -8 }}
              animate={{ opacity: 1, y: 0, transition: { type: "spring", stiffness: 280, damping: 22 } }}
              exit={{ opacity: 0, y: -6, transition: { duration: 0.18 } }}
            >
              <div className="link-ready-head">
                <span className="dot sodium" style={{ width: 8, height: 8, boxShadow: "0 0 8px var(--sodium)" }} />
                <b>Stopped · uploading…</b>
              </div>
              {savingHasRealId && lastClip && (
                <input
                  className="input mono"
                  readOnly
                  value={lastClip.url}
                  onFocus={(e) => e.currentTarget.select()}
                  onClick={(e) => e.currentTarget.select()}
                />
              )}
              <div className="link-ready-hint" style={{ marginTop: 6 }}>
                {savingHasRealId
                  ? "Your link already works — the final video is being assembled on the server."
                  : "Don't refresh — your recording is being assembled on the server. Refresh any time, and you'll land back here on this page."}
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        {/* Link-ready card — emitted only when the server has handed us a
            real clip id (post-commit).  URL + Copy + Open all surface here,
            and the card persists across reloads via localStorage. */}
        <AnimatePresence>
          {showLinkReady && (
            <motion.div
              key="linkReady"
              className="link-ready"
              initial={reduced ? false : { opacity: 0, y: -12, scale: 0.98 }}
              animate={{ opacity: 1, y: 0, scale: 1, transition: { type: "spring", stiffness: 320, damping: 28 } }}
              exit={{ opacity: 0, y: -8, transition: { duration: 0.18 } }}
            >
              <div className="link-ready-head">
                <span className="dot signal" style={{ width: 8, height: 8, boxShadow: "0 0 8px var(--signal)" }} />
                <b>Recording ready · link copied</b>
                <button
                  type="button"
                  className="link-ready-x"
                  onClick={() => retire()}
                  aria-label="Dismiss link card"
                  title="Dismiss"
                >
                  ✕
                </button>
              </div>
              <input
                className="input mono"
                readOnly
                value={lastClip.url}
                onFocus={(e) => e.currentTarget.select()}
                onClick={(e) => e.currentTarget.select()}
              />
              <div className="link-ready-row">
                <button
                  type="button"
                  className="btn-signal btn-pill"
                  onClick={copyAgain}
                  style={{ padding: "0 18px" }}
                >
                  {copied === "copied" ? "✓ Copied" : copied === "failed" ? "Press ⌘C" : "Copy link"}
                </button>
                <button
                  type="button"
                  className="btn btn-pill"
                  onClick={() => lastClip && onOpenClip(lastClip.id)}
                  style={{ padding: "0 18px" }}
                >
                  Open clip →
                </button>
                <span className="link-ready-hint">
                  Indexing transcript / OCR / captions in the background — refresh any time.
                </span>
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        {/* Failed-state card. The user gets both a clear reason and a Retry
            button. The Retry tries to recover (the server may have actually
            finished while the page was reloading); if that fails, the
            message says to just record again. */}
        <AnimatePresence>
          {failed && lastClip && lastClip.status === "failed" && (
            <motion.div
              key="failedCard"
              className="link-ready failed"
              initial={reduced ? false : { opacity: 0, y: -12, scale: 0.98 }}
              animate={{ opacity: 1, y: 0, scale: 1, transition: { type: "spring", stiffness: 320, damping: 28 } }}
              exit={{ opacity: 0, y: -8, transition: { duration: 0.18 } }}
            >
              <div className="link-ready-head">
                <span className="dot" style={{ width: 8, height: 8, background: "var(--danger)" }} />
                <b>Upload didn't land</b>
                <button
                  type="button"
                  className="link-ready-x"
                  onClick={() => retire()}
                  aria-label="Dismiss"
                  title="Dismiss"
                >
                  ✕
                </button>
              </div>
              <div className="link-ready-hint" style={{ marginTop: 4 }}>
                {lastClip.error ?? "Upload timed out before the server responded."}
              </div>
              <div className="link-ready-row">
                <button
                  type="button"
                  className="btn-signal btn-pill"
                  onClick={retryUpload}
                  style={{ padding: "0 18px" }}
                >
                  Retry
                </button>
                <button
                  type="button"
                  className="btn btn-pill"
                  onClick={() => { retire(); start({ camera: camStream, cameraConfig: cameraCfg, mode }); }}
                  style={{ padding: "0 18px" }}
                >
                  Record again
                </button>
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        <div className="stage-shell">
          <div className="vframe mock">
            <div className="wipe-card-bar" style={{ background: "#f1f1f3", borderRadius: 0 }}>
              <i style={{ width: 9, height: 9, borderRadius: "50%", background: "#ec6a5e", display: "inline-block" }} />
              <i style={{ width: 9, height: 9, borderRadius: "50%", background: "#f4be4f", display: "inline-block" }} />
              <i style={{ width: 9, height: 9, borderRadius: "50%", background: "#61c454", display: "inline-block" }} />
              <span style={{ marginLeft: 8, fontFamily: "var(--font-mono)", fontSize: 10, color: "#8a8a90", background: "#fff", border: "1px solid #e3e3e6", borderRadius: 7, padding: "2px 9px" }}>
                your screen → clipxd
              </span>
            </div>
            {counting ? (
              <div className="countdown-panel">
                <div className="countdown-num" key={countdown}>{countdown}</div>
                <div style={{ fontWeight: 700, fontSize: 15 }}>Get your screen ready…</div>
                <div style={{ marginTop: 6, fontFamily: "var(--font-mono)", fontSize: 12, color: "#777" }}>
                  Switch to the window you want to show — recording starts automatically.
                </div>
                <div style={{ marginTop: 16, display: "flex", gap: 10 }}>
                  <button className="btn-sodium btn-pill" onClick={skipCountdown} style={{ fontSize: 13, padding: "9px 18px" }}>
                    Start now
                  </button>
                  <button className="btn btn-pill" onClick={cancelCountdown} style={{ fontSize: 13, padding: "9px 18px" }}>
                    Cancel
                  </button>
                </div>
              </div>
            ) : (
              <div style={{ padding: 28, color: "#222", background: "#fff", minHeight: 160 }}>
                <div style={{ fontWeight: 700, fontSize: 15 }}>
                  {recording
                    ? "Recording your screen…"
                    : processing
                    ? "Taking you to your clip…"
                    : failed
                    ? "Upload didn't complete."
                    : "Press record — pick a screen or window."}
                </div>
                <div style={{ marginTop: 10, fontFamily: "var(--font-mono)", fontSize: 12, color: "#777" }}>
                  {recording
                    ? "System audio + your cursor are being captured. Hit Stop when you're done."
                    : processing
                    ? "The link already works — the full video finishes assembling in the background."
                    : failed
                    ? "See the card above for what to do next."
                    : "The browser will ask which screen/window/tab to capture. System audio + your cursor are recorded too."}
                </div>
              </div>
            )}
          </div>
        </div>

        <div>
          <div className="rec-hint" style={{ marginBottom: 6 }}>mic</div>
          <div className="mic-bars">
            {MIC_BARS.map((b, i) => (
              <span
                key={i}
                style={{
                  height: recording ? `${b.h}%` : "10%",
                  animationDelay: `${b.delay}s`,
                  animationPlayState: recording ? "running" : "paused",
                }}
              />
            ))}
          </div>
        </div>

        <div className="toolbar">
          {!counting && !recording && !processing && !failed && (
            <button className="btn-sodium btn-pill" onClick={() => { retire(); start({ camera: camStream, cameraConfig: cameraCfg, mode }); }} style={{ fontSize: 14, padding: "12px 22px" }}>
              ● {mode === "voice" ? "Record voice" : lastClip?.status === "failed" ? "Try again" : lastClip ? "Record another" : "Start recording"}
            </button>
          )}
          {counting && (
            <button className="btn btn-pill" disabled style={{ fontSize: 14, padding: "12px 22px" }}>
              Starting in {countdown}… (controls above)
            </button>
          )}
          {recording && (
            <button className="btn-sodium btn-pill" onClick={stopAndOpen} style={{ fontSize: 14, padding: "12px 22px" }}>
              ■ Stop &amp; open clip
            </button>
          )}
          {processing && (
            <button className="btn btn-pill" disabled style={{ fontSize: 14, padding: "12px 22px" }}>
              <span className="spin" /> Uploading…
            </button>
          )}
          {failed && (
            <button className="btn-sodium btn-pill" disabled style={{ fontSize: 14, padding: "12px 22px" }}>
              Upload failed — Retry above ↓
            </button>
          )}
          {/* Capture mode: screen (screen + system audio + optional camera) vs voice-only
              (microphone only → a transcript + styled-caption clip, no video). */}
          {!counting && !recording && !processing && !failed && (
            <div className="mode-toggle" role="group" aria-label="Capture mode">
              <button className={mode === "screen" ? "on" : ""} onClick={() => setMode("screen")}>🖥 Screen</button>
              <button className={mode === "voice" ? "on" : ""} onClick={() => { setMode("voice"); setCamera(false); }}>🎙 Voice only</button>
            </div>
          )}
          {mode === "screen" && (
            <button
              className={"btn btn-pill" + (camera ? " on" : "")}
              onClick={() => setCamera((c) => !c)}
              style={camera ? { borderColor: "var(--signal)" } : undefined}
            >
              📷 Camera {camera ? "on" : "off"}
            </button>
          )}
          {mode === "screen" && camera && (
            <button
              className={"btn btn-pill" + (showCamSettings ? " on" : "")}
              onClick={() => setShowCamSettings((s) => !s)}
              style={showCamSettings ? { borderColor: "var(--signal)" } : undefined}
            >
              ✨ Filters & bg
            </button>
          )}
          <button
            className="btn btn-pill"
            onClick={() => setShowPrompter((s) => !s)}
            style={showPrompter ? { borderColor: "var(--signal)" } : undefined}
          >
            📜 Prompter
          </button>
        </div>
      </div>

      <div className="rec-right">
        <div className="head">
          <span className="led-on" />
          <span className="lbl">
            <b>read</b> · index forms on stop
          </span>
          <span className="egress">on device · 0 px egress</span>
        </div>
        <div className="rec-events">
          <AnimatePresence initial={false}>
            {HINTS.map((h, i) => (
              <motion.div
                key={h.label}
                className="read-row"
                style={{ cursor: "default" }}
                initial={reduced ? false : { opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0, transition: { delay: 0.04 * i, duration: 0.32, ease: [0.22, 1, 0.36, 1] } }}
              >
                <span className="t" style={{ width: 18 }}>{h.icon}</span>
                <div>
                  <div style={{ fontSize: 13.5, fontWeight: 600 }}>{h.label}</div>
                  <div className="mono" style={{ fontSize: 11, color: "var(--text-3)" }}>{h.detail}</div>
                </div>
              </motion.div>
            ))}
          </AnimatePresence>
        </div>
      </div>

      <AnimatePresence>
        {camStream && <CameraBubble key="cam" stream={camStream} cfg={cameraCfg} />}
      </AnimatePresence>
      <AnimatePresence>
        {showPrompter && <Prompter key="prompter" onClose={() => setShowPrompter(false)} />}
      </AnimatePresence>
      <AnimatePresence>
        {showCamSettings && mode === "screen" && camera && (
          <CameraSettings key="camset" cfg={cameraCfg} setCfg={setCameraCfg} onClose={() => setShowCamSettings(false)} />
        )}
      </AnimatePresence>
    </div>
  );
}

function CameraBubble({ stream, cfg }: { stream: MediaStream; cfg: CameraConfig }) {
  const reduced = usePrefersReducedMotion();
  const ref = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    const v = ref.current;
    if (v) {
      v.srcObject = stream;
      v.play().catch(() => {});
    }
  }, [stream]);
  // Preview the chosen background as a ring around the camera inset, matching what gets baked
  // into the recorded canvas (the recorder draws the bg behind a sharp inset of the same size).
  const bg = cfg.background;
  const insetPct = Math.round((bg.inset ?? 0.82) * 100);
  const showRing = bg.kind !== "none" && bg.kind !== "blur";
  return (
    <motion.div
      className="cam-bubble"
      title="This camera bubble is baked into your recording (bottom-right)"
      initial={reduced ? false : { opacity: 0, y: 20, scale: 0.9 }}
      animate={{ opacity: 1, y: 0, scale: 1, transition: { type: "spring", stiffness: 280, damping: 24 } }}
      exit={{ opacity: 0, y: 12, transition: { duration: 0.16 } }}
      style={{ background: showRing ? previewBackgroundCss(bg) : undefined, display: "grid", placeItems: "center" }}
    >
      <video
        ref={ref}
        muted
        playsInline
        style={{ filter: filterCss(cfg.filter), width: showRing ? `${insetPct}%` : "100%", height: showRing ? `${insetPct}%` : "100%", borderRadius: "50%", objectFit: "cover" }}
      />
    </motion.div>
  );
}

// Camera filters + background popover. The filter is applied live to the preview <video> (via
// CSS `filter`) AND baked into the recorded canvas (see useScreenRecorder), so the recorded
// bubble matches what the presenter sees. Background here is the area behind the camera
// inset inside the bubble — a soft blur, a solid, or a gradient for a clean produced look.
function CameraSettings({ cfg, setCfg, onClose }: { cfg: CameraConfig; setCfg: (c: CameraConfig) => void; onClose: () => void }) {
  const reduced = usePrefersReducedMotion();
  const set = (patch: Partial<CameraConfig>) => setCfg({ ...cfg, ...patch });
  const setFilter = (patch: Partial<CameraConfig["filter"]>) => set({ filter: { ...cfg.filter, ...patch } });
  const setBg = (patch: Partial<CameraConfig["background"]>) => set({ background: { ...cfg.background, ...patch } });
  return (
    <motion.div
      className="cam-settings"
      initial={reduced ? false : { opacity: 0, y: 12, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1, transition: { type: "spring", stiffness: 300, damping: 26 } }}
      exit={{ opacity: 0, y: 8, transition: { duration: 0.16 } }}
    >
      <div className="prompter-bar">
        <b>Camera look</b>
        <span className="tb-spacer" />
        <button onClick={onClose} title="Close">✕</button>
      </div>
      <div className="cam-settings-body">
        <div className="lbl">Presets</div>
        <div className="cam-presets">
          {CAMERA_PRESETS.map((p) => (
            <button
              key={p.name}
              className="cam-preset"
              onClick={() => set({ filter: { ...p.filter } })}
              style={{ filter: filterCss(p.filter) }}
            >
              {p.name}
            </button>
          ))}
        </div>

        <div className="lbl">Live filter</div>
        <label className="cam-slider"> brightness
          <input type="range" min={50} max={150} value={Math.round(cfg.filter.brightness * 100)} onChange={(e) => setFilter({ brightness: +e.target.value / 100 })} />
        </label>
        <label className="cam-slider"> contrast
          <input type="range" min={50} max={150} value={Math.round(cfg.filter.contrast * 100)} onChange={(e) => setFilter({ contrast: +e.target.value / 100 })} />
        </label>
        <label className="cam-slider"> saturate
          <input type="range" min={0} max={200} value={Math.round(cfg.filter.saturate * 100)} onChange={(e) => setFilter({ saturate: +e.target.value / 100 })} />
        </label>
        <label className="cam-slider"> grayscale
          <input type="range" min={0} max={100} value={Math.round(cfg.filter.grayscale * 100)} onChange={(e) => setFilter({ grayscale: +e.target.value / 100 })} />
        </label>
        <label className="cam-slider"> sepia
          <input type="range" min={0} max={100} value={Math.round(cfg.filter.sepia * 100)} onChange={(e) => setFilter({ sepia: +e.target.value / 100 })} />
        </label>
        <label className="cam-slider"> hue
          <input type="range" min={-180} max={180} value={cfg.filter.hue} onChange={(e) => setFilter({ hue: +e.target.value })} />
        </label>

        <div className="lbl">Background (clean look)</div>
        <div className="cam-bg-kinds">
          {(["none", "blur", "solid", "gradient", "preset", "image"] as CameraBgKind[]).map((k) => (
            <button key={k} className={cfg.background.kind === k ? "on" : ""} onClick={() => setBg({ kind: k })}>{k}</button>
          ))}
        </div>
        {cfg.background.kind === "preset" && (
          <>
            <div className="cam-bg-presets">
              {CAMERA_BG_PRESETS.map((p) => (
                <button
                  key={p.id}
                  className={"cam-bg-swatch" + (cfg.background.presetId === p.id ? " on" : "")}
                  style={{ background: p.css }}
                  title={p.label}
                  onClick={() => setBg({ presetId: p.id })}
                >
                  <span>{p.label}</span>
                </button>
              ))}
            </div>
            <label className="cam-up">
              upload your own image
              <input
                type="file"
                accept="image/*"
                onChange={(e) => {
                  const f = e.target.files?.[0];
                  if (!f) return;
                  const url = URL.createObjectURL(f);
                  setBg({ kind: "image", imageSrc: url });
                }}
              />
            </label>
          </>
        )}
        {cfg.background.kind === "image" && (
          <>
            <label className="cam-up">
              {cfg.background.imageSrc ? "replace image" : "choose an image"}
              <input
                type="file"
                accept="image/*"
                onChange={(e) => {
                  const f = e.target.files?.[0];
                  if (!f) return;
                  const url = URL.createObjectURL(f);
                  setBg({ imageSrc: url });
                }}
              />
            </label>
            {cfg.background.imageSrc && (
              <div className="cam-bg-swatch on" style={{ background: previewBackgroundCss(cfg.background), height: 48 }}>
                <span>your image</span>
              </div>
            )}
          </>
        )}
        {cfg.background.kind === "blur" && (
          <label className="cam-slider"> blur
            <input type="range" min={1} max={24} value={cfg.background.blur} onChange={(e) => setBg({ blur: +e.target.value })} />
          </label>
        )}
        {cfg.background.kind === "solid" && (
          <label className="cam-color"> color
            <input type="color" value={cfg.background.color} onChange={(e) => setBg({ color: e.target.value })} />
          </label>
        )}
        {cfg.background.kind === "gradient" && (
          <>
            <label className="cam-color"> from
              <input type="color" value={cfg.background.color} onChange={(e) => setBg({ color: e.target.value })} />
            </label>
            <label className="cam-color"> to
              <input type="color" value={cfg.background.color2} onChange={(e) => setBg({ color2: e.target.value })} />
            </label>
          </>
        )}
        {cfg.background.kind !== "none" && (
          <label className="cam-slider"> camera inset {(Math.round(cfg.background.inset * 100))}%
            <input type="range" min={50} max={95} value={Math.round(cfg.background.inset * 100)} onChange={(e) => setBg({ inset: +e.target.value / 100 })} />
          </label>
        )}
        <button className="btn btn-pill" style={{ marginTop: 8 }} onClick={() => setCfg({ ...DEFAULT_CAMERA_CONFIG })}>Reset</button>
      </div>
    </motion.div>
  );
}
