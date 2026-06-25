import { useRef, useState } from "react";

export type RecState = "idle" | "recording" | "processing";

// Screen recording in the browser. With `camera`, it composites the screen + a circular
// webcam bubble onto a canvas and records THAT (so the face is baked into the video). The
// resulting webm is POSTed to clipxd-web /ingest → indexed → the page reloads on the new clip.
export function useScreenRecorder(apiBase: string) {
  const [state, setState] = useState<RecState>("idle");
  const [camStream, setCamStream] = useState<MediaStream | null>(null); // for the live preview bubble
  const ref = useRef<{ mr: MediaRecorder; streams: MediaStream[]; raf: number } | null>(null);
  const chunks = useRef<Blob[]>([]);

  const start = async (opts: { camera: boolean }) => {
    try {
      const screen = await navigator.mediaDevices.getDisplayMedia({ video: { frameRate: 30 }, audio: true });
      let cam: MediaStream | null = null;
      if (opts.camera) {
        try { cam = await navigator.mediaDevices.getUserMedia({ video: { width: 640, height: 480 }, audio: false }); }
        catch (e) { console.warn("camera unavailable:", e); }
      }
      setCamStream(cam);
      chunks.current = [];
      const streams = [screen, cam].filter(Boolean) as MediaStream[];
      let recordStream: MediaStream = screen;
      let raf = 0;

      if (cam) {
        const st = screen.getVideoTracks()[0].getSettings();
        const W = st.width ?? 1280;
        const H = st.height ?? 720;
        const sv = document.createElement("video"); sv.srcObject = screen; sv.muted = true; await sv.play();
        const cv = document.createElement("video"); cv.srcObject = cam; cv.muted = true; await cv.play();
        const canvas = document.createElement("canvas"); canvas.width = W; canvas.height = H;
        const ctx = canvas.getContext("2d")!;
        const d = Math.round(H * 0.24);
        const margin = Math.round(H * 0.03);
        const cx = W - d / 2 - margin;
        const cy = H - d / 2 - margin;
        const draw = () => {
          ctx.drawImage(sv, 0, 0, W, H);
          // circular, cover-fit camera bubble bottom-right
          ctx.save();
          ctx.beginPath(); ctx.arc(cx, cy, d / 2, 0, Math.PI * 2); ctx.closePath(); ctx.clip();
          const ar = (cv.videoWidth || 4) / (cv.videoHeight || 3);
          let dw = d, dh = d;
          if (ar > 1) dw = d * ar; else dh = d / ar;
          ctx.drawImage(cv, cx - dw / 2, cy - dh / 2, dw, dh);
          ctx.restore();
          ctx.lineWidth = 5; ctx.strokeStyle = "#fff";
          ctx.beginPath(); ctx.arc(cx, cy, d / 2, 0, Math.PI * 2); ctx.stroke();
          raf = requestAnimationFrame(draw);
        };
        draw();
        const composited = canvas.captureStream(30);
        screen.getAudioTracks().forEach((t) => composited.addTrack(t)); // keep system audio
        recordStream = composited;
      }

      let mr: MediaRecorder;
      try { mr = new MediaRecorder(recordStream, { mimeType: "video/webm" }); } catch { mr = new MediaRecorder(recordStream); }
      mr.ondataavailable = (e) => { if (e.data.size) chunks.current.push(e.data); };
      mr.onstop = async () => {
        cancelAnimationFrame(raf);
        streams.forEach((s) => s.getTracks().forEach((t) => t.stop()));
        setCamStream(null);
        setState("processing");
        const blob = new Blob(chunks.current, { type: "video/webm" });
        try {
          const r = await fetch(`${apiBase}/ingest`, { method: "POST", headers: { "content-type": "video/webm" }, body: blob });
          const j = await r.json();
          if (j.id) { window.location.href = `${location.pathname}?clip=${j.id}&api=${encodeURIComponent(apiBase)}`; return; }
        } catch (e) { console.warn("ingest failed:", e); }
        setState("idle");
      };
      screen.getVideoTracks()[0].addEventListener("ended", () => { if (mr.state !== "inactive") mr.stop(); });
      mr.start();
      ref.current = { mr, streams, raf };
      setState("recording");
    } catch (e) {
      console.warn("screen capture canceled/failed:", e);
      setState("idle");
    }
  };

  const stop = () => ref.current?.mr.stop();
  return { state, camStream, start, stop };
}
