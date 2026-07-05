// Main-world hook: content scripts run in an isolated world and can't see the page's real
// console or catch its uncaught errors, so this small shim runs IN the page. It wraps the
// console methods and listens for window errors, forwarding each to the isolated content
// script via window.postMessage (the only channel across the world boundary).
(function () {
  const post = (payload) => {
    try {
      window.postMessage({ __clipxd_cap: true, ...payload }, "*");
    } catch (e) {
      /* postMessage can throw on some cross-origin edge cases — never let it break the page */
    }
  };

  ["log", "info", "warn", "error"].forEach((level) => {
    const orig = console[level];
    console[level] = function (...args) {
      post({ kind: "console", level, text: args.map(stringify).join(" ").slice(0, 2000), uncaught: false });
      return orig.apply(this, args);
    };
  });

  window.addEventListener("error", (e) => {
    post({ kind: "console", level: "error", text: String(e.message || "error").slice(0, 2000), uncaught: true });
  });
  window.addEventListener("unhandledrejection", (e) => {
    post({ kind: "console", level: "error", text: ("unhandled rejection: " + String((e.reason && e.reason.message) || e.reason || "")).slice(0, 2000), uncaught: true });
  });

  function stringify(a) {
    if (typeof a === "string") return a;
    try {
      return JSON.stringify(a);
    } catch (e) {
      return String(a);
    }
  }
})();
