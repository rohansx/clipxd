import { useEffect, useRef, useState } from "react";

const SAMPLE = `Paste your script here — clipxd scrolls it while you record, so you can read naturally and still look at your camera.

Hit "Start scroll" and adjust the speed to match your pace. Drag the opacity slider down until the text is just legible to you.

This overlay lives on the clipxd tab only. It is never drawn into the recorded canvas — so it won't appear in your recording unless you deliberately capture this tab itself.`;

export interface PrompterConfig {
  text: string;
  opacity: number; // 0.15 .. 0.9 — how visible the overlay is to the presenter
  fontSize: number; // px
  speed: number; // px/s scroll
  mirror: boolean; // mirror text (presenter-facing glass prompter)
}

export const DEFAULT_PROMPTER: PrompterConfig = {
  text: SAMPLE,
  opacity: 0.55,
  fontSize: 34,
  speed: 40,
  mirror: false,
};

const STORAGE_KEY = "clipxd:prompter";

export function loadPrompterConfig(): PrompterConfig {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) return { ...DEFAULT_PROMPTER, ...JSON.parse(raw) };
  } catch {}
  return { ...DEFAULT_PROMPTER };
}

function savePrompterConfig(c: PrompterConfig) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(c));
  } catch {}
}

// A floating, semi-transparent teleprompter the presenter reads while recording. It is a DOM
// element on the clipxd tab only — the recorder's canvas draws screen + the camera bubble, NOT
// this overlay — so it does not appear in the recording unless the user is capturing this tab.
// Eye-line placement (just below the camera, top-center) so the presenter's gaze stays near
// the lens. Opacity / size / speed / mirror are all adjustable and persisted.
export function Prompter({ onClose }: { onClose: () => void }) {
  const [cfg, setCfg] = useState<PrompterConfig>(loadPrompterConfig);
  const [scrolling, setScrolling] = useState(false);
  const [editing, setEditing] = useState(false);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => savePrompterConfig(cfg), [cfg]);

  useEffect(() => {
    if (!scrolling) return;
    let raf = 0;
    let last = 0;
    const tick = (ts: number) => {
      const box = boxRef.current;
      if (box && last) {
        box.scrollTop += (cfg.speed * (ts - last)) / 1000;
        if (box.scrollTop + box.clientHeight >= box.scrollHeight - 1) setScrolling(false);
      }
      last = ts;
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [scrolling, cfg.speed]);

  const toggleScroll = () => {
    if (scrolling) {
      setScrolling(false);
      return;
    }
    if (boxRef.current) boxRef.current.scrollTop = 0;
    setEditing(false);
    setScrolling(true);
  };

  const readingLineTop = "42%";

  return (
    <div
      className="prompter prompter-glass"
      style={{
        // opacity controls only the script surface — the control bar stays solid so it's usable.
        // The reading-line highlight sits at eye level.
      }}
    >
      <div className="prompter-bar">
        <b>Teleprompter</b>
        <button onClick={toggleScroll}>{scrolling ? "⏸ Pause" : "▶ Start scroll"}</button>
        <button onClick={() => { setScrolling(false); setEditing((e) => !e); }} style={editing ? { borderColor: "var(--signal)" } : undefined}>
          {editing ? "▶ Preview" : "✎ Edit"}
        </button>
        <label className="pspeed" title="How visible the script is to you (not recorded)">
          opacity
          <input
            type="range"
            min={15}
            max={90}
            value={Math.round(cfg.opacity * 100)}
            onChange={(e) => setCfg((c) => ({ ...c, opacity: +e.target.value / 100 }))}
          />
        </label>
        <label className="pspeed" title="Font size">
          size
          <input
            type="range"
            min={20}
            max={64}
            value={cfg.fontSize}
            onChange={(e) => setCfg((c) => ({ ...c, fontSize: +e.target.value }))}
          />
        </label>
        <label className="pspeed" title="Scroll speed">
          speed
          <input type="range" min={12} max={140} value={cfg.speed} onChange={(e) => setCfg((c) => ({ ...c, speed: +e.target.value }))} />
        </label>
        <label className="pspeed" title="Mirror text (presenter-facing glass prompter)">
          mirror
          <input type="checkbox" checked={cfg.mirror} onChange={(e) => setCfg((c) => ({ ...c, mirror: e.target.checked }))} />
        </label>
        <span className="tb-spacer" />
        <button onClick={onClose} title="Close prompter">✕</button>
      </div>
      <div className="prompter-hint" style={{ opacity: cfg.opacity < 0.3 ? 1 : 0.7 }}>
        not recorded — this overlay never goes into your capture (unless you record this tab)
      </div>
      {editing ? (
        <textarea
          className="prompter-edit"
          value={cfg.text}
          onChange={(e) => setCfg((c) => ({ ...c, text: e.target.value }))}
          placeholder="Paste your script…"
        />
      ) : (
        <div
          className="prompter-scroll prompter-scroll-glass"
          ref={boxRef}
          onClick={() => setScrolling((s) => !s)}
          style={{ opacity: cfg.opacity }}
        >
          {/* reading-line highlight band — the words the presenter should be speaking */}
          <div className="prompter-reading-line" style={{ top: readingLineTop }} aria-hidden />
          <div
            style={{ transform: cfg.mirror ? "scaleX(-1)" : undefined }}
          >
            {cfg.text.split("\n").map((l, i) => (
              <p key={i} style={{ fontSize: cfg.fontSize }}>{l || "\u00A0"}</p>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}