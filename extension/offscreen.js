// Runs in the offscreen document (real DOM) — the only MV3 context that can call
// getUserMedia/MediaRecorder. background.js hands this a tabCapture stream id; this file
// turns it into a MediaStream, records it, and PUTs chunks straight to the existing chunked
// streaming-ingest endpoint (/ingest/stage/:id) as they're produced — no video ever passes
// back through the message bus, only small JSON acks do.

let mediaRecorder = null;
let audioCtx = null;
let seq = 0;
let host = "";
let token = "";
let clipId = "";
let lastUploadPromise = Promise.resolve();

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
      .then(() => sendResponse({ ok: true }))
      .catch((e) => sendResponse({ ok: false, error: String((e && e.message) || e) }));
    return true;
  }
});

async function startCapture({ streamId, host: h, token: t, clipId: id }) {
  host = h;
  token = t;
  clipId = id;
  seq = 0;

  const stream = await navigator.mediaDevices.getUserMedia({
    audio: { mandatory: { chromeMediaSource: "tab", chromeMediaSourceId: streamId } },
    video: { mandatory: { chromeMediaSource: "tab", chromeMediaSourceId: streamId } },
  });

  // Capturing a tab's audio mutes its normal output; route it back to the speakers so the
  // person recording still hears their own tab while capture is running.
  audioCtx = new AudioContext();
  audioCtx.createMediaStreamSource(stream).connect(audioCtx.destination);

  mediaRecorder = new MediaRecorder(stream, { mimeType: "video/webm;codecs=vp8,opus" });
  mediaRecorder.ondataavailable = (e) => {
    if (e.data && e.data.size > 0) lastUploadPromise = uploadChunk(e.data);
  };
  // 4s timeslice: small enough that a mid-recording crash loses little, matches the cadence
  // the web recorder already streams at.
  mediaRecorder.start(4000);
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
  if (!mediaRecorder) return;
  await new Promise((resolve) => {
    mediaRecorder.onstop = resolve;
    mediaRecorder.stop(); // triggers one final ondataavailable before onstop fires
  });
  await lastUploadPromise; // make sure that final chunk's PUT has actually completed
  mediaRecorder.stream.getTracks().forEach((tr) => tr.stop());
  mediaRecorder = null;
  if (audioCtx) {
    await audioCtx.close().catch(() => {});
    audioCtx = null;
  }
}
