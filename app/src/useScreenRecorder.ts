import { useRef, useState } from "react";
import { ingest, postCursor } from "./api";

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
// Streaming upload: MediaRecorder emits a chunk every 15 s which is immediately PUT to
// /ingest/stage/:session — so by the time the user stops, most of the video is already
// on the server. Only the last ≤15 s chunk needs to upload after stop.
export function useScreenRecorder(apiBase: string, onClipReady?: (id: string) => void) {
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

      // Streaming stage session — pre-create on server so dir exists before first chunk arrives.
      const sessionId = `stg_${Date.now().toString(16).padStart(16, "0")}`;
      fetch(`${apiBase}/ingest/stage`, { method: "POST", credentials: "include" }).catch(() => {});

      let chunkSeq = 0;
      // Track the last PUT so we can await it before commit (ensures final chunk is on server).
      let lastChunkPut: Promise<unknown> = Promise.resolve();

      const cursors: { t: number; x: number; y: number }[] = [];
      const clicks: { t: number; x: number; y: number }[] = [];
      const startMs = Date.now();
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

      let mr: MediaRecorder;
      try { mr = new MediaRecorder(recordStream, recorderOpts()); } catch { mr = new MediaRecorder(recordStream); }

      mr.ondataavailable = (e) => {
        if (!e.data.size) return;
        chunks.current.push(e.data);
        // Upload every chunk (both mid-recording 15 s slices and the final flush on stop)
        // as a fire-and-forget PUT. Track the promise so onstop can await the last one.
        const seq = chunkSeq++;
        lastChunkPut = fetch(`${apiBase}/ingest/stage/${sessionId}?seq=${seq}`, {
          method: "PUT",
          body: e.data,
          credentials: "include",
        }).catch(() => {});
      };

      mr.onstop = async () => {
        cleanups.forEach((fn) => fn());
        setState("processing");

        // Wait for the final chunk PUT to land before calling commit.
        await lastChunkPut;

        try {
          const res = await fetch(`${apiBase}/ingest/stage/${sessionId}/commit`, {
            method: "POST",
            credentials: "include",
          });
          if (res.ok) {
            const data = await res.json() as { id?: string };
            const id = data.id;
            if (id) {
              if (cursors.length || clicks.length) {
                try { await postCursor(id, { cursors, clicks, keys: [] }, apiBase); } catch { /* non-fatal */ }
              }
              setState("idle");
              if (onClipReady) onClipReady(id);
              else window.location.href = `${location.pathname}?clip=${id}&api=${encodeURIComponent(apiBase)}`;
              return;
            }
          }
        } catch { /* fall through to full-blob fallback */ }

        // Fallback: assemble full blob and POST to /ingest (original behaviour).
        const blob = new Blob(chunks.current, { type: "video/webm" });
        try {
          const id = await ingest(blob, apiBase);
          if (id) {
            if (cursors.length || clicks.length) {
              try { await postCursor(id, { cursors, clicks, keys: [] }, apiBase); } catch { /* non-fatal */ }
            }
            setState("idle");
            if (onClipReady) onClipReady(id);
            else window.location.href = `${location.pathname}?clip=${id}&api=${encodeURIComponent(apiBase)}`;
            return;
          }
        } catch { /* nothing more to try */ }
        setState("idle");
      };

      screen.getVideoTracks()[0].addEventListener("ended", () => { if (mr.state !== "inactive") mr.stop(); });
      // 15 000 ms timeslice: emit a chunk every 15 seconds so upload can start during recording.
      mr.start(15_000);
      ref.current = { mr };
      setState("recording");
    } catch {
      setState("idle");
    }
  };

  const stop = () => ref.current?.mr.stop();
  return { state, start, stop };
}
