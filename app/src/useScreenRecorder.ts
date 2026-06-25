import { useRef, useState } from "react";

export type RecState = "idle" | "recording" | "processing";

// Best available high-quality recorder config (VP9 → VP8 → default, ~8 Mbps).
function recorderOpts(): MediaRecorderOptions {
  const prefs = ["video/webm;codecs=vp9", "video/webm;codecs=vp8", "video/webm"];
  for (const mimeType of prefs) {
    if (typeof MediaRecorder !== "undefined" && MediaRecorder.isTypeSupported(mimeType)) {
      return { mimeType, videoBitsPerSecond: 8_000_000 };
    }
  }
  return { videoBitsPerSecond: 8_000_000 };
}

// Screen recording in the browser. A supplied camera stream is composited as a circular
// bubble onto a canvas and recorded (face baked in). High-bitrate VP9 + 1080p when available.
// The webm is POSTed to /ingest, the captured cursor to /cursor (zoom follows it), then reload.
export function useScreenRecorder(apiBase: string) {
  const [state, setState] = useState<RecState>("idle");
  const ref = useRef<{ mr: MediaRecorder } | null>(null);
  const chunks = useRef<Blob[]>([]);

  const start = async (cameraStream: MediaStream | null) => {
    try {
      const screen = await navigator.mediaDevices.getDisplayMedia({
        video: { frameRate: { ideal: 30 }, width: { ideal: 1920 }, height: { ideal: 1080 } },
        audio: true,
      });
      chunks.current = [];
      let recordStream: MediaStream = screen;
      let raf = 0;
      const cleanups: Array<() => void> = [() => screen.getTracks().forEach((t) => t.stop())];

      if (cameraStream && cameraStream.getVideoTracks().length) {
        const st = screen.getVideoTracks()[0].getSettings();
        const W = st.width ?? 1920;
        const H = st.height ?? 1080;
        const sv = document.createElement("video"); sv.srcObject = screen; sv.muted = true; await sv.play();
        const cv = document.createElement("video"); cv.srcObject = cameraStream; cv.muted = true; await cv.play();
        const canvas = document.createElement("canvas"); canvas.width = W; canvas.height = H;
        const ctx = canvas.getContext("2d")!;
        const d = Math.round(H * 0.24);
        const margin = Math.round(H * 0.03);
        const cx = W - d / 2 - margin;
        const cy = H - d / 2 - margin;
        const draw = () => {
          ctx.drawImage(sv, 0, 0, W, H);
          ctx.save();
          ctx.beginPath(); ctx.arc(cx, cy, d / 2, 0, Math.PI * 2); ctx.closePath(); ctx.clip();
          const ar = (cv.videoWidth || 4) / (cv.videoHeight || 3);
          let dw = d, dh = d;
          if (ar > 1) dw = d * ar; else dh = d / ar;
          ctx.drawImage(cv, cx - dw / 2, cy - dh / 2, dw, dh);
          ctx.restore();
          ctx.lineWidth = 6; ctx.strokeStyle = "#fff";
          ctx.beginPath(); ctx.arc(cx, cy, d / 2, 0, Math.PI * 2); ctx.stroke();
          raf = requestAnimationFrame(draw);
        };
        draw();
        cleanups.push(() => cancelAnimationFrame(raf));
        const comp = canvas.captureStream(30);
        screen.getAudioTracks().forEach((t) => comp.addTrack(t)); // keep system audio
        recordStream = comp;
      }

      let mr: MediaRecorder;
      try { mr = new MediaRecorder(recordStream, recorderOpts()); } catch { mr = new MediaRecorder(recordStream); }
      mr.ondataavailable = (e) => { if (e.data.size) chunks.current.push(e.data); };

      // capture the cursor (screen-normalized) so the zoom follows it, Screen-Studio style.
      // Pointer events only fire while the cursor is over this window — best for web content;
      // the native recorder captures the OS cursor globally.
      const startMs = Date.now();
      const cursors: { t: number; x: number; y: number }[] = [];
      const clicks: { t: number; x: number; y: number }[] = [];
      const clamp01 = (v: number) => Math.max(0, Math.min(1, v));
      let lastSample = -1;
      const onMove = (e: PointerEvent) => {
        const t = (Date.now() - startMs) / 1000;
        if (t - lastSample < 0.04) return;
        lastSample = t;
        cursors.push({ t, x: clamp01(e.screenX / window.screen.width), y: clamp01(e.screenY / window.screen.height) });
      };
      const onDown = (e: PointerEvent) => {
        clicks.push({ t: (Date.now() - startMs) / 1000, x: clamp01(e.screenX / window.screen.width), y: clamp01(e.screenY / window.screen.height) });
      };
      window.addEventListener("pointermove", onMove, true);
      window.addEventListener("pointerdown", onDown, true);
      cleanups.push(() => {
        window.removeEventListener("pointermove", onMove, true);
        window.removeEventListener("pointerdown", onDown, true);
      });

      mr.onstop = async () => {
        cleanups.forEach((fn) => fn());
        setState("processing");
        const blob = new Blob(chunks.current, { type: "video/webm" });
        try {
          const r = await fetch(`${apiBase}/ingest`, { method: "POST", headers: { "content-type": "video/webm" }, body: blob });
          const j = await r.json();
          if (j.id) {
            if (cursors.length || clicks.length) {
              try {
                await fetch(`${apiBase}/clip/${j.id}/cursor`, {
                  method: "POST",
                  headers: { "content-type": "application/json" },
                  body: JSON.stringify({ cursors, clicks, keys: [] }),
                });
              } catch (e) { console.warn("cursor post failed:", e); }
            }
            window.location.href = `${location.pathname}?clip=${j.id}&api=${encodeURIComponent(apiBase)}`;
            return;
          }
        } catch (e) { console.warn("ingest failed:", e); }
        setState("idle");
      };
      screen.getVideoTracks()[0].addEventListener("ended", () => { if (mr.state !== "inactive") mr.stop(); });
      mr.start();
      ref.current = { mr };
      setState("recording");
    } catch (e) {
      console.warn("screen capture canceled/failed:", e);
      setState("idle");
    }
  };

  const stop = () => ref.current?.mr.stop();
  return { state, start, stop };
}
