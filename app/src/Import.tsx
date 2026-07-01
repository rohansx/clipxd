import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import { importUrl } from "./api";
import { usePrefersReducedMotion } from "./motion";

interface ImportProps {
  initialUrl?: string;
  onDone: (id: string) => void;
  showToast: (m: string) => void;
}

const SOURCES = [
  { label: "Loom", host: "loom.com/share/…", tint: "oklch(0.6 0.18 280)" },
  { label: "Cap", host: "cap.so/s/…", tint: "oklch(0.62 0.16 200)" },
  { label: "YouTube", host: "youtube.com/watch…", tint: "oklch(0.62 0.2 25)" },
  { label: "Any MP4", host: "https://…/clip.mp4", tint: "oklch(0.6 0.14 160)" },
];

type StepState = "pending" | "active" | "done";

const STEP_DEFS = [
  { label: "Fetch & decode", detail: "yt-dlp / direct download → frames" },
  { label: "veyo salience gate", detail: "emit only frames where the scene changes" },
  { label: "Transcript · OCR · captions", detail: "whisper.cpp + PaddleOCR + Moondream2" },
  { label: "CloakPipe redaction", detail: "strip PII / secrets before indexing" },
  { label: "Publish index + MCP", detail: "/clip/<id> + index.json sidecar" },
];

export function ImportView({ initialUrl, onDone, showToast }: ImportProps) {
  const reduced = usePrefersReducedMotion();
  const [url, setUrl] = useState(initialUrl ?? "");
  const [busy, setBusy] = useState(false);
  const [active, setActive] = useState(-1);
  const [err, setErr] = useState<string | null>(null);
  const autoRan = useRef(false);

  const stepState = (i: number): StepState => {
    if (!busy && active < 0) return "pending";
    if (i < active) return "done";
    if (i === active) return "active";
    return "pending";
  };

  const run = async () => {
    const u = url.trim();
    if (!u || busy) return;
    setBusy(true);
    setErr(null);
    setActive(0);
    const timers = [1, 2, 3, 4].map((i, k) => window.setTimeout(() => setActive(i), (k + 1) * 1400));
    try {
      const id = await importUrl(u);
      timers.forEach(window.clearTimeout);
      setActive(STEP_DEFS.length);
      showToast("Imported — opening your clip");
      onDone(id);
    } catch (e) {
      timers.forEach(window.clearTimeout);
      setActive(-1);
      setErr(e instanceof Error ? e.message : "import failed");
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    if (initialUrl && initialUrl.trim() && !autoRan.current) {
      autoRan.current = true;
      void run();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="import">
      <div className="import-inner">
        <h1 className="view-title">Paste a Loom, Cap, or video URL. clipxd reads it.</h1>
        <p className="view-sub" style={{ marginTop: 6 }}>
          No recording, no manual download — same index, same MCP endpoint. The video stays at its source; only the index is built.
        </p>

        <div className="import-field">
          <div className="box">
            <span style={{ color: "var(--signal-text)", fontFamily: "var(--font-mono)", fontSize: 13 }}>↳</span>
            <input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && run()}
              placeholder="https://…/video.mp4  ·  loom.com/share/…  ·  youtube.com/watch?v=…"
              autoFocus
            />
          </div>
          <button className="btn-signal btn-pill" onClick={run} disabled={busy || !url.trim()} style={{ padding: "0 22px" }}>
            {busy ? <span className="spin" /> : "Read it →"}
          </button>
        </div>

        <div className="import-sources">
          {SOURCES.map((s) => (
            <span key={s.label} className="import-source">
              <span className="swatch" style={{ background: s.tint }} />
              {s.label}
              <span className="host">{s.host}</span>
            </span>
          ))}
        </div>

        {err && (
          <div className="redaction-note" style={{ marginTop: 18, color: "var(--danger)", borderColor: "var(--danger)" }}>
            {err}
          </div>
        )}

        <AnimatePresence>
          {(busy || active >= 0) && (
            <motion.div
              className="import-steps"
              initial={reduced ? false : { opacity: 0, y: 12 }}
              animate={{ opacity: 1, y: 0, transition: { duration: 0.32, ease: [0.22, 1, 0.36, 1] } }}
              exit={{ opacity: 0, y: 6, transition: { duration: 0.18 } }}
            >
              <div className="head">
                <span>reading {url || "source"} →</span>
                <span className="note">video stays at source — only the index is built</span>
              </div>
              <div className="body">
                {STEP_DEFS.map((st, i) => {
                  const s = stepState(i);
                  return (
                    <div key={i} className={"step " + s}>
                      <span className="glyph">
                        {s === "done" ? "✓" : s === "active" ? "●" : i + 1}
                      </span>
                      <div style={{ flex: 1 }}>
                        <div className="label">{st.label}</div>
                        <div className="detail">{st.detail}</div>
                      </div>
                      <span className="state">{s === "done" ? "done" : s === "active" ? "reading…" : "queued"}</span>
                    </div>
                  );
                })}
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}
