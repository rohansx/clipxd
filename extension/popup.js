const $ = (id) => document.getElementById(id);
const recBtn = $("rec");
const statusEl = $("status");
const hostIn = $("host");
const tokenIn = $("token");
const camIn = $("cam");

// load config
chrome.storage.local.get(["host", "token", "includeCamera"]).then((c) => {
  hostIn.value = c.host || "https://clipxd.com";
  tokenIn.value = c.token || "";
  camIn.checked = !!c.includeCamera;
});
const saveCfg = () => chrome.storage.local.set({ host: hostIn.value.trim(), token: tokenIn.value.trim(), includeCamera: camIn.checked });
hostIn.addEventListener("change", saveCfg);
tokenIn.addEventListener("change", saveCfg);
camIn.addEventListener("change", saveCfg);

function paint(st) {
  const on = st && st.recording;
  recBtn.textContent = on ? "■ Stop & save clip" : "● Record this tab";
  recBtn.classList.toggle("on", !!on);
  if (on) {
    const kind = st.videoCaptured ? "video + " + (st.count || 0) + " events" : (st.count || 0) + " events (no video — tab capture unavailable)";
    statusEl.textContent = "Recording… " + kind;
    statusEl.className = "status";
  } else if (st && st.lastResult) {
    const r = st.lastResult;
    if (r.error) {
      statusEl.innerHTML = "";
      statusEl.textContent = r.error;
      statusEl.className = "status err";
    } else {
      const kind = r.hasVideo ? "clip (video + " + (r.count || 0) + " events)" : (r.count || 0) + " events (trace only, no video)";
      statusEl.innerHTML = 'Saved ' + kind + ' → <a href="' + r.url + '" target="_blank">open clip ↗</a>';
      statusEl.className = "status";
    }
  } else {
    statusEl.textContent = "";
  }
}

async function refresh() {
  const st = await chrome.runtime.sendMessage({ cmd: "status" });
  paint(st);
}

recBtn.addEventListener("click", async () => {
  await saveCfg();
  const st = await chrome.runtime.sendMessage({ cmd: "status" });
  recBtn.disabled = true;
  if (st && st.recording) {
    const r = await chrome.runtime.sendMessage({ cmd: "stop" });
    paint({ recording: false, lastResult: r });
  } else {
    const r = await chrome.runtime.sendMessage({ cmd: "start" });
    if (r && r.error) paint({ recording: false, lastResult: { error: r.error } });
    else paint({ recording: true, count: 0 });
  }
  recBtn.disabled = false;
});

refresh();
setInterval(refresh, 1500);
