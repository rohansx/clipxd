// Manual zoom regions — the editor's override of the auto-zoom. An immutable model: every
// edit produces a new array (→ trivial undo via history snapshots). Exported as a `.clipxd`
// project alongside the clip's index.

export type Seg = { id: string; start: number; end: number };
export type ZoomRegion = Seg & { scale: number };
export type EditKind = "trim" | "speed";
export type EditRegion = Seg & { kind: EditKind; rate: number };

let _seq = 0;
export function newRegion(t: number, dur: number, scale = 2.0): ZoomRegion {
  _seq += 1;
  return { id: `z${_seq}`, start: t, end: t + dur, scale };
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
