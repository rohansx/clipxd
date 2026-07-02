import { useRef, useState } from "react";
import { postCursor } from "./api";

export type RecState = "idle" | "recording" | "processing" | "failed";

export interface RecorderCallbacks {
  /** Fires as soon as the server mints the clip id at record start (instant link) —
   *  the share URL already resolves while the recording is still running. Not fired
   *  against older servers that only return a `stg_` session id. */
  onRecordingLink?: (id: string) => void;
  /** Fires the moment the recorder stops. Persist a "saving" record to
   *  localStorage NOW so a hard refresh during upload doesn't lose state. */
  onPending?: (stopId: string) => void;
  /** Fires with the server-issued clip id once Phase 1 is committed. */
  onClipReady?: (id: string) => void;
  /** Fires if every upload path timed out or errored. */
  onError?: (reason: string) => void;
}

// Total time we'll wait between Stop and a usable id. The server's commit
// involves concatenating chunks (fast), storing Phase 1 (fast), then
// spawning Phase 2 enrichment in the background. Phase 1 should be
// < 5s; we give it 60s before falling back to /ingest, then a further
// 60s on /ingest, for a total worst-case of ~120s. Anything past that
// is a real failure (server hung, OOM, network) and we surface an error
// rather than spin forever.
const COMMIT_TIMEOUT_MS = 60_000;
const INGEST_TIMEOUT_MS = 60_000;

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

/** Wrap a fetch with an AbortController and reject on timeout. */
function fetchWithTimeout(url: string, init: RequestInit, timeoutMs: number, label: string): Promise<Response> {
  const ctl = new AbortController();
  const to = window.setTimeout(() => ctl.abort(), timeoutMs);
  return fetch(url, { ...init, signal: ctl.signal })
    .catch((e) => { throw new Error(`${label} ${e?.name === "AbortError" ? "timed out after " + (timeoutMs / 1000) + "s" : "failed: " + (e?.message ?? e)}`); })
    .finally(() => window.clearTimeout(to));
}

// Screen recording in the browser. A supplied camera stream is composited as a circular
// bubble onto a canvas and recorded (face baked in). High-bitrate VP9 + 1080p when available.
// Streaming upload: MediaRecorder emits a chunk every 15 s which is immediately PUT to
// /ingest/stage/:session — so by the time the user stops, most of the video is already
// on the server. Only the last ≤15 s chunk needs to upload after stop.
export function useScreenRecorder(apiBase: string, callbacks: RecorderCallbacks = {}) {
  const { onRecordingLink, onPending, onClipReady, onError } = callbacks;
  const [state, setState] = useState<RecState>("idle");
  // Distinct "we tried, here's why" reason once state === "failed".
  const [error, setError] = useState<string | null>(null);
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

      // Streaming stage session — the server creates the on-disk session dir and hands back
      // its id; chunk PUTs must target *that* id (not a locally-generated one) or they 404.
      // New servers mint the real clip id here (instant link): the session IS the clip, and
      // the share URL resolves from this moment. `id` is absent on older servers.
      const sessionPromise: Promise<string | null> = fetch(`${apiBase}/ingest/stage`, {
        method: "POST",
        credentials: "include",
      })
        .then((r) => (r.ok ? r.json() : null))
        .then((d: { session?: string; id?: string } | null) => {
          const session = d?.session ?? d?.id ?? null;
          if (d?.id && d.id.startsWith("clp_") && onRecordingLink) onRecordingLink(d.id);
          return session;
        })
        .catch(() => null);

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
        // as a fire-and-forget PUT, once the session id is known. Track the promise so
        // onstop can await the last one.
        const seq = chunkSeq++;
        const chunkData = e.data;
        lastChunkPut = sessionPromise.then((session) => {
          if (!session) return;
          return fetch(`${apiBase}/ingest/stage/${session}?seq=${seq}`, {
            method: "PUT",
            body: chunkData,
            credentials: "include",
          }).catch(() => {});
        });
      };

      mr.onstop = async () => {
        cleanups.forEach((fn) => fn());
        const stopId = `${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 8)}`;
        // Persist a "saving" record the instant the user clicks Stop.  A mid-upload
        // refresh now keeps the banner instead of dropping everything on the floor.
        if (onPending) onPending(stopId);
        setState("processing");

        const failed = (reason: string) => {
          setError(reason);
          setState("failed");
          if (onError) onError(reason);
        };
        const ok = (id: string) => {
          setError(null);
          setState("idle");
          if (cursors.length || clicks.length) {
            postCursor(id, { cursors, clicks, keys: [] }, apiBase).catch(() => {});
          }
          if (onClipReady) onClipReady(id);
          else window.location.href = `${location.pathname}?clip=${id}&api=${encodeURIComponent(apiBase)}`;
        };

        try {
          // Wait for the final chunk PUT to land before calling commit.
          await lastChunkPut;
          const session = await sessionPromise;

          if (session) {
            let res: Response;
            try {
              res = await fetchWithTimeout(
                `${apiBase}/ingest/stage/${session}/commit`,
                { method: "POST", credentials: "include" },
                COMMIT_TIMEOUT_MS,
                "commit",
              );
            } catch (e) {
              // Stage commit timed out or errored — try the original /ingest as a
              // fallback rather than hanging the UI.
              console.warn("[clipxd] stage commit failed, falling back to /ingest:", (e as Error).message);
              res = new Response("", { status: 599 });
            }
            if (res.ok) {
              const data = await res.json().catch(() => ({})) as { id?: string };
              if (data.id) { ok(data.id); return; }
            }
            // 5xx or no id → fall through to the original /ingest path.
          }

          // Fallback: assemble full blob and POST to /ingest. For an instant-link session
          // the share URL for `session` may already be copied/shared — pass it as ?reuse=
          // so the recording lands under that same URL instead of a fresh, unshared id.
          const blob = new Blob(chunks.current, { type: "video/webm" });
          if (blob.size === 0) { failed("Recording produced no data"); return; }
          const reuse = session && session.startsWith("clp_") ? session : null;
          try {
            const id = await ingestWithTimeout(blob, apiBase, INGEST_TIMEOUT_MS, reuse);
            if (id) { ok(id); return; }
            failed("Upload did not return a clip id (server unreachable or timed out)");
          } catch (e) {
            failed(`Upload failed: ${(e as Error).message}`);
          }
        } catch (e) {
          failed(`Unexpected error: ${(e as Error).message}`);
        }
      };

      screen.getVideoTracks()[0].addEventListener("ended", () => { if (mr.state !== "inactive") mr.stop(); });
      // 15 000 ms timeslice: emit a chunk every 15 seconds so upload can start during recording.
      mr.start(15_000);
      ref.current = { mr };
      setState("recording");
      setError(null);
    } catch (e) {
      setState("idle");
      setError((e as Error).message ?? "Could not start recording");
    }
  };

  const stop = () => {
    const mr = ref.current?.mr;
    if (!mr || mr.state === "inactive") return;
    mr.stop();
  };

  return { state, error, start, stop };
}

async function ingestWithTimeout(blob: Blob, apiBase: string, timeoutMs: number, reuseId: string | null = null): Promise<string | null> {
  try {
    const url = `${apiBase}/ingest${reuseId ? `?reuse=${encodeURIComponent(reuseId)}` : ""}`;
    const r = await fetchWithTimeout(
      url,
      { method: "POST", headers: { "content-type": "video/webm" }, body: blob, credentials: "include" },
      timeoutMs,
      "/ingest",
    );
    if (!r.ok) return null;
    const j = await r.json().catch(() => ({})) as { id?: string };
    return j.id ?? null;
  } catch {
    return null;
  }
}
