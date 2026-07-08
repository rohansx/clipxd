// Camera capture settings shared between the recording preview and the recorder hook, so
// what the presenter sees (CSS filter on the <video>) is exactly what gets baked into the
// recorded canvas (ctx.filter before drawImage) — true WYSIWYG. Pure presentation; never
// changes the index or the agent surface.

export interface CameraFilter {
  brightness: number; // 0.5 .. 1.5 (1 = unchanged)
  contrast: number; // 0.5 .. 1.5
  saturate: number; // 0 .. 2
  grayscale: number; // 0 .. 1
  sepia: number; // 0 .. 1
  hue: number; // -180 .. 180 (degrees)
}

export type CameraBgKind = "none" | "blur" | "solid" | "gradient" | "preset" | "image";

export interface CameraBackground {
  kind: CameraBgKind;
  /** For `blur`: how strongly to blur the camera frame drawn behind the sharp inset (0..30). */
  blur: number;
  /** For `solid` / `gradient`: the fill color(s). */
  color: string;
  color2: string;
  /** For `solid`/`gradient`/`preset`/`image`: how much of the bubble the inset (sharp camera)
   *  covers (0.5..0.95). The background fills the rest as a clean ring/halo. */
  inset: number;
  /** For `preset`: which scene preset (see CAMERA_BG_PRESETS). */
  presetId?: string;
  /** For `image`: a custom uploaded background (object URL or data URL). Not persisted across
   *  reloads unless it's a data URL — object URLs are session-only. */
  imageSrc?: string;
}

export interface CameraConfig {
  filter: CameraFilter;
  background: CameraBackground;
}

export const DEFAULT_CAMERA_CONFIG: CameraConfig = {
  filter: { brightness: 1, contrast: 1, saturate: 1, grayscale: 0, sepia: 0, hue: 0 },
  background: { kind: "none", blur: 8, color: "#0d1117", color2: "#1f6feb", inset: 0.82 },
};

// Preset "live filter" looks — one click applies a curated filter, matching how Cap/Screen
// Studio surface a handful of finishes rather than raw sliders only.
export const CAMERA_PRESETS: { name: string; filter: CameraFilter }[] = [
  { name: "Natural", filter: { brightness: 1, contrast: 1, saturate: 1, grayscale: 0, sepia: 0, hue: 0 } },
  { name: "Bright", filter: { brightness: 1.12, contrast: 1.05, saturate: 1.1, grayscale: 0, sepia: 0, hue: 0 } },
  { name: "Warm", filter: { brightness: 1.05, contrast: 1.02, saturate: 1.15, grayscale: 0, sepia: 0.18, hue: 0 } },
  { name: "Cool", filter: { brightness: 1.02, contrast: 1.05, saturate: 1.05, grayscale: 0, sepia: 0, hue: -12 } },
  { name: "Mono", filter: { brightness: 1.05, contrast: 1.1, saturate: 0, grayscale: 1, sepia: 0, hue: 0 } },
  { name: "Film", filter: { brightness: 0.98, contrast: 1.12, saturate: 0.85, grayscale: 0, sepia: 0.12, hue: 0 } },
];

// Google-Meet-style background SCENES: a curated set of clean, professional gradient presets
// (the honest, no-ML-licensing equivalent of Meet's abstract backgrounds) plus custom image
// upload for a real photo. Only the chosen `id` is persisted to localStorage; the draw code
// lives here. `css` is the preview background; `draw` paints the same scene onto the recorded
// canvas inside the camera-bubble clip. They mirror the render wallpapers (aurora/dusk/ocean/
// violet/noir) so a camera bubble can match the video frame behind it.
export interface BgPreset {
  id: string;
  label: string;
  css: string;
  draw: (ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number) => void;
}

function lgrad(angle: number, stops: [number, string][]): string {
  return `linear-gradient(${angle}deg, ${stops.map(([o, c]) => `${c} ${o * 100}%`).join(", ")})`;
}

