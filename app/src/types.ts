// Mirrors the Rust `clipxd-index` schema (crates/clipxd-index/src/schema.rs) so the UI can
// render a clip straight from `GET /clip/:id/index.json`. Every key matches the JSON verbatim;
// optional fields use `skip_serializing_if` on the Rust side, so treat them as possibly-absent.

export type ClipSource = "screen" | "browser" | "import";
export type ClipStatus = "complete" | "partial" | "enriching" | "recording";
export type TextKind = "ocr" | "dom";

export interface Metadata {
  duration: number;
  resolution: [number, number];
  fps: number;
  created_at: string;
  title: string;
  app_focus?: { start: number; end: number; app: string; window: string }[];
  url_context?: string;
  has_video: boolean;
}

export interface TranscriptSegment {
  start: number;
  end: number;
  speaker?: string;
  text: string;
}

export interface VisualMoment {
  t: number;
  salience: number;
  caption: string;
  delta: string;
  frame_ref?: string;
}

export interface OnScreenText {
  start: number;
  end: number;
  text: string;
  source: TextKind;
  bbox?: [number, number, number, number];
}

export interface ClipEvent {
  t: number;
  kind: string;
  text?: string;
  data?: Record<string, unknown>;
}

export interface Chapter {
  start: number;
  title: string;
}

export interface Comment {
  id: string;
  t: number;
  author: string;
  text: string;
  created_at: number;
}

export interface Summary {
  tldr: string;
  chapters?: Chapter[];
}

export interface Redaction {
  ran: boolean;
  engine?: string;
  items?: { stream: string; t: number; entity: string; action: string }[];
  policy: string;
}

export interface Index {
  clipxd_version: string;
  id: string;
  source: ClipSource;
  status: ClipStatus;
  metadata: Metadata;
  transcript: TranscriptSegment[];
  visual_timeline: VisualMoment[];
  on_screen_text: OnScreenText[];
  event_track: ClipEvent[];
  summary: Summary;
  redaction: Redaction;
}

export interface ClipCounts {
  events: number;
  on_screen_text: number;
  transcript: number;
  visual: number;
}

export interface ClipSummary {
  id: string;
  source: ClipSource;
  status?: ClipStatus;
  metadata: Metadata;
  counts: ClipCounts;
}

export interface ZoomKeyframe {
  t: number;
  scale: number;
  cx: number;
  cy: number;
}

export interface TextHit {
  t: number;
  text: string;
  stream: "transcript" | "on_screen_text" | "caption";
  score: number;
}

export interface QueryAnswer {
  text: string;
  citations: number[];
}

export const fmt = (t: number): string =>
  `${Math.floor(t / 60)}:${String(Math.floor(t % 60)).padStart(2, "0")}`;
