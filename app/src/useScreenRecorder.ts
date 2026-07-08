import { useRef, useState } from "react";
import { postCursor } from "./api";
import { filterCss, bgPresetById, loadImageBg, type CameraConfig } from "./CameraConfig";

export type RecState = "idle" | "counting" | "recording" | "processing" | "failed";
export type RecordMode = "screen" | "voice";

/** Seconds between picking a screen/window and capture actually starting — time to switch
 *  to the right window, close notification popups, etc. Same pattern as Loom/Cap.so. */
export const COUNTDOWN_SECONDS = 3;

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

export interface StartOptions {
  /** Optional camera stream to composite as a bubble (screen mode only). */
  camera?: MediaStream | null;
  /** Live camera filters + background, baked into the canvas so WYSIWYG. */
  cameraConfig?: CameraConfig;
  /** `voice` records the microphone only (no display media, no canvas, no camera). */
  mode?: RecordMode;
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

// Voice-only mode: an audio-only container. Opus-in-webm first (what MediaRecorder prefers on
// Chromium), then a bare audio/webm, then whatever the browser offers. No video bitrate.
function audioRecorderOpts(): MediaRecorderOptions {
  const prefs = ["audio/webm;codecs=opus", "audio/webm", "audio/ogg;codecs=opus"];
  for (const mimeType of prefs) {
    if (typeof MediaRecorder !== "undefined" && MediaRecorder.isTypeSupported(mimeType)) {
      return { mimeType };
    }
  }
  return {};
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
  // Seconds remaining while state === "counting"; null otherwise.
  const [countdown, setCountdown] = useState<number | null>(null);
  const ref = useRef<{ mr: MediaRecorder } | null>(null);
  const chunks = useRef<Blob[]>([]);
  // Resolves the in-flight countdown promise: true → proceed to recording, false → abort.
  // null whenever state !== "counting".
  const countdownResolve = useRef<((proceed: boolean) => void) | null>(null);

  const start = async (opts: StartOptions = {}) => {
    const cameraStream = opts.camera ?? null;
    const mode: RecordMode = opts.mode ?? "screen";
    const cameraConfig = opts.cameraConfig;
    try {
      // Voice-only mode: capture the microphone, no display media. Produces an audio-only
      // clip (has_video: false) whose value is the transcript + styled captions.
      if (mode === "voice") {
        const mic = await navigator.mediaDevices.getUserMedia({
          audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
          video: false,
        });
        return startFromStream(mic, mic, mode, null);
      }
      const screen = await navigator.mediaDevices.getDisplayMedia({
        video: { frameRate: { ideal: 30 }, width: { ideal: 1920 }, height: { ideal: 1080 } },
        audio: true,
      });
      return startFromStream(screen, screen, mode, cameraStream, cameraConfig);
    } catch (e) {
      setState("idle");
      setError((e as Error).message ?? "Could not start recording");
    }
  };

  /** Shared capture-to-commit path for both screen and voice modes. `sourceStream` is what
   *  the user permitted (screen track or mic-only); `recordStream` is what MediaRecorder
   *  consumes — for screen+camera that's a composited canvas, otherwise it's `sourceStream`. */
  const startFromStream = async (
    sourceStream: MediaStream,
    initialRecordStream: MediaStream,
    mode: RecordMode,
    cameraStream: MediaStream | null,
    cameraConfig?: CameraConfig,
  ) => {
    try {
      const screen = sourceStream;
      let recordStream: MediaStream = initialRecordStream;
      let raf = 0;
      const cleanups: Array<() => void> = [() => screen.getTracks().forEach((t) => t.stop())];

      if (cameraStream && cameraStream.getVideoTracks().length && mode === "screen") {
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
        // WYSIWYG camera: bake the live filter + background into the composited canvas so the
        // recorded bubble matches the on-screen preview. `ctx.filter` is supported in the same
        // Chromium/Firefox the recorder already requires.
        const fcss = cameraConfig ? filterCss(cameraConfig.filter) : "none";
        const bg = cameraConfig?.background;
        // Preload a custom uploaded background image once (data URL / object URL) so the rAF
        // draw loop just blits it each frame instead of re-decoding. Presets are pure canvas
        // draws — no preload needed.
        let bgImage: HTMLImageElement | null = null;
        if (bg?.kind === "image" && bg.imageSrc) {
          try { bgImage = await loadImageBg(bg.imageSrc); } catch { /* broken image -> none */ }
        }
        const draw = () => {
          ctx.filter = "none";
          ctx.drawImage(sv, 0, 0, W, H);
          ctx.save();
          ctx.beginPath(); ctx.arc(cx, cy, d / 2, 0, Math.PI * 2); ctx.closePath(); ctx.clip();
          // Clean background behind the camera: a soft blurred halo or a solid/gradient fill,
          // then the sharp camera inset on top — a produced look without ML segmentation.
          if (bg && bg.kind !== "none") {
            const inset = bg.inset ?? 0.82;
            const od = d; // outer (blurred/filled) diameter
            const id = Math.round(od * inset); // sharp inset diameter
            if (bg.kind === "blur") {
              ctx.filter = `blur(${Math.max(1, bg.blur)}px)`;
              const ar0 = (cv.videoWidth || 4) / (cv.videoHeight || 3);
              let bwd = od, bhd = od;
              if (ar0 > 1) bwd = od * ar0; else bhd = od / ar0;
              ctx.drawImage(cv, cx - bwd / 2, cy - bhd / 2, bwd, bhd);
              ctx.filter = "none";
            } else if (bg.kind === "solid") {
              ctx.fillStyle = bg.color;
              ctx.fillRect(cx - od / 2, cy - od / 2, od, od);
            } else if (bg.kind === "gradient") {
              const g = ctx.createLinearGradient(cx - od / 2, cy - od / 2, cx + od / 2, cy + od / 2);
              g.addColorStop(0, bg.color); g.addColorStop(1, bg.color2);
              ctx.fillStyle = g;
              ctx.fillRect(cx - od / 2, cy - od / 2, od, od);
            } else if (bg.kind === "preset") {
              // Google-Meet-style scene preset: a curated gradient drawn into the bubble ring
              // behind the sharp camera inset (clean produced look, no ML segmentation).
              const preset = bgPresetById(bg.presetId);
              if (preset) preset.draw(ctx, cx - od / 2, cy - od / 2, od, od);
              else { ctx.fillStyle = "#0d1117"; ctx.fillRect(cx - od / 2, cy - od / 2, od, od); }
            } else if (bg.kind === "image" && bgImage) {
              // custom uploaded background, "cover"-fitted into the bubble ring.
              const ar = (bgImage.naturalWidth || od) / (bgImage.naturalHeight || od);
              let iw = od, ih = od;
              if (ar > 1) ih = od / ar; else iw = od * ar;
              ctx.drawImage(bgImage, cx - iw / 2, cy - ih / 2, iw, ih);
            }
            // sharp inset camera, filtered
            ctx.beginPath(); ctx.arc(cx, cy, id / 2, 0, Math.PI * 2); ctx.closePath(); ctx.clip();
            const ar = (cv.videoWidth || 4) / (cv.videoHeight || 3);
            let dw = id, dh = id;
            if (ar > 1) dw = id * ar; else dh = id / ar;
            ctx.filter = fcss;
            ctx.drawImage(cv, cx - dw / 2, cy - dh / 2, dw, dh);
          } else {
            const ar = (cv.videoWidth || 4) / (cv.videoHeight || 3);
            let dw = d, dh = d;
            if (ar > 1) dw = d * ar; else dh = d / ar;
            ctx.filter = fcss;
            ctx.drawImage(cv, cx - dw / 2, cy - dh / 2, dw, dh);
          }
          ctx.restore();
          ctx.filter = "none";
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

      // Loom/Cap.so-style countdown: the screen/window/tab is already picked (the browser's
      // native "you are sharing" indicator is up), but actual capture doesn't start until
      // this resolves — gives the user a few seconds to switch to the right window, close a
      // notification popup, etc. Cancellable via `cancelCountdown`/skippable via
      // `skipCountdown`, both wired to `countdownResolve`.
      setState("counting");
      const proceed = await new Promise<boolean>((resolve) => {
        let remaining = COUNTDOWN_SECONDS;
        setCountdown(remaining);
        const tick = window.setInterval(() => {
          remaining -= 1;
          if (remaining <= 0) {
            window.clearInterval(tick);
            resolve(true);
          } else {
            setCountdown(remaining);
          }
        }, 1000);
        countdownResolve.current = (p) => {
          window.clearInterval(tick);
          resolve(p);
        };
      });
      countdownResolve.current = null;
      setCountdown(null);
      if (!proceed) {
        cleanups.forEach((fn) => fn());
        setState("idle");
        return;
      }

      chunks.current = [];

      // Voice-only: pick an audio-only mime. Screen mode keeps VP9/VP8 video.
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
      try {
        mr = new MediaRecorder(recordStream, mode === "voice" ? audioRecorderOpts() : recorderOpts());
      } catch { mr = new MediaRecorder(recordStream); }

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

      // In screen mode, the browser's native "stop sharing" control should end the recording
      // too. Voice mode has no video track to listen on — the mic's own track ended event
      // would fire on a hard device unplug, which MediaRecorder surfaces via onstop anyway.
      const vt = screen.getVideoTracks()[0];
      if (vt) vt.addEventListener("ended", () => { if (mr.state !== "inactive") mr.stop(); });
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
    if (countdownResolve.current) {
      countdownResolve.current(false); // still counting down — Stop cancels, nothing was recorded
      return;
    }
    const mr = ref.current?.mr;
    if (!mr || mr.state === "inactive") return;
    mr.stop();
  };

  /** "Start now" — skip the rest of the countdown and begin capturing immediately. */
  const skipCountdown = () => countdownResolve.current?.(true);
  /** Abort during the countdown — no recording happens, back to idle. */
  const cancelCountdown = () => countdownResolve.current?.(false);

  return { state, error, countdown, start, stop, skipCountdown, cancelCountdown };
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
