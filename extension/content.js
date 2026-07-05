// Isolated-world capture: clicks, input, scroll, in-page navigation, and (relayed from the
// main-world inject.js) console. Stays dormant until the background worker "arms" this tab
// when the user hits Record, so idle tabs never send anything.
(function () {
  let armed = false;

  const send = (evt) => {
    if (!armed) return;
    try {
      chrome.runtime.sendMessage({ __clipxd: true, evt });
    } catch (e) {
      /* the worker may be asleep between events — the next one re-wakes it */
    }
  };

  const now = () => Date.now();

  // A short, human-legible target descriptor: tag + #id + first class, plus a text label.
  function describe(el) {
    if (!el || el.nodeType !== 1) return { target: "", label: null };
    let sel = el.tagName.toLowerCase();
    if (el.id) sel += "#" + el.id;
    else if (el.classList && el.classList.length) sel += "." + el.classList[0];
    const label =
      (el.getAttribute && (el.getAttribute("aria-label") || el.getAttribute("placeholder"))) ||
      (el.innerText || el.value || "").trim().slice(0, 80) ||
      (el.getAttribute && el.getAttribute("title")) ||
      null;
    return { target: sel, label: label || null };
  }

  const isSecret = (el) =>
    el && (el.type === "password" || /cc-number|cc-csc|creditcard|card-number/i.test(el.autocomplete || "" + el.name));

  // clicks
  document.addEventListener(
    "click",
    (e) => {
      const d = describe(e.target);
      send({ type: "click", t_ms: now(), click_kind: e.button === 2 ? "right" : "left", target: d.target, label: d.label, x: Math.round(e.clientX), y: Math.round(e.clientY) });
    },
    true,
  );

  // input / change (value masked for secrets; Enter marks a submit)
  const onInput = (e, submit) => {
    const el = e.target;
    if (!el || !("value" in el)) return;
    const d = describe(el);
    const masked = isSecret(el);
    send({ type: "input", t_ms: now(), target: d.target, label: d.label, value: masked ? "" : String(el.value || "").slice(0, 200), masked, submit: !!submit });
  };
  document.addEventListener("change", (e) => onInput(e, false), true);
  document.addEventListener(
    "keydown",
    (e) => {
      if (e.key === "Enter" && e.target && "value" in e.target) onInput(e, true);
    },
    true,
  );

  // scroll (throttled to ~4/s)
  let lastScroll = 0;
  document.addEventListener(
    "scroll",
    () => {
      const t = now();
      if (t - lastScroll < 250) return;
      lastScroll = t;
      send({ type: "scroll", t_ms: t, x: Math.round(window.scrollX), y: Math.round(window.scrollY) });
    },
    true,
  );

  // in-page navigation (SPA route changes)
  let lastUrl = location.href;
  const navCheck = (kind) => {
    if (location.href !== lastUrl) {
      const from = lastUrl;
      lastUrl = location.href;
      send({ type: "navigate", t_ms: now(), url: location.href, from, nav_kind: kind, title: document.title });
    }
  };
  window.addEventListener("popstate", () => navCheck("popstate"));
  window.addEventListener("hashchange", () => navCheck("hashchange"));
  // patch pushState/replaceState to catch SPA nav
  ["pushState", "replaceState"].forEach((m) => {
    const orig = history[m];
    history[m] = function () {
      const r = orig.apply(this, arguments);
      setTimeout(() => navCheck(m === "pushState" ? "push" : "replace"), 0);
      return r;
    };
  });

  // console relayed from the main world (inject.js)
  window.addEventListener("message", (e) => {
    if (e.source !== window || !e.data || !e.data.__clipxd_cap) return;
    if (e.data.kind === "console") {
      send({ type: "console", t_ms: now(), level: e.data.level, text: e.data.text, uncaught: !!e.data.uncaught });
    }
  });

  // arm / disarm from the background worker
  chrome.runtime.onMessage.addListener((msg) => {
    if (!msg || !msg.cmd) return;
    if (msg.cmd === "arm") {
      armed = true;
      lastUrl = location.href;
      // seed with where we are + a coarse DOM snapshot
      send({ type: "navigate", t_ms: now(), url: location.href, nav_kind: "load", title: document.title });
      send({ type: "dom_snapshot", t_ms: now(), url: location.href, node_count: document.getElementsByTagName("*").length, text: (document.body ? document.body.innerText : "").slice(0, 4000) });
    } else if (msg.cmd === "disarm") {
      armed = false;
    }
  });
})();