// radial blobs composited from the same colours the `draw` paints — gives a soft, premium
// scene (aurora/dusk/ocean/violet/noir/mint) rather than a flat two-stop gradient.
export const CAMERA_BG_PRESETS: BgPreset[] = [
  {
    id: "aurora",
    label: "Aurora",
    css: "radial-gradient(at 18% 18%, #2a55c0 0%, transparent 55%), radial-gradient(at 82% 22%, #6a4ad0 0%, transparent 50%), radial-gradient(at 75% 88%, #1f9c8c 0%, transparent 55%), radial-gradient(at 12% 92%, #d0506a 0%, transparent 52%), #0a0f1c",
    draw: (ctx, x, y, w, h) => drawBlobs(ctx, x, y, w, h, "#0a0f1c", [
      [0.18, 0.18, 0.7, "#2a55c0"], [0.82, 0.22, 0.66, "#6a4ad0"], [0.75, 0.88, 0.72, "#1f9c8c"], [0.12, 0.92, 0.66, "#d0506a"],
    ]),
  },
  {
    id: "dusk",
    label: "Dusk",
    css: "radial-gradient(at 15% 12%, #5a46c8 0%, transparent 55%), radial-gradient(at 88% 18%, #cd5096 0%, transparent 52%), radial-gradient(at 70% 92%, #3c5ab9 0%, transparent 55%), #14102a",
    draw: (ctx, x, y, w, h) => drawBlobs(ctx, x, y, w, h, "#14102a", [
      [0.15, 0.12, 0.75, "#5a46c8"], [0.88, 0.18, 0.65, "#cd5096"], [0.70, 0.92, 0.7, "#3c5ab9"],
    ]),
  },
  {
    id: "ocean",
    label: "Ocean",
    css: "radial-gradient(at 18% 15%, #2878cd 0%, transparent 55%), radial-gradient(at 86% 90%, #1eb9af 0%, transparent 55%), radial-gradient(at 60% 25%, #505adc 0%, transparent 50%), #07121e",
    draw: (ctx, x, y, w, h) => drawBlobs(ctx, x, y, w, h, "#07121e", [
      [0.18, 0.15, 0.75, "#2878cd"], [0.86, 0.90, 0.7, "#1eb9af"], [0.60, 0.25, 0.55, "#505adc"],
    ]),
  },
  {
    id: "violet",
    label: "Violet",
    css: "radial-gradient(at 20% 20%, #8250eb 0%, transparent 58%), radial-gradient(at 85% 15%, #d25ac8 0%, transparent 52%), radial-gradient(at 50% 95%, #5a46d7 0%, transparent 55%), #100a1c",
    draw: (ctx, x, y, w, h) => drawBlobs(ctx, x, y, w, h, "#100a1c", [
      [0.20, 0.20, 0.75, "#8250eb"], [0.85, 0.15, 0.65, "#d25ac8"], [0.50, 0.95, 0.7, "#5a46d7"],
    ]),
  },
  {
    id: "noir",
    label: "Noir",
    css: "radial-gradient(at 20% 12%, #2a2e3c 0%, transparent 60%), radial-gradient(at 85% 95%, #161620 0%, transparent 60%), #0a0c10",
    draw: (ctx, x, y, w, h) => drawBlobs(ctx, x, y, w, h, "#0a0c10", [
      [0.20, 0.12, 0.7, "#2a2e3c"], [0.85, 0.95, 0.7, "#161620"],
    ]),
  },
  {
    id: "mint",
    label: "Mint",
    css: "radial-gradient(at 22% 20%, #2bbf9a 0%, transparent 58%), radial-gradient(at 80% 82%, #cdecc0 0%, transparent 55%), radial-gradient(at 70% 18%, #8ad7d0 0%, transparent 50%), #06201c",
    draw: (ctx, x, y, w, h) => drawBlobs(ctx, x, y, w, h, "#06201c", [
      [0.22, 0.20, 0.72, "#2bbf9a"], [0.80, 0.82, 0.66, "#cdecc0"], [0.70, 0.18, 0.55, "#8ad7d0"],
    ]),
  },
  {
    id: "gradient-warm",
    label: "Warm",
    css: lgrad(135, [[0, "#ff9966"], [1, "#ff5e62"]]),
    draw: (ctx, x, y, w, h) => drawLinear(ctx, x, y, w, h, 135, ["#ff9966", "#ff5e62"]),
  },
  {
    id: "gradient-cool",
    label: "Cool",
    css: lgrad(135, [[0, "#36d1dc"], [1, "#5b86e5"]]),
    draw: (ctx, x, y, w, h) => drawLinear(ctx, x, y, w, h, 135, ["#36d1dc", "#5b86e5"]),
  },
];

