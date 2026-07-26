import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useState } from "react";
import { apiBase } from "./api";
import { usePrefersReducedMotion } from "./motion";
import { useEscape } from "./useEscape";

interface ShareModalProps {
  url: string;
  /** The clip being shared — needed to read/write its link visibility. */
  clipId?: string | null;
  onClose: () => void;
}

type Visibility = "public" | "private";

/** Split the URL into the parts we want to emphasise differently in the preview.
 *  - `base` is the protocol + host (dim, supporting role)
 *  - `tail` is the path that names the clip (loud, the part you actually want people to read)
 *  Together they reassemble to the full URL.  The split lets us typeset the link as
 *  `clipxd.com / u/you / checkout-500-6ad3` so the brand reads first and the title second
 *  — the inverse of the old `<input>` that dumped the whole URL at one typographic weight
 *  and hid the meaningful part behind the bare token.
 */
function splitUrl(url: string): { base: string; tail: string } {
  const m = url.match(/^(https?:\/\/[^/]+)(\/.*)$/);
  if (!m) return { base: "", tail: url };
  return { base: m[1], tail: m[2] };
}

/**
 * Share a clip. The URL resolves to the backend's watch+ask page (which itself renders a
 * scan-to-phone QR, generated offline). We show the link, a copy button, and a way to open it.
 */
export function ShareModal({ url, clipId, onClose }: ShareModalProps) {
  const reduced = usePrefersReducedMotion();
  useEscape(onClose);
  const [copied, setCopied] = useState(false);
  const { base, tail } = splitUrl(url);

  // Who can open this link. Until now a clip URL was unguessable but not access-controlled —
  // anyone holding it could watch the video and read the index, signed in or not.
  const [visibility, setVisibility] = useState<Visibility | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    if (!clipId) return;
    let cancelled = false;
    fetch(`${apiBase()}/clip/${clipId}/visibility`, { credentials: "include" })
      .then((r) => (r.ok ? r.json() : null))
      .then((d: { visibility?: Visibility } | null) => {
        if (!cancelled && d?.visibility) setVisibility(d.visibility);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [clipId]);

  const setVis = async (next: Visibility) => {
    if (!clipId) return;
    setSaving(true);
    const previous = visibility;
    setVisibility(next); // optimistic — reverted below if the server disagrees
    try {
      const r = await fetch(`${apiBase()}/clip/${clipId}/visibility`, {
        method: "POST",
        credentials: "include",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ visibility: next }),
      });
      if (!r.ok) setVisibility(previous);
    } catch {
      setVisibility(previous);
    } finally {
      setSaving(false);
    }
  };
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
          className="modal share-modal"
          onClick={(e) => e.stopPropagation()}
          initial={reduced ? false : { opacity: 0, y: 12, scale: 0.98 }}
          animate={{ opacity: 1, y: 0, scale: 1, transition: { type: "spring", stiffness: 320, damping: 28 } }}
          exit={{ opacity: 0, y: 6, transition: { duration: 0.16 } }}
        >
          <div className="m-head">
            <span className="dot signal" />
            <b style={{ fontFamily: "var(--font-display)", fontWeight: 500, fontSize: 15 }}>Share this clip</b>
            <span style={{ flex: 1 }} />
            <button className="btn-ghost btn-pill" onClick={onClose} style={{ padding: "4px 9px", fontSize: 12 }}>
              ✕
            </button>
          </div>
          <div className="m-body">
            <div className="share-url" onClick={(e) => (e.currentTarget as HTMLElement).querySelector("button")?.focus()}>
              <span className="share-url-base">{base}</span>
              <span className="share-url-tail">{tail}</span>
              <button
                type="button"
                className="share-copy-inline"
                onClick={copy}
                aria-label={copied ? "Copied" : "Copy share link"}
              >
                {copied ? "✓ Copied" : "Copy"}
              </button>
            </div>
            {visibility && (
              <div className="share-vis" role="group" aria-label="Who can open this link">
                <button
                  type="button"
                  className={"filter-pill" + (visibility === "public" ? " on" : "")}
                  aria-pressed={visibility === "public"}
                  disabled={saving}
                  onClick={() => setVis("public")}
                >
                  Anyone with the link
                </button>
                <button
                  type="button"
                  className={"filter-pill" + (visibility === "private" ? " on" : "")}
                  aria-pressed={visibility === "private"}
                  disabled={saving}
                  onClick={() => setVis("private")}
                >
                  Only me
                </button>
                <span className="share-hint">
                  {visibility === "private"
                    ? "The link 404s for everyone but you — video, index and all."
                    : "Unlisted, but not access-controlled: anyone who gets the link can watch and read it."}
                </span>
              </div>
            )}
            <div className="share-actions">
              <a className="btn btn-pill" href={url} target="_blank" rel="noreferrer" style={{ textDecoration: "none" }}>
                Open watch page ↗
              </a>
              <span className="share-hint">QR & embed on the watch page</span>
            </div>
          </div>
        </motion.div>
      </motion.div>
    </AnimatePresence>
  );
}

