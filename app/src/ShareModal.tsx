import { AnimatePresence, motion } from "framer-motion";
import { useState } from "react";
import { usePrefersReducedMotion } from "./motion";

interface ShareModalProps {
  url: string;
  onClose: () => void;
}

/**
 * Share a clip. The URL resolves to the backend's watch+ask page (which itself renders a
 * scan-to-phone QR, generated offline). We show the link, a copy button, and a way to open it.
 */
export function ShareModal({ url, onClose }: ShareModalProps) {
  const reduced = usePrefersReducedMotion();
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    } catch {
      /* noop — the field is selectable */
    }
  };
  return (
    <AnimatePresence>
      <motion.div
        className="modal-scrim"
        onClick={onClose}
        initial={reduced ? false : { opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0, transition: { duration: 0.16 } }}
      >
        <motion.div
          className="modal"
          onClick={(e) => e.stopPropagation()}
          initial={reduced ? false : { opacity: 0, y: 12, scale: 0.98 }}
          animate={{ opacity: 1, y: 0, scale: 1, transition: { type: "spring", stiffness: 320, damping: 28 } }}
          exit={{ opacity: 0, y: 6, transition: { duration: 0.16 } }}
        >
          <div className="m-head">
            <span className="dot signal" />
            <b style={{ fontFamily: "var(--font-display)", fontWeight: 500, fontSize: 17 }}>Share this clip</b>
            <span style={{ flex: 1 }} />
            <button className="btn-ghost btn-pill" onClick={onClose} style={{ padding: "5px 10px" }}>
              ✕
            </button>
          </div>
          <div className="m-body">
            <p style={{ color: "var(--text-2)", fontSize: 13.5, textAlign: "center" }}>
              A read-only link — anyone with it can watch the clip and ask it questions. The page includes a scan-to-phone QR.
            </p>
            <input className="input mono" value={url} readOnly onFocus={(e) => e.currentTarget.select()} style={{ textAlign: "center" }} />
            <div style={{ display: "flex", gap: 10 }}>
              <button className="btn-signal btn-pill" onClick={copy}>
                {copied ? "✓ Copied" : "Copy link"}
              </button>
              <a className="btn btn-pill" href={url} target="_blank" rel="noreferrer" style={{ textDecoration: "none" }}>
                Open watch page ↗
              </a>
            </div>
          </div>
        </motion.div>
      </motion.div>
    </AnimatePresence>
  );
}