export function bgPresetById(id?: string): BgPreset | undefined {
  return CAMERA_BG_PRESETS.find((p) => p.id === id);
}

/** Composite N radial colour blobs over a base — the same mesh idea the render wallpapers use,
 *  painted inside the bubble's circle clip by the caller. */
function drawBlobs(
  ctx: CanvasRenderingContext2D,
  x: number, y: number, w: number, h: number,
  base: string,
  blobs: [number, number, number, string][],
) {
  ctx.fillStyle = base;
  ctx.fillRect(x, y, w, h);
  for (const [fx, fy, rad, c] of blobs) {
    const cx = x + fx * w;
    const cy = y + fy * h;
    const r = rad * Math.max(w, h);
    const g = ctx.createRadialGradient(cx, cy, 0, cx, cy, r);
    g.addColorStop(0, c);
    g.addColorStop(1, "transparent");
    ctx.fillStyle = g;
    ctx.fillRect(x, y, w, h);
  }
}

function drawLinear(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, angle: number, stops: string[]) {
  const rad = ((angle - 90) * Math.PI) / 180;
  const cx = x + w / 2, cy = y + h / 2;
  const len = Math.abs(w * Math.cos(rad)) + Math.abs(h * Math.sin(rad));
  const dx = (Math.cos(rad) * len) / 2, dy = (Math.sin(rad) * len) / 2;
  const g = ctx.createLinearGradient(cx - dx, cy - dy, cx + dx, cy + dy);
  stops.forEach((c, i) => g.addColorStop(i / (stops.length - 1), c));
  ctx.fillStyle = g;
  ctx.fillRect(x, y, w, h);
}

/** The CSS `background` string for the camera bubble's preview, per the chosen background. */
export function previewBackgroundCss(bg: CameraBackground): string {
  switch (bg.kind) {
    case "solid":
      return bg.color;
    case "gradient":
      return lgrad(135, [[0, bg.color], [1, bg.color2]]);
    case "preset":
      return bgPresetById(bg.presetId)?.css ?? "#0d1117";
    case "image":
      return bg.imageSrc ? `url(${bg.imageSrc}) center/cover` : "#0d1117";
    default:
      return "#0d1117";
  }
}

/** The CSS `filter` string for a CameraFilter — works for both the <video> preview and the
 *  canvas `ctx.filter` (Chromium/Firefox support `ctx.filter`). */
export function filterCss(f: CameraFilter): string {
  return [
    `brightness(${f.brightness})`,
    `contrast(${f.contrast})`,
    `saturate(${f.saturate})`,
    `grayscale(${f.grayscale})`,
    `sepia(${f.sepia})`,
    `hue-rotate(${f.hue}deg)`,
  ].join(" ");
}

// Per-session image cache for custom uploaded backgrounds: object URLs / data URLs → an
// already-decoded HTMLImageElement the recorder can draw without re-decoding each frame.
const imageCache = new Map<string, HTMLImageElement>();
export function loadImageBg(src: string): Promise<HTMLImageElement> {
  const cached = imageCache.get(src);
  if (cached && cached.complete) return Promise.resolve(cached);
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => { imageCache.set(src, img); resolve(img); };
    img.onerror = () => reject(new Error("background image failed to load"));
    img.src = src;
  });
}

const STORAGE_KEY = "clipxd:camera-config";

export function loadCameraConfig(): CameraConfig {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      return {
        filter: { ...DEFAULT_CAMERA_CONFIG.filter, ...parsed?.filter },
        background: { ...DEFAULT_CAMERA_CONFIG.background, ...parsed?.background },
      };
    }
  } catch {}
  return { ...DEFAULT_CAMERA_CONFIG };
}

export function saveCameraConfig(c: CameraConfig) {
  try {
    // Don't persist a session-only object URL as the image src — it'd be a dead link on
    // reload. Drop it before saving so a reload cleanly falls back to "none".
    const toSave: CameraConfig = {
      ...c,
      background: {
        ...c.background,
        imageSrc: c.background.imageSrc && c.background.imageSrc.startsWith("data:") ? c.background.imageSrc : undefined,
      },
    };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(toSave));
  } catch {}
}