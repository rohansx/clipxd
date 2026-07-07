// Runs in the offscreen document (real DOM) — the only MV3 context that can call
// getUserMedia/MediaRecorder. background.js hands this a tabCapture stream id; this file
// turns it into a MediaStream, records it, and PUTs chunks straight to the existing chunked
// streaming-ingest endpoint (/ingest/stage/:id) as they're produced — no video ever passes
// back through the message bus, only small JSON acks do.
//
// Optionally composites a circular webcam "bubble" over the tab video (Loom-style) and mixes
// the presenter's mic narration in with the tab's own audio, when the caller asks for a camera.
//
// Optionally also samples the tab video every few seconds and runs it through a fully local,
// in-browser Moondream2 (WebGPU/wasm via Transformers.js — see local-captioner.js) when the
// caller's account has `caption_mode: "local"` — buffered captions are handed back on stop() so
// background.js can POST them to /clip/:id/local-captions after the trace commit.

let mediaRecorder = null;
let audioCtx = null;
let compositeRaf = null;
let tracksToStop = [];
let seq = 0;
let host = "";
let token = "";
let clipId = "";
let lastUploadPromise = Promise.resolve();
let localCaptioningActive = false;

chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (!msg || msg.target !== "offscreen") return;
  if (msg.cmd === "start") {
    startCapture(msg)
      .then(() => sendResponse({ ok: true }))
      .catch((e) => sendResponse({ ok: false, error: String((e && e.message) || e) }));
    return true; // keep the channel open for the async response
  }
  if (msg.cmd === "stop") {
    stopCapture()
      .then((captions) => sendResponse({ ok: true, captions: captions || [] }))
      .catch((e) => sendResponse({ ok: false, error: String((e && e.message) || e) }));
    return true;
  }
});

async function startCapture({ streamId, host: h, token: t, clipId: id, includeCamera, includeLocalCaptioning }) {
  host = h;
  token = t;
  clipId = id;
  seq = 0;
  tracksToStop = [];

  const tabStream = await navigator.mediaDevices.getUserMedia({
    audio: { mandatory: { chromeMediaSource: "tab", chromeMediaSourceId: streamId } },
    video: { mandatory: { chromeMediaSource: "tab", chromeMediaSourceId: streamId } },
  });
  tracksToStop.push(...tabStream.getTracks());

  // Capturing a tab's audio mutes its normal output; route it back to the speakers so the
  // person recording still hears their own tab while capture is running.
  audioCtx = new AudioContext();
  audioCtx.createMediaStreamSource(tabStream).connect(audioCtx.destination);

  let recordStream = tabStream;

  if (includeCamera) {
    let camStream = null;
    try {
      camStream = await navigator.mediaDevices.getUserMedia({ video: { width: 320, height: 320 }, audio: true });
    } catch (e) {
      // Camera/mic denied or unavailable — fall back to tab-only, don't fail the whole recording.
      camStream = null;
    }
    if (camStream) {
      tracksToStop.push(...camStream.getTracks());
      recordStream = await compositeWithCameraBubble(tabStream, camStream, audioCtx);
    }
  }

  localCaptioningActive = false;
  if (includeLocalCaptioning) {
    if (typeof ClipxdLocalCaptioner === "undefined") {
      console.warn("clipxd: local-captioner.js did not load — skipping local captioning");
    } else {
      localCaptioningActive = true;
      // Fire-and-forget: model loading can take a while (first-time download), and must never
      // delay/block the recording itself — see local-captioner.js.
      ClipxdLocalCaptioner.start(tabStream);
    }
  }

  mediaRecorder = new MediaRecorder(recordStream, { mimeType: "video/webm;codecs=vp8,opus" });
  mediaRecorder.ondataavailable = (e) => {
    if (e.data && e.data.size > 0) lastUploadPromise = uploadChunk(e.data);
  };
  // 4s timeslice: small enough that a mid-recording crash loses little, matches the cadence
  // the web recorder already streams at.
  mediaRecorder.start(4000);
}

/// Draw the tab video full-frame onto a canvas, with the webcam feed clipped into a circle in
/// the bottom-right corner (Loom's "camera bubble"), and mix the tab's own audio with the
/// mic's narration into one track. Returns the combined MediaStream to actually record.
async function compositeWithCameraBubble(tabStream, camStream, ctx) {
  const tabVideoEl = document.createElement("video");
  tabVideoEl.srcObject = new MediaStream(tabStream.getVideoTracks());
  tabVideoEl.muted = true;
  await tabVideoEl.play();

  const camVideoEl = document.createElement("video");
  camVideoEl.srcObject = new MediaStream(camStream.getVideoTracks());
  camVideoEl.muted = true;
  await camVideoEl.play();

  const w = tabVideoEl.videoWidth || 1280;
  const h = tabVideoEl.videoHeight || 720;
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const draw2d = canvas.getContext("2d");

  const bubbleR = Math.round(Math.min(w, h) * 0.12); // ~12% of the shorter side
  const cx = w - bubbleR - 24;
  const cy = h - bubbleR - 24;

  const drawFrame = () => {
    draw2d.drawImage(tabVideoEl, 0, 0, w, h);
    draw2d.save();
    draw2d.beginPath();
    draw2d.arc(cx, cy, bubbleR, 0, Math.PI * 2);
    draw2d.closePath();
    draw2d.clip();
    // cover-fit the (roughly square) camera feed into the circle
    const camAspect = (camVideoEl.videoWidth || 1) / (camVideoEl.videoHeight || 1);
    const side = bubbleR * 2;
    let dw = side, dh = side;
    if (camAspect > 1) dw = side * camAspect;
    else dh = side / camAspect;
    draw2d.drawImage(camVideoEl, cx - dw / 2, cy - dh / 2, dw, dh);
    draw2d.restore();
    compositeRaf = requestAnimationFrame(drawFrame);
  };
  drawFrame();

  const canvasStream = canvas.captureStream(30);

  // Mix tab audio + mic audio into one track (MediaRecorder only takes one audio track per
  // stream cleanly) via a destination node; the tab audio is already connected to speakers
  // above, this just ALSO taps it into the mix.
  const dest = ctx.createMediaStreamDestination();
  if (tabStream.getAudioTracks().length) ctx.createMediaStreamSource(new MediaStream(tabStream.getAudioTracks())).connect(dest);
  if (camStream.getAudioTracks().length) ctx.createMediaStreamSource(new MediaStream(camStream.getAudioTracks())).connect(dest);

  const combined = new MediaStream([...canvasStream.getVideoTracks(), ...dest.stream.getAudioTracks()]);
  return combined;
}

async function uploadChunk(blob) {
  const mySeq = seq++;
  const url = `${host.replace(/\/$/, "")}/ingest/stage/${clipId}?seq=${mySeq}`;
  const headers = token ? { Authorization: "Bearer " + token } : {};
  try {
    await fetch(url, { method: "PUT", headers, body: blob });
  } catch (e) {
    // Best-effort: a dropped chunk leaves a gap in the video, not a hard recording failure —
    // the trace/comment on Stop still lands regardless.
    console.error("clipxd: chunk upload failed", e);
  }
}

async function stopCapture() {
  // Stop sampling first (before tearing down tracks below) and grab whatever got buffered —
  // returned to background.js regardless of what happens to the rest of the recorder teardown.
  const localCaptions = localCaptioningActive && typeof ClipxdLocalCaptioner !== "undefined" ? ClipxdLocalCaptioner.stop() : [];
  localCaptioningActive = false;
  if (!mediaRecorder) return localCaptions;
  await new Promise((resolve) => {
    mediaRecorder.onstop = resolve;
    mediaRecorder.stop(); // triggers one final ondataavailable before onstop fires
  });
  await lastUploadPromise; // make sure that final chunk's PUT has actually completed
  if (compositeRaf) {
    cancelAnimationFrame(compositeRaf);
    compositeRaf = null;
  }
  // Stop both the underlying source tracks (tab/cam, tracked separately since compositing
  // wraps them in new stream objects) and whatever the recorder itself was actually fed
  // (the raw tab stream when not compositing; the canvas+mixed-audio stream when it is).
  mediaRecorder.stream.getTracks().forEach((tr) => tr.stop());
  tracksToStop.forEach((tr) => tr.stop());
  tracksToStop = [];
  mediaRecorder = null;
  if (audioCtx) {
    await audioCtx.close().catch(() => {});
    audioCtx = null;
  }
  return localCaptions;
}
