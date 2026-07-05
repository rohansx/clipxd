const $ = (id) => document.getElementById(id);
const recBtn = $("rec");
const statusEl = $("status");
const hostIn = $("host");
const tokenIn = $("token");

// load config
chrome.storage.local.get(["host", "token"]).then((c) => {
  hostIn.value = c.host || "https://clipxd.com";
  tokenIn.value = c.token || "";
});
const saveCfg = () => chrome.storage.local.set({ host: hostIn.value.trim(), token: tokenIn.value.trim() });
hostIn.addEventListener("change", saveCfg);
tokenIn.addEventListener("change", saveCfg);

function paint(st) {
  const on = st && st.recording;
  recBtn.textContent = on ? "■ Stop & save clip" : "● Record this tab";
  recBtn.classList.toggle("on", !!on);
  if (on) {
    statusEl.textContent = "Recording… " + (st.count || 0) + " events captured";
    statusEl.className = "status";
  } else if (st && st.lastResult) {
    const r = st.lastResult;
    if (r.error) {
      statusEl.innerHTML = "";
      statusEl.textContent = r.error;
      statusEl.className = "status err";
    } else {
      statusEl.innerHTML = 'Saved ' + (r.count || 0) + ' events → <a href="' + r.url + '" target="_blank">open clip ↗</a>';
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
