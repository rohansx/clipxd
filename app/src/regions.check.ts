// Runnable check for the zoom-region geometry the editor and the renderer must agree on.
//   node app/src/regions.check.ts && npm run build
// (Node ≥22.18 strips the types; no test runner, no dependency.) It is not imported by the app,
// so Vite never bundles it, but `tsc --noEmit` still type-checks it.
//
// It EXITS NON-ZERO on failure. It used to end at the console.log below, so a deliberately broken
// cropRect() printed "4 failure(s)" and the `&& npm run build` after it still built and shipped.
//
// The cases mirror crates/clipxd-cinematic/src/render.rs's own tests for crop_rect() — if that
// clamp ever changes, the browser overlay would start lying about where the render will zoom,
// and this is the thing that fails first.
import { cropRect, newRegion, toProject } from "./regions.ts";

let bad = 0;
const eq = (name: string, got: number, want: number) => {
  if (Math.abs(got - want) > 1e-9) {
    bad += 1;
    console.error(`FAIL ${name}: got ${got}, want ${want}`);
  }
};

// 1× is the whole frame, wherever the focus is.
const full = cropRect({ scale: 1, cx: 0.2, cy: 0.9 });
eq("1x w", full.w, 1);
eq("1x x", full.x, 0);
eq("1x y", full.y, 0);

// 2× centred: half the frame, centred.
const mid = cropRect({ scale: 2, cx: 0.5, cy: 0.5 });
eq("2x w", mid.w, 0.5);
eq("2x x", mid.x, 0.25);
eq("2x y", mid.y, 0.25);

// 2× aimed at a corner: the window slides to the edge instead of hanging off it (the render
// cannot sample outside the source frame, so the editor must not draw a rect that does).
const corner = cropRect({ scale: 2, cx: 0.97, cy: 0.03 });
eq("corner x", corner.x, 0.5);
eq("corner y", corner.y, 0);

// Off-centre but in bounds: the window is centred on the focus point.
const off = cropRect({ scale: 4, cx: 0.6, cy: 0.4 });
eq("off x", off.x, 0.6 - 0.125);
eq("off y", off.y, 0.4 - 0.125);

// Below 1× is meaningless (the renderer does kf.scale.max(1.0)) — never invert the rect.
eq("sub-1x w", cropRect({ scale: 0.4, cx: 0.5, cy: 0.5 }).w, 1);

// A fresh region is centre-focused, i.e. byte-identical to what the pre-focus editor produced.
const fresh = newRegion(2, 1.5);
eq("new cx", fresh.cx, 0.5);
eq("new cy", fresh.cy, 0.5);

// The exported project keeps the server's field names (start/end/scale read positionally by
// name in beautify.rs) and carries the focus alongside them. beautify.rs reads `z["cx"]`/`z["cy"]`
// as normalised 0..1 floats, so the values must survive toProject unrenamed and unscaled — key
// presence alone would still pass if someone exported the focus in pixels.
const focused = { ...newRegion(1, 1), cx: 0.8, cy: 0.2 };
const proj = toProject("clip1", [focused], []);
const z = proj.zoom_regions[0];
for (const k of ["start", "end", "scale", "cx", "cy"]) {
  if (!(k in z)) {
    bad += 1;
    console.error(`FAIL project zoom_regions missing ${k}`);
  }
}
eq("project cx", z.cx, 0.8);
eq("project cy", z.cy, 0.2);

console.log(bad ? `regions.check: ${bad} failure(s)` : "regions.check: ok");
// No @types/node in this browser app, so name the one global the exit code needs.
declare const process: { exitCode: number };
process.exitCode = bad ? 1 : 0;
