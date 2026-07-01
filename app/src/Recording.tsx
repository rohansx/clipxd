import { AnimatePresence, motion } from "framer-motion";
import { useCallback, useEffect, useRef, useState } from "react";
import { useScreenRecorder } from "./useScreenRecorder";
import { Prompter } from "./Prompter";
import { apiBase } from "./api";
import { usePrefersReducedMotion } from "./motion";
import {
  recordLastClipPending,
  recordLastClipReady,
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

  const { state, start, stop } = useScreenRecorder(base, {
    onPending: handlePending,
    onClipReady: handleReady,
    onError: handleError,
  });

  // camera preview stream
  useEffect(() => {
    if (!camera) {
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

  // rec clock
  useEffect(() => {
    if (state !== "recording") {
      setSecs(0);
      return;
    }
    const h = window.setInterval(() => setSecs((s) => s + 1), 1000);
    return () => window.clearInterval(h);
  }, [state]);

  const clock = `${String(Math.floor(secs / 60)).padStart(2, "0")}:${String(secs % 60).padStart(2, "0")}`;
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
          if (found.status !== "enriching") recordLastClipDone();
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

  // The link card mounts when the user has a clip AND it's not actively being
  // recorded.  In "saving" the user just clicked Stop; in "indexing" the
  // server has issued a real id and we're polling; in "ready" the local
  // card is gone (banner cleared via recordLastClipDone).
  const showLinkReady =
    lastClip &&
    !lastClip.id.startsWith("pending_") &&
    !recording &&
    !processing &&
    !failed;

  // The "still uploading" pill — fires AS SOON AS Stop is clicked, before
  // the commit returns. This is the band-aid for "I refreshed mid-upload".
  const showSavingPill = lastClip && lastClip.id.startsWith("pending_");

  return (
    <div className="recording">
      <div className="rec-left">
        <div className="rec-status">
          <span className="rec-badge">
            <span className="led" />
            {recording
              ? "REC"
              : processing
              ? "UPLOADING"
              : failed
              ? "FAILED"
              : "READY"}
          </span>
          <span className="rec-clock">{recording ? clock : "00:00"}</span>
          <span className="rec-hint">screen · 1080p · auto-zoom on{camera ? " · camera" : ""}</span>
        </div>

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
              <div className="link-ready-hint" style={{ marginTop: 6 }}>
                Don't refresh — your recording is being assembled on the server. Refresh any time,
                and you'll land back here on this page.
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
                  onClick={() => { retire(); start(camStream); }}
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
            <div style={{ padding: 28, color: "#222", background: "#fff", minHeight: 160 }}>
              <div style={{ fontWeight: 700, fontSize: 15 }}>
                {recording
                  ? "Recording your screen…"
                  : processing
                  ? "Uploading — link ready in a moment…"
                  : failed
                  ? "Upload didn't complete."
                  : "Press record — pick a screen or window."}
              </div>
              <div style={{ marginTop: 10, fontFamily: "var(--font-mono)", fontSize: 12, color: "#777" }}>
                The browser will ask which screen/window/tab to capture. System audio + your cursor are recorded too.
              </div>
            </div>
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
          {!recording && !processing && !failed && (
            <button className="btn-sodium btn-pill" onClick={() => { retire(); start(camStream); }} style={{ fontSize: 14, padding: "12px 22px" }}>
              ● {lastClip?.status === "failed" ? "Try again" : lastClip ? "Record another" : "Start recording"}
            </button>
          )}
          {recording && (
            <button className="btn-sodium btn-pill" onClick={stop} style={{ fontSize: 14, padding: "12px 22px" }}>
              ■ Stop &amp; get link
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
          <button
            className={"btn btn-pill" + (camera ? " on" : "")}
            onClick={() => setCamera((c) => !c)}
            style={camera ? { borderColor: "var(--signal)" } : undefined}
          >
            📷 Camera {camera ? "on" : "off"}
          </button>
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
        {camStream && <CameraBubble key="cam" stream={camStream} />}
      </AnimatePresence>
      <AnimatePresence>
        {showPrompter && <Prompter key="prompter" onClose={() => setShowPrompter(false)} />}
      </AnimatePresence>
    </div>
  );
}

function CameraBubble({ stream }: { stream: MediaStream }) {
  const reduced = usePrefersReducedMotion();
  const ref = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    const v = ref.current;
    if (v) {
      v.srcObject = stream;
      v.play().catch(() => {});
    }
  }, [stream]);
  return (
    <motion.div
      className="cam-bubble"
      title="This camera bubble is baked into your recording (bottom-right)"
      initial={reduced ? false : { opacity: 0, y: 20, scale: 0.9 }}
      animate={{ opacity: 1, y: 0, scale: 1, transition: { type: "spring", stiffness: 280, damping: 24 } }}
      exit={{ opacity: 0, y: 12, transition: { duration: 0.16 } }}
    >
      <video ref={ref} muted playsInline />
    </motion.div>
  );
}
