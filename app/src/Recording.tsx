import { useEffect, useRef, useState } from "react";
import { useScreenRecorder } from "./useScreenRecorder";
import { Prompter } from "./Prompter";
import { apiBase } from "./api";

interface RecordingProps {
  onClipReady: (id: string) => void;
  showToast: (m: string) => void;
}

const MIC_BARS = Array.from({ length: 56 }, (_, i) => ({
  h: 14 + Math.round(Math.abs(Math.sin(i * 0.7) * Math.cos(i * 0.3)) * 40),
  delay: (i % 12) * 0.08,
}));

export function Recording({ onClipReady, showToast }: RecordingProps) {
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
  }, [camera]);

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
            <div className="mock-bar">
              <i style={{ background: "#ec6a5e" }} />
              <i style={{ background: "#f4be4f" }} />
              <i style={{ background: "#61c454" }} />
              <span className="mock-url">your screen → clipxd</span>
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
              <span key={i} style={{ height: recording ? `${b.h}%` : "10%", animationDelay: `${b.delay}s`, animationPlayState: recording ? "running" : "paused" }} />
            ))}
          </div>
        </div>

        <div className="toolbar">
          {!recording && !processing && (
            <button className="btn-sodium" onClick={() => start(camStream)} style={{ borderRadius: 0, fontSize: 14, padding: "12px 22px" }}>
              ● Start recording
            </button>
          )}
          {recording && (
            <button className="btn-sodium" onClick={stop} style={{ borderRadius: 0, fontSize: 14, padding: "12px 22px" }}>
              ■ Stop &amp; get link
            </button>
          )}
          {processing && (
            <button className="btn" disabled style={{ borderRadius: 0, fontSize: 14, padding: "12px 22px" }}>
              <span className="spin" /> Indexing…
            </button>
          )}
          <button className={"btn" + (camera ? " " : "")} onClick={() => setCamera((c) => !c)} style={{ borderRadius: 0, borderColor: camera ? "var(--signal)" : undefined }}>
            📷 Camera {camera ? "on" : "off"}
          </button>
          <button className="btn" onClick={() => setShowPrompter((s) => !s)} style={{ borderRadius: 0, borderColor: showPrompter ? "var(--signal)" : undefined }}>
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
          <Hint icon="●" label="veyo salience gate" detail="emits a frame only when the scene changes" />
          <Hint icon="◎" label="cursor-follow auto-zoom" detail="zoom tracks your pointer + clicks" />
          <Hint icon="▦" label="OCR + captions" detail="on-screen text + scene captions, timestamped" />
          <Hint icon="◈" label="agent-queryable" detail="ask the clip the moment it finishes" />
        </div>
      </div>

      {camStream && <CameraBubble stream={camStream} />}
      {showPrompter && <Prompter onClose={() => setShowPrompter(false)} />}
    </div>
  );
}

function Hint({ icon, label, detail }: { icon: string; label: string; detail: string }) {
  return (
    <div className="read-row" style={{ cursor: "default" }}>
      <span className="t" style={{ width: 18 }}>{icon}</span>
      <div>
        <div style={{ fontSize: 13.5, fontWeight: 600 }}>{label}</div>
        <div className="mono" style={{ fontSize: 11, color: "var(--text-3)" }}>{detail}</div>
      </div>
    </div>
  );
}

function CameraBubble({ stream }: { stream: MediaStream }) {
  const ref = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    const v = ref.current;
    if (v) {
      v.srcObject = stream;
      v.play().catch(() => {});
    }
  }, [stream]);
  return (
    <div className="cam-bubble" title="This camera bubble is baked into your recording (bottom-right)">
      <video ref={ref} muted playsInline />
    </div>
  );
}
