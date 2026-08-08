// Manual zoom regions — the editor's override of the auto-zoom. An immutable model: every
// edit produces a new array (→ trivial undo via history snapshots). Exported as a `.clipxd`
// project alongside the clip's index.

export type Seg = { id: string; start: number; end: number };
/** `cx`/`cy` are the focus point in normalised source-frame coordinates (0..1, top-left origin) —
 *  the same pair `ZoomKeyframe` carries. Additive: the renderer reads start/end/scale by name, so
 *  a project written by this editor still loads on a server that ignores the focus. */
export type ZoomRegion = Seg & { scale: number; cx: number; cy: number };
export type EditKind = "trim" | "speed";
export type EditRegion = Seg & { kind: EditKind; rate: number };

let _seq = 0;
export function newRegion(t: number, dur: number, scale = 2.0): ZoomRegion {
  _seq += 1;
  // Centre focus by default: that is exactly what the renderer assumed back when regions had no
  // focus at all, so a region the user never clicks renders the same as it always did.
  return { id: `z${_seq}`, start: t, end: t + dur, scale, cx: 0.5, cy: 0.5 };
}

export type Rect = { x: number; y: number; w: number; h: number };

/** The normalised slice of the source frame the render keeps at this zoom.
 *
 *  Mirrors `crop_rect()` in crates/clipxd-cinematic/src/render.rs: a 1/scale window centred on
 *  the focus point and then *clamped* inside the frame, so a focus near an edge slides the
 *  window rather than sampling off-frame. Shared by the editor's focus overlay and the live
 *  video transform so both describe the rectangle the server will actually crop.
 *
 *  Geometry only. It says nothing about how the render resamples, composites the wallpaper or
 *  rasterises captions — the browser cannot and does not claim to match those. (Sub-pixel drift
 *  vs. the renderer is expected: render.rs rounds the crop to even pixel dimensions.) */
export function cropRect(z: { scale: number; cx: number; cy: number }): Rect {
  const w = 1 / Math.max(1, z.scale);
  const fit = (c: number) => Math.min(Math.max(c - w / 2, 0), 1 - w);
  return { x: fit(z.cx), y: fit(z.cy), w, h: w };
}

export function newEdit(kind: EditKind, t: number, dur: number): EditRegion {
  _seq += 1;
  return { id: `e${_seq}`, kind, start: t, end: t + dur, rate: kind === "speed" ? 2 : 1 };
}

export function regionAt(rs: ZoomRegion[], t: number): ZoomRegion | undefined {
  return rs.find((r) => t >= r.start && t <= r.end);
}

export function editAt(es: EditRegion[], t: number, kind: EditKind): EditRegion | undefined {
  return es.find((e) => e.kind === kind && t >= e.start && t <= e.end);
}

export type Project = { clipxd_project: "1"; clip: string; zoom_regions: ZoomRegion[]; edit_regions: EditRegion[] };

export function toProject(clip: string, zoom: ZoomRegion[], edits: EditRegion[]): Project {
  const bystart = (a: Seg, b: Seg) => a.start - b.start;
  return { clipxd_project: "1", clip, zoom_regions: [...zoom].sort(bystart), edit_regions: [...edits].sort(bystart) };
}

export function download(name: string, data: unknown) {
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  URL.revokeObjectURL(a.href);
}
