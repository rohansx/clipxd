import { useState } from "react";
import { setSubtitleStyle as apiSetSubtitleStyle } from "./api";
import type { EmphasisKind, Index, SubtitleDesign, SubtitlePosition, SubtitleStyle } from "./types";

// The styled-subtitle layer: a design picker + a live preview overlay. The *designs* are
// static presets; the *emphasis* that drives the Karaoke/Bold highlight comes from
// `index.subtitle_emphasis`, which the server's Ollama-Cloud pass produced AT INDEXING TIME
// (see crates/clipxd-web/src/emphasis.rs). So picking a design is only meaningful after
// indexing has run — matching the "after recording done, when indexing" requirement.

export const SUBTITLE_DESIGNS: { id: SubtitleDesign; label: string; blurb: string }[] = [
  { id: "classic", label: "Classic", blurb: "White captions, soft shadow" },
  { id: "bold", label: "Bold", blurb: "Heavier weight, focus words accented" },
  { id: "karaoke", label: "Karaoke", blurb: "Words light up as you speak them" },
  { id: "minimal", label: "Minimal", blurb: "Thin, low-contrast, no shadow" },
  { id: "boxed", label: "Boxed", blurb: "Semi-transparent bar behind the text" },
  { id: "glow", label: "Glow", blurb: "Neon glow drop-shadow" },
];

const POSITIONS: { id: SubtitlePosition; label: string }[] = [
  { id: "bottom", label: "Bottom" },
  { id: "center", label: "Center" },
  { id: "top", label: "Top" },
];

const DEFAULT_STYLE: SubtitleStyle = { design: "classic", font_scale: 1, position: "bottom", emphasis: true };

/** Find the transcript segment active at time `t` (the last one whose start ≤ t and not yet
 *  past its end). Returns null when no transcript exists or none is active. */
export function activeSegment(idx: Index, t: number): Index["transcript"][number] | null {
  if (!idx.transcript?.length) return null;
  let best: Index["transcript"][number] | null = null;
  for (const s of idx.transcript) {
    if (s.start <= t && (best === null || s.start > best.start) && (s.end >= t || s.end <= 0)) {
      // prefer a segment that currently contains t; fall back to the latest started
      if (s.start <= t && (s.end === 0 || t <= s.end)) return s;
      best = s;
    }
  }
  return best;
}

/** The emphasis segment matching `seg`, if the indexing-time LLM pass produced one. Matched by
 *  overlapping [start,end] (the LLM is told to keep the same segment count + order, but we don't
 *  trust that strictly — overlap is robust). */
function emphasisFor(idx: Index, seg: { start: number; end: number }) {
  const em = idx.subtitle_emphasis;
  if (!em?.segments?.length) return null;
  // exact-ish start match first
  let best = em.segments.find((s) => Math.abs(s.start - seg.start) < 0.4);
  if (!best) {
    // overlapping match
    best = em.segments
      .filter((s) => s.start <= seg.end && s.end >= seg.start)
      .sort((a, b) => Math.abs(a.start - seg.start) - Math.abs(b.start - seg.start))[0];
  }
  return best ?? null;
}

interface WordStyle {
  text: string;
  className: string;
  style: React.CSSProperties;
}

/** Split a segment into words, applying the design + emphasis. For Karaoke, `progress` (0..1
 *  of the segment elapsed) lights words up left-to-right. */
