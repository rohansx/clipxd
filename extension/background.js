// Service worker: owns recording state, captures network via webRequest, buffers events from
// the content script, and on Stop assembles the BrowserTrace and POSTs it to the clipxd
// ingest endpoint. Records a single tab at a time (the active one when Record is pressed).

const state = {
  recording: false,
  tabId: null,
  startedAt: 0,
  url: "",
  viewport: { w: 1280, h: 800 },
  events: [],
  reqStart: {}, // requestId -> t_ms, for network durations
  lastResult: null, // { id } | { error }
};

const DEFAULTS = { host: "https://clipxd.com", token: "" };
const cfg = async () => ({ ...DEFAULTS, ...(await chrome.storage.local.get(["host", "token"])) });

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
    sendResponse({ recording: state.recording, count: state.events.length, lastResult: state.lastResult });
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

async function start() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab || !tab.id) return { error: "no active tab" };
  if (/^chrome:|^edge:|^about:|chrome\.google\.com\/webstore/.test(tab.url || "")) {
    return { error: "can't record browser-internal pages" };
  }
  state.recording = true;
  state.tabId = tab.id;
  state.startedAt = Date.now();
  state.url = tab.url || "";
  state.events = [];
  state.reqStart = {};
  state.lastResult = null;
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
  return { recording: true };
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
  const trace = {
    clipxd_trace_version: "1",
    session_id: "ext-" + state.startedAt,
    captured_by: "clipxd-extension/0.1.0",
    started_at_ms: state.startedAt,
    viewport: state.viewport,
    url: state.url,
    events: state.events.sort((a, b) => a.t_ms - b.t_ms),
  };
  const { host, token } = await cfg();
  try {
    const r = await fetch(host.replace(/\/$/, "") + "/ingest/browser-trace", {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(token ? { Authorization: "Bearer " + token } : {}) },
      body: JSON.stringify(trace),
    });
    if (!r.ok) {
      const msg = r.status === 401 ? "login required — set your token in the popup" : "ingest failed (" + r.status + ")";
      state.lastResult = { error: msg };
      return { error: msg };
    }
    const j = await r.json();
    state.lastResult = { id: j.id, url: host.replace(/\/$/, "") + "/clip/" + j.id, count: trace.events.length };
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
