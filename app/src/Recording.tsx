import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import { useScreenRecorder } from "./useScreenRecorder";
import { Prompter } from "./Prompter";
import { apiBase } from "./api";
import { usePrefersReducedMotion } from "./motion";

interface RecordingProps {
  onClipReady: (id: string) => void;
  showToast: (m: string) => void;
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

export function Recording({ onClipReady, showToast }: RecordingProps) {
  const reduced = usePrefersReducedMotion();
  const base = apiBase();
  const { state, start, stop } = useScreenRecorder(base, (id) => {
    onClipReady(id);
  });
  const [camera, setCamera] = useState(false);
  const [showPrompter, setShowPrompter] = useState(false);
  const [camStream, setCamStream] = useState<MediaStream | null>(null);
  const [secs, setSecs] = useState(0);

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

  return (
    <div className="recording">
      <div className="rec-left">
        <div className="rec-status">
          <span className="rec-badge">
            <span className="led" />
            {recording ? "REC" : processing ? "UPLOADING" : "READY"}
          </span>
          <span className="rec-clock">{recording ? clock : "00:00"}</span>
          <span className="rec-hint">screen · 1080p · auto-zoom on{camera ? " · camera" : ""}</span>
        </div>

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
                {recording ? "Recording your screen…" : processing ? "Uploading — link ready in a moment…" : "Press record — pick a screen or window."}
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
          {!recording && !processing && (
            <button className="btn-sodium btn-pill" onClick={() => start(camStream)} style={{ fontSize: 14, padding: "12px 22px" }}>
              ● Start recording
            </button>
          )}
          {recording && (
            <button className="btn-sodium btn-pill" onClick={stop} style={{ fontSize: 14, padding: "12px 22px" }}>
              ■ Stop &amp; get link
            </button>
          )}
          {processing && (
            <button className="btn btn-pill" disabled style={{ fontSize: 14, padding: "12px 22px" }}>
              <span className="spin" /> Indexing…
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
