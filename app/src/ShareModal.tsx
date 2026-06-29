import { useState } from "react";

interface ShareModalProps {
  url: string;
  onClose: () => void;
}

/**
 * Share a clip. The URL resolves to the backend's watch+ask page (which itself renders a
 * scan-to-phone QR, generated offline). We show the link, a copy button, and a way to open it.
 */
export function ShareModal({ url, onClose }: ShareModalProps) {
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
    <div className="modal-scrim" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="m-head">
          <span className="dot signal" />
          <b style={{ fontFamily: "var(--font-display)", fontWeight: 500, fontSize: 17 }}>Share this clip</b>
          <span style={{ flex: 1 }} />
          <button className="btn-ghost" onClick={onClose} style={{ borderRadius: 0, padding: "5px 10px" }}>
            ✕
          </button>
        </div>
        <div className="m-body">
          <p style={{ color: "var(--text-2)", fontSize: 13.5, textAlign: "center" }}>
            A read-only link — anyone with it can watch the clip and ask it questions. The page includes a scan-to-phone QR.
          </p>
          <input className="input mono" value={url} readOnly onFocus={(e) => e.currentTarget.select()} style={{ textAlign: "center" }} />
          <div style={{ display: "flex", gap: 10 }}>
            <button className="btn-signal" onClick={copy} style={{ borderRadius: 0 }}>
              {copied ? "✓ Copied" : "Copy link"}
            </button>
            <a className="btn" href={url} target="_blank" rel="noreferrer" style={{ borderRadius: 0, textDecoration: "none" }}>
              Open watch page ↗
            </a>
          </div>
        </div>
      </div>
    </div>
  );
}
