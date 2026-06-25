// Manual zoom regions — the editor's override of the auto-zoom. An immutable model: every
// edit produces a new array (→ trivial undo via history snapshots). Exported as a `.clipxd`
// project alongside the clip's index.

export type ZoomRegion = { id: string; start: number; end: number; scale: number };

let _seq = 0;
export function newRegion(t: number, dur: number, scale = 2.0): ZoomRegion {
  _seq += 1;
  return { id: `z${_seq}`, start: t, end: t + dur, scale };
}

export function regionAt(rs: ZoomRegion[], t: number): ZoomRegion | undefined {
  return rs.find((r) => t >= r.start && t <= r.end);
}

export type Project = { clipxd_project: "1"; clip: string; zoom_regions: ZoomRegion[] };

export function toProject(clip: string, regions: ZoomRegion[]): Project {
  return { clipxd_project: "1", clip, zoom_regions: [...regions].sort((a, b) => a.start - b.start) };
}

export function download(name: string, data: unknown) {
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  URL.revokeObjectURL(a.href);
}
