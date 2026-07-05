// Service worker: owns recording state, captures network via webRequest, buffers events from
// the content script, drives tab video+audio capture (via an offscreen document — MV3 service
// workers have no DOM and can't run MediaRecorder themselves), and on Stop fuses the trace
// into the same clip the video streamed to. Records a single tab at a time (the active one
// when Record is pressed).

const state = {
  recording: false,
  tabId: null,
  startedAt: 0,
  url: "",
  viewport: { w: 1280, h: 800 },
  events: [],
  reqStart: {}, // requestId -> t_ms, for network durations
  lastResult: null, // { id, url, count, hasVideo } | { error }
  clipId: null, // set once /ingest/stage mints the id (only when tab capture is possible)
  videoCaptured: false,
};

const DEFAULTS = { host: "https://clipxd.com", token: "", includeCamera: false };
const cfg = async () => ({ ...DEFAULTS, ...(await chrome.storage.local.get(["host", "token", "includeCamera"])) });

// Track the last real (non-extension) page that was active, rather than querying "the active
// tab" at Record time. A default_popup isn't part of chrome.tabs in real usage — but this
// guard is worth having regardless: it also means the extension's own pages (e.g. if opened
// full-tab) never become an accidental recording target.
let lastActiveTabId = null;
const isRealPage = (tab) => !!tab && !!tab.url && /^https?:\/\//.test(tab.url);
chrome.tabs.onActivated.addListener(({ tabId }) => {
  chrome.tabs.get(tabId).then((tab) => {
    if (isRealPage(tab)) lastActiveTabId = tabId;
  }).catch(() => {});
});
// onActivated only fires on tab-switch; a tab that's already active and merely navigates
// (e.g. a fresh tab going from about:blank to the real page) needs onUpdated instead.
chrome.tabs.onUpdated.addListener((tabId, _info, tab) => {
  if (tab.active && isRealPage(tab)) lastActiveTabId = tabId;
});
chrome.tabs.query({ active: true, currentWindow: true }).then(([t]) => {
  if (isRealPage(t)) lastActiveTabId = t.id;
}).catch(() => {});

function buffer(evt) {
  if (state.recording) state.events.push(evt);
}

// ---- network capture (observational; needs host_permissions) ----
chrome.webRequest.onBeforeRequest.addListener(
  (d) => {
    if (state.recording && d.tabId === state.tabId) state.reqStart[d.requestId] = Date.now();
  },
  { urls: ["<all_urls>"] },
);
chrome.webRequest.onCompleted.addListener(
  (d) => {
    if (!state.recording || d.tabId !== state.tabId) return;
    const start = state.reqStart[d.requestId];
    delete state.reqStart[d.requestId];
    buffer({
      type: "network",
      t_ms: Date.now(),
      method: d.method,
      url: d.url,
      status: d.statusCode,
      resource_type: d.type,
      duration_ms: start ? Date.now() - start : undefined,
      request_id: String(d.requestId),
    });
  },
  { urls: ["<all_urls>"] },
);
chrome.webRequest.onErrorOccurred.addListener(
  (d) => {
    if (!state.recording || d.tabId !== state.tabId) return;
    const start = state.reqStart[d.requestId];
    delete state.reqStart[d.requestId];
    buffer({ type: "network", t_ms: Date.now(), method: d.method, url: d.url, resource_type: d.type, error_text: d.error, duration_ms: start ? Date.now() - start : undefined, request_id: String(d.requestId) });
  },
  { urls: ["<all_urls>"] },
);

// ---- events relayed from content scripts + commands from the popup ----
chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (msg && msg.__clipxd && msg.evt) {
    if (state.recording && sender.tab && sender.tab.id === state.tabId) buffer(msg.evt);
    return; // no response needed
  }
  if (msg && msg.cmd === "status") {
    sendResponse({ recording: state.recording, count: state.events.length, videoCaptured: state.videoCaptured, lastResult: state.lastResult });
    return true;
  }
  if (msg && msg.cmd === "start") {
    start().then(sendResponse);
    return true;
  }
  if (msg && msg.cmd === "stop") {
    stop().then(sendResponse);
    return true;
  }
});

async function ensureOffscreen() {
  const existing = await chrome.runtime.getContexts({ contextTypes: ["OFFSCREEN_DOCUMENT"] });
  if (existing.length > 0) return;
  await chrome.offscreen.createDocument({
    url: "offscreen.html",
    reasons: ["USER_MEDIA"],
    justification: "Record the active tab's video+audio (via chrome.tabCapture) into a clipxd clip.",
  });
}

/// Get a tabCapture stream id for `tabId`, or null if capture isn't possible (permission
/// denied, another capture already active, etc). Checked BEFORE minting a staged clip, so a
/// capture failure never leaves an orphaned `status: recording` stub server-side — it just
/// falls through to the trace-only path below.
function getTabCaptureStreamId(tabId) {
  return new Promise((resolve) => {
    chrome.tabCapture.getMediaStreamId({ consumerTabId: tabId }, (id) => {
      resolve(chrome.runtime.lastError || !id ? null : id);
    });
  });
}

async function start() {
  const tab = lastActiveTabId != null ? await chrome.tabs.get(lastActiveTabId).catch(() => null) : null;
  if (!tab || !tab.id) return { error: "no page to record — click into the tab you want first" };
  if (/^chrome:|^edge:|^about:|chrome\.google\.com\/webstore/.test(tab.url || "")) {
    return { error: "can't record browser-internal pages" };
  }
  const { host, token, includeCamera } = await cfg();

  // Try tab video+audio capture first. If it's possible, mint the clip up front (instant-link
  // architecture — the share URL exists from record-start) and stream to it; if not, this
  // recording falls back to trace-only (POST /ingest/browser-trace at Stop, as before).
  let clipId = null;
  let videoCaptured = false;
  const streamId = await getTabCaptureStreamId(tab.id);
  if (streamId) {
    try {
      const r = await fetch(host.replace(/\/$/, "") + "/ingest/stage", { method: "POST", headers: token ? { Authorization: "Bearer " + token } : {} });
      if (r.ok) {
        clipId = (await r.json()).id;
        await ensureOffscreen();
        const resp = await chrome.runtime.sendMessage({ target: "offscreen", cmd: "start", streamId, host, token, clipId, includeCamera: !!includeCamera });
        videoCaptured = !!(resp && resp.ok);
      }
    } catch (e) {
      videoCaptured = false;
    }
  }

  state.recording = true;
  state.tabId = tab.id;
  state.startedAt = Date.now();
  state.url = tab.url || "";
  state.events = [];
  state.reqStart = {};
  state.lastResult = null;
  state.clipId = clipId;
  state.videoCaptured = videoCaptured;
  try {
    const [{ result } = {}] = await chrome.scripting.executeScript({ target: { tabId: tab.id }, func: () => ({ w: window.innerWidth, h: window.innerHeight }) });
    if (result) state.viewport = result;
  } catch (e) {
    /* keep default viewport */
  }
  try {
    await chrome.tabs.sendMessage(tab.id, { cmd: "arm" });
  } catch (e) {
    // content script not present (e.g. tab opened before install) — inject then arm
    try {
      await chrome.scripting.executeScript({ target: { tabId: tab.id }, files: ["content.js"] });
      await chrome.tabs.sendMessage(tab.id, { cmd: "arm" });
    } catch (e2) {
      state.recording = false;
      return { error: "could not attach to this tab — reload it and try again" };
    }
  }
  await setBadge("REC");
  return { recording: true, videoCaptured };
}

async function stop() {
  if (!state.recording) return { error: "not recording" };
  state.recording = false;
  await setBadge("");
  if (state.tabId != null) {
    try {
      await chrome.tabs.sendMessage(state.tabId, { cmd: "disarm" });
    } catch (e) {
      /* tab may be gone */
    }
  }
  if (state.videoCaptured) {
    try {
      await chrome.runtime.sendMessage({ target: "offscreen", cmd: "stop" });
    } catch (e) {
      /* offscreen doc may already be gone */
    }
    try {
      await chrome.offscreen.closeDocument();
    } catch (e) {
      /* nothing to close */
    }
  }

  const trace = {
    clipxd_trace_version: "1",
    session_id: state.clipId || "ext-" + state.startedAt,
    captured_by: "clipxd-extension/0.2.0",
    started_at_ms: state.startedAt,
    viewport: state.viewport,
    url: state.url,
    events: state.events.sort((a, b) => a.t_ms - b.t_ms),
  };
  const { host, token } = await cfg();
  const authHeader = token ? { Authorization: "Bearer " + token } : {};
  try {
    // Fusion path: the video already streamed to state.clipId via /ingest/stage during
    // recording — commit it with the trace as the body, so the server merges the two
    // (see merge_browser_trace_into_clip). Fallback: no video was captured, so mint a
    // trace-only clip via the CLI-style ingester endpoint instead.
    const url = state.videoCaptured && state.clipId
      ? `${host.replace(/\/$/, "")}/ingest/stage/${state.clipId}/commit`
      : `${host.replace(/\/$/, "")}/ingest/browser-trace`;
    const r = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...authHeader },
      body: JSON.stringify(trace),
    });
    if (!r.ok) {
      const msg = r.status === 401 ? "login required — set your token in the popup" : "ingest failed (" + r.status + ")";
      state.lastResult = { error: msg };
      return state.lastResult;
    }
    const j = await r.json();
    state.lastResult = { id: j.id, url: host.replace(/\/$/, "") + "/clip/" + j.id, count: trace.events.length, hasVideo: state.videoCaptured };
    return state.lastResult;
  } catch (e) {
    state.lastResult = { error: "network error reaching " + host };
    return state.lastResult;
  }
}

async function setBadge(text) {
  try {
    await chrome.action.setBadgeText({ text });
    await chrome.action.setBadgeBackgroundColor({ color: "#FF7A59" });
  } catch (e) {
    /* action API may be unavailable in some contexts */
  }
}
