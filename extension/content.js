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

  // Roles a tag implies when the author didn't write one. Only the handful worth naming in an
  // event row — anything else falls back to the tag name, which reads fine ("clicked div").
  const IMPLICIT_ROLE = {
    A: "link", BUTTON: "button", SELECT: "combobox", TEXTAREA: "textbox",
    SUMMARY: "disclosure", OPTION: "option", NAV: "navigation", FORM: "form",
    H1: "heading", H2: "heading", H3: "heading", H4: "heading", IMG: "image",
  };
  const INPUT_ROLE = {
    checkbox: "checkbox", radio: "radio", range: "slider", submit: "button",
    button: "button", file: "file input", search: "searchbox",
  };

  function roleOf(el) {
    const explicit = el.getAttribute && el.getAttribute("role");
    if (explicit) return explicit;
    if (el.tagName === "INPUT") return INPUT_ROLE[(el.type || "text").toLowerCase()] || "textbox";
    return IMPLICIT_ROLE[el.tagName] || el.tagName.toLowerCase();
  }

  // An approximation of the accessible name, in the order a screen reader resolves it. This is
  // the string that turns "click at (51%, 36%)" into "clicked Generate credentials".
  function accessibleName(el) {
    const attr = (n) => (el.getAttribute && el.getAttribute(n)) || "";
    const byId = attr("aria-labelledby")
      .split(/\s+/)
      .map((id) => (id && document.getElementById(id) ? document.getElementById(id).innerText : ""))
      .join(" ")
      .trim();
    let label = "";
    if (el.labels && el.labels.length) label = el.labels[0].innerText || "";
    const own = (el.innerText || "").trim();
    const name =
      attr("aria-label").trim() || byId || label.trim() || own || attr("title") ||
      attr("placeholder") || attr("alt") || attr("value") || "";
    return name.replace(/\s+/g, " ").trim().slice(0, 80) || null;
  }

  // A PostHog-style element chain: the target plus a few ancestors, each as
  // tag[#id][.class][:nth-of-type]. Enough to locate the element again and to see what part of
  // the page it lived in ("button.primary < form#signup < main"), without shipping the DOM.
  function elementChain(el) {
    const parts = [];
    let node = el;
    for (let depth = 0; node && node.nodeType === 1 && depth < 5; depth++) {
      let bit = node.tagName.toLowerCase();
      if (node.id) {
        bit += "#" + node.id;
        parts.push(bit);
        break; // an id is unique — nothing above it adds information
      }
      if (node.classList && node.classList.length) bit += "." + node.classList[0];
      const parent = node.parentElement;
      if (parent) {
        const sibs = Array.from(parent.children).filter((c) => c.tagName === node.tagName);
        if (sibs.length > 1) bit += ":nth-of-type(" + (sibs.indexOf(node) + 1) + ")";
      }
      parts.push(bit);
      node = node.parentElement;
    }
    return parts.join(" < ");
  }

  /**
   * Describe what was interacted with, semantically.
   *
   * The old version returned "tag.class" plus raw x/y, which is why every event row on a share
   * page read "CLICK AT (51%, 36%)" — a coordinate tells a viewer (or an agent) nothing about
   * what happened. The browser knows the role, the name, and the box; this hands all three over.
   * `target` keeps its old shape so existing consumers and stored clips stay valid.
   */
  function describe(el) {
    if (!el || el.nodeType !== 1) return { target: "", label: null };
    let sel = el.tagName.toLowerCase();
    if (el.id) sel += "#" + el.id;
    else if (el.classList && el.classList.length) sel += "." + el.classList[0];
    let rect = null;
    try {
      const r = el.getBoundingClientRect();
      // Viewport-relative and rounded: enough to place the element, not enough to reconstruct
      // the page. Zero-size elements (detached, display:none) are reported as null.
      if (r && (r.width || r.height)) {
        rect = { x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height) };
      }
    } catch (e) {
      /* getBoundingClientRect can throw on a detached node */
    }
    return {
      target: sel,
      label: accessibleName(el),
      role: roleOf(el),
      chain: elementChain(el),
      rect,
      href: el.tagName === "A" ? (el.getAttribute("href") || "").slice(0, 200) || null : null,
    };
  }

  const isSecret = (el) =>
    el && (el.type === "password" || /cc-number|cc-csc|creditcard|card-number/i.test(el.autocomplete || "" + el.name));

  // clicks
  document.addEventListener(
    "click",
    (e) => {
      const d = describe(e.target);
      send({
        type: "click", t_ms: now(), click_kind: e.button === 2 ? "right" : "left",
        target: d.target, label: d.label, role: d.role, chain: d.chain, rect: d.rect, href: d.href,
        // x/y stay for the cinematic zoom track, which wants a point, not an element.
        x: Math.round(e.clientX), y: Math.round(e.clientY),
      });
    },
    true,
  );

  // input / change (value masked for secrets; Enter marks a submit)
  const onInput = (e, submit) => {
    const el = e.target;
    if (!el || !("value" in el)) return;
    const d = describe(el);
    const masked = isSecret(el);
    send({
      type: "input", t_ms: now(), target: d.target, label: d.label, role: d.role, chain: d.chain,
      value: masked ? "" : String(el.value || "").slice(0, 200), masked, submit: !!submit,
    });
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
