// Runnable check for RegionTrack's selection: a DOM event-ordering property no type-checker can
// see, and one that shipped broken. It mounts the REAL <RegionTrack> and clicks it.
//
//   cd app && d=$(mktemp -d) &&
//     npx esbuild src/RegionTrack.select.check.tsx --bundle --jsx=automatic \
//       --define:process.env.NODE_ENV='"production"' --outfile="$d/sel.js" &&
//     printf '<!doctype html><body><script src="sel.js"></script>' > "$d/sel.html" &&
//     "${CHROME:-/usr/bin/google-chrome}" --headless=new --no-sandbox --disable-gpu \
//       --dump-dom "file://$d/sel.html" | tail -1 | tee /dev/stderr |
//       grep -qx '<html>RESULT ok</html>'
//
// The predecessor of this file was an .html page that re-implemented the lane's click handler
// inline. It could not fail on the bug it documented: deleting the guard from RegionTrack.tsx
// left it green. Hence the bundle — the only thing this file supplies is state and clicks; the
// handler under test is imported.
//
// Two traps in the gate line, both already paid for. `| grep RESULT` matches "RESULT fail: …"
// and exits 0 — it gated nothing. And `grep -q` for the pass token matched the token inside the
// page's own script source back when the script was inline. So the last act here is to replace
// the document with a one-line verdict, and the gate is a whole-line (-x) match on it: a page
// that threw before reaching that line leaves markup whose last line never matches, so a check
// that could not run reads as a failure.
//
// Inert to the app: nothing imports it, so Vite never bundles it, while `tsc --noEmit` (include:
// ["src"]) still type-checks it.
import { useState } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";
import { RegionTrack } from "./RegionTrack";
import type { ZoomRegion } from "./regions";

// A dispatched PointerEvent has no *active* pointer behind it, so the component's
// setPointerCapture(e.pointerId) throws NotFoundError in a real browser and the handler dies
// before it selects. Capture is drag plumbing, not the property under test — stub that one API
// and nothing else.
Element.prototype.setPointerCapture = () => {};

// State lives in the parent in the shipped app (ClipPage), so it lives in the parent here too.
function Harness() {
  const [selected, setSelected] = useState<string | null>(null);
  const [regions, setRegions] = useState<ZoomRegion[]>([
    { id: "z1", start: 1, end: 3, scale: 2, cx: 0.5, cy: 0.5 },
  ]);
  return (
    <RegionTrack
      regions={regions}
      duration={10}
      selected={selected}
      laneLabel="zoom"
      onSelect={setSelected}
      onDragStart={() => {}}
      onChange={setRegions}
      renderLabel={(r) => r.id}
    />
  );
}

const init = { bubbles: true, cancelable: true, pointerId: 1, isPrimary: true };
// A block's onPointerDown selects and calls stopPropagation() — but the *click* that follows is a
// separate event and still bubbles to the lane. Both must be dispatched or the bug is invisible.
const clickOn = (el: Element) => {
  el.dispatchEvent(new PointerEvent("pointerdown", init));
  el.dispatchEvent(new PointerEvent("pointerup", init));
  el.dispatchEvent(new MouseEvent("click", init));
};

// React 18 does not repaint inside the dispatch: an update from a discrete event is flushed from
// a MICROTASK queued during dispatch, so `.sel` is not in the DOM on the line after
// dispatchEvent(). Awaiting an already-resolved promise queues this function's continuation
// *behind* React's, which is deterministic — unlike a setTimeout, which can lose a race with
// Chrome's load-time --dump-dom. Both settle long before the load event.
const settled = () => Promise.resolve();

const fails: string[] = [];
(async () => {
  try {
    const host = document.body.appendChild(document.createElement("div"));
    // Concurrent root: flushSync makes the mount observable on the next line.
    flushSync(() => createRoot(host).render(<Harness />));

    const lane = host.querySelector(".regiontrack");
    const block = host.querySelector(".region");
    if (!lane || !block) throw new Error("RegionTrack rendered no lane and/or no region block");

    // Selection must survive the click that made it. Without the `e.target === e.currentTarget`
    // guard on the lane, the bubbled click clears it again: selection lasted one frame, Delete
    // stayed disabled, and the zoom inspector never appeared.
    clickOn(block);
    await settled();
    if (!host.querySelector(".region.sel")) fails.push("clicking a region does NOT keep it selected");

    // …and the lane background must still deselect, i.e. the guard did not just delete the handler.
    clickOn(lane);
    await settled();
    if (host.querySelector(".region.sel")) fails.push("clicking the lane background no longer deselects");
  } catch (e) {
    fails.push(String(e));
  }
  document.documentElement.textContent = fails.length ? "RESULT fail: " + fails.join("; ") : "RESULT ok";
})();