function styledWords(
  text: string,
  design: SubtitleDesign,
  emphasisOn: boolean,
  emWords: { text: string; emphasis: EmphasisKind }[] | null,
  progress: number,
): WordStyle[] {
  const raw = text.split(/\s+/).filter(Boolean);
  const total = raw.length || 1;
  return raw.map((w, i) => {
    const emWord = emWords?.find((e) => e.text.toLowerCase() === w.toLowerCase().replace(/[.,!?;:"']/g, ""));
    const emphasis = emphasisOn ? emWord?.emphasis ?? "none" : "none";
    const lit = design === "karaoke" && i / total <= progress; // spoken-so-far
    let cls = "sub-word";
    let style: React.CSSProperties = {};
    if (design === "bold") {
      if (emphasis === "primary") { style = { fontWeight: 800, color: "#ffd54a" }; cls += " sub-primary"; }
      else if (emphasis === "secondary") { style = { fontWeight: 700, color: "#fff" }; cls += " sub-secondary"; }
      else { style = { fontWeight: 600, color: "rgba(255,255,255,.92)" }; }
    } else if (design === "karaoke") {
      if (emphasis === "primary") style = { color: "#ffd54a", fontWeight: 800 };
      else if (lit) style = { color: "#fff", fontWeight: 700 };
      else style = { color: "rgba(255,255,255,.45)" };
    } else if (design === "glow") {
      if (emphasis === "primary") style = { color: "#7cf9ff", textShadow: "0 0 12px #7cf9ff, 0 0 22px #18a8ff" };
      else if (emphasis === "secondary") style = { color: "#fff", textShadow: "0 0 10px #fff" };
    } else if (design === "boxed" || design === "classic") {
      if (emphasis === "primary") style = { color: "#ffd54a", fontWeight: 700 };
      else if (emphasis === "secondary") style = { color: "#fff", fontWeight: 600 };
    } else if (design === "minimal") {
      if (emphasis === "primary") style = { color: "#000", fontWeight: 700 };
      else style = { color: "rgba(0,0,0,.72)" };
    }
    return { text: w, className: cls, style };
  });
}

const WRAP_STYLE: Record<SubtitleDesign, React.CSSProperties> = {
  classic: { color: "#fff", textShadow: "0 2px 6px rgba(0,0,0,.9)" },
  bold: { color: "rgba(255,255,255,.92)", textShadow: "0 2px 8px rgba(0,0,0,.95)" },
  karaoke: { textShadow: "0 2px 8px rgba(0,0,0,.95)" },
  minimal: { color: "rgba(20,20,20,.78)", textShadow: "0 1px 2px rgba(255,255,255,.5)" },
  boxed: { color: "#fff", background: "rgba(0,0,0,.62)", padding: "8px 16px", borderRadius: 10, textShadow: "0 1px 3px rgba(0,0,0,.8)" },
  glow: { color: "#fff", textShadow: "0 0 10px rgba(124,249,255,.9), 0 0 20px rgba(24,168,255,.6)" },
};

/** The live subtitle overlay rendered on top of the video. Honors the user's chosen
 *  `subtitle_style` and the indexing-time `subtitle_emphasis`. No-op (renders nothing) when
 *  no transcript exists or no style has been chosen yet. */
export function SubtitleLayer({ index, t }: { index: Index; t: number }) {
  const style = index.subtitle_style;
  if (!style) return null; // nothing chosen yet — the picker below enables this
  const seg = activeSegment(index, t);
  if (!seg) return null;
  const em = emphasisFor(index, seg);
  const progress = seg.end > seg.start ? Math.max(0, Math.min(1, (t - seg.start) / (seg.end - seg.start))) : 1;
  const words = styledWords(seg.text, style.design, style.emphasis, em?.words ?? null, progress);
  const posCss: React.CSSProperties =
    style.position === "top" ? { top: "8%" }
    : style.position === "center" ? { top: "50%", transform: "translateY(-50%)" }
    // 12% alone lands inside the ~53px player bar on any video shorter than ~442px, putting the
    // subtitle on top of the seek track. Floor it just above the bar.
    : { bottom: "max(12%, 64px)" };
  return (
    <div
      className={"subtitle-layer sub-" + style.design}
      style={{ ...posCss, fontSize: `${style.font_scale * 1.0}em`, ...WRAP_STYLE[style.design] }}
    >
      {words.map((w, i) => (
        <span key={i} className={w.className} style={w.style}>{w.text} </span>
      ))}
    </div>
  );
}

/** The design-selection bar — chips for each design, font-scale + position + emphasis toggles,
 *  and a Save button that persists the choice to the clip's index. Shown on the clip page once
 *  a transcript exists (the designs need speech to act on). */
export function SubtitleStyleBar({
  id,
  index,
  showToast,
}: {
  id: string;
  index: Index;
  showToast: (m: string) => void;
}) {
  const initial = index.subtitle_style ?? DEFAULT_STYLE;
  const [style, setStyle] = useState<SubtitleStyle>(initial);
  const [saving, setSaving] = useState(false);
  const hasEmphasis = !!index.subtitle_emphasis?.segments?.length;

  const save = async () => {
    setSaving(true);
    try {
      await apiSetSubtitleStyle(id, style);
      showToast("Subtitle style saved");
    } catch (e) {
      showToast(e instanceof Error ? e.message : "couldn't save subtitle style");
    } finally {
      setSaving(false);
    }
  };

  return (
    // Collapsed by default: styling subtitles is authoring, and most clip-page visits are
    // someone watching. Native <details> — no state, no JS. Wrapping (rather than converting
    // .subtitle-style-bar itself) leaves that element's flex layout untouched.
    <details className="ssb-details">
      <summary className="ssb-summary">Edit subtitles</summary>
    <div className="subtitle-style-bar">
      <div className="ssb-head">
        <span className="ssb-note">
          {hasEmphasis
            ? "highlighted keywords are ready ✓"
            : "keyword highlighting isn’t ready yet — re-index this clip to add it"}
        </span>
      </div>
      <div className="ssb-designs">
        {SUBTITLE_DESIGNS.map((d) => (
          <button
            key={d.id}
            className={"ssb-chip" + (style.design === d.id ? " on" : "")}
            onClick={() => setStyle((s) => ({ ...s, design: d.id }))}
            title={d.blurb}
          >
            {d.label}
          </button>
        ))}
      </div>
      <div className="ssb-controls">
        <label className="cam-slider"> size
          <input type="range" min={80} max={160} value={Math.round(style.font_scale * 100)} onChange={(e) => setStyle((s) => ({ ...s, font_scale: +e.target.value / 100 }))} />
        </label>
        <div className="ssb-positions">
          {POSITIONS.map((p) => (
            <button key={p.id} className={style.position === p.id ? "on" : ""} onClick={() => setStyle((s) => ({ ...s, position: p.id }))}>{p.label}</button>
          ))}
        </div>
        <label className="ssb-toggle">
          <input type="checkbox" checked={style.emphasis} onChange={(e) => setStyle((s) => ({ ...s, emphasis: e.target.checked }))} />
          highlight keywords
        </label>
      </div>
      {/* live preview against the first transcript segment */}
      <div className="ssb-preview">
        <SubtitleLayerForPreview index={index} style={style} />
      </div>
      <button className="btn-signal btn-pill" onClick={save} disabled={saving} style={{ alignSelf: "flex-start", padding: "8px 18px" }}>
        {saving ? <span className="spin" /> : "Save style"}
      </button>
    </div>
    </details>
  );
}

/** A preview-only variant that renders a chosen (possibly unsaved) style over the first
 *  transcript segment, regardless of whether index.subtitle_style is set yet. */
function SubtitleLayerForPreview({ index, style }: { index: Index; style: SubtitleStyle }) {
  const seg = index.transcript?.find((s) => s.text.trim());
  if (!seg) return <div className="ssb-preview-empty">No transcript yet — captions need speech.</div>;
  const em = emphasisFor(index, seg);
  const words = styledWords(seg.text, style.design, style.emphasis, em?.words ?? null, 1);
  return (
    <div className={"subtitle-layer sub-" + style.design} style={{ position: "static", fontSize: `${style.font_scale}em`, ...WRAP_STYLE[style.design] }}>
      {words.map((w, i) => (
        <span key={i} className={w.className} style={w.style}>{w.text} </span>
      ))}
    </div>
  );
}