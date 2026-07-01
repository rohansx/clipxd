/**
 * The clipxd mark — a clay tablet with a sodium play wedge.
 * Two SVG variants:
 *   - "logo"     → the full logo with sodium play + signal highlight (for branding surfaces)
 *   - "logomark" → just the rounded square (for the sidebar bubble where space is tight)
 *
 * Inline SVG (no asset fetch) keeps first paint fast and lets the parent theme the
 * highlights via CSS variables on the parent element.
 */
interface BrandProps {
  size?: number;
  /** Wrap the mark in the "ClipXD" wordmark pill (compact centred auth/login layout). */
  withWord?: boolean;
}

function SodiumPlay() {
  /* The sodium wedge — also drawn twice (offset + main) to fake a 3D shadow like the design. */
  return (
    <>
      <path
        d="M16.5 13.6 L16.5 25.4 L26.6 19.9 Z"
        fill="#C8432A"
        transform="translate(0,1.6)"
        stroke="#C8432A"
        strokeWidth="3"
        strokeLinejoin="round"
      />
      <path
        d="M16.5 13.6 L16.5 25.4 L26.6 19.9 Z"
        fill="url(#lx_play)"
        stroke="url(#lx_play)"
        strokeWidth="3"
        strokeLinejoin="round"
      />
    </>
  );
}

function MarkSvg({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 40 40" fill="none" aria-hidden>
      <defs>
        <linearGradient id={`lx_side_${size}`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#19D7A6" />
          <stop offset="1" stopColor="#0B7E5F" />
        </linearGradient>
        <linearGradient id={`lx_face_${size}`} x1="0.2" y1="0" x2="0.7" y2="1">
          <stop offset="0" stopColor="#FFFFFF" />
          <stop offset="0.55" stopColor="#F6EEFA" />
          <stop offset="1" stopColor="#E4D6F0" />
        </linearGradient>
        <linearGradient id={`lx_play_${size}`} x1="0.1" y1="0.05" x2="0.85" y2="0.95">
          <stop offset="0" stopColor="#FFB48F" />
          <stop offset="0.45" stopColor="#FF7A59" />
          <stop offset="1" stopColor="#EF5A39" />
        </linearGradient>
        <radialGradient id={`lx_spec_${size}`} cx="0.32" cy="0.26" r="0.55">
          <stop offset="0" stopColor="#FFFFFF" stopOpacity="0.9" />
          <stop offset="1" stopColor="#FFFFFF" stopOpacity="0" />
        </radialGradient>
      </defs>
      <rect x="5" y="8.5" width="30" height="28" rx="11" fill={`url(#lx_side_${size})`} />
      <rect x="5" y="4.5" width="30" height="29" rx="11" fill={`url(#lx_face_${size})`} />
      <path
        d="M11 6.5 H29 C32 6.5 33.5 8 33.5 11 V15 C29 12 11 12 6.5 15 V11 C6.5 8 8 6.5 11 6.5 Z"
        fill={`url(#lx_spec_${size})`}
        opacity="0.7"
      />
      <SodiumPlay />
      <ellipse
        cx="18.6"
        cy="16.4"
        rx="2.1"
        ry="1.4"
        fill="#FFFFFF"
        opacity="0.6"
        transform="rotate(-32 18.6 16.4)"
      />
    </svg>
  );
}

export function Brand({ size = 40, withWord = false }: BrandProps) {
  if (!withWord) {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 40 40"
        fill="none"
        aria-label="ClipXD"
        role="img"
      >
        <defs>
          <linearGradient id="bx_side" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stopColor="#19D7A6" />
            <stop offset="1" stopColor="#0B7E5F" />
          </linearGradient>
          <linearGradient id="bx_face" x1="0.2" y1="0" x2="0.7" y2="1">
            <stop offset="0" stopColor="#FFFFFF" />
            <stop offset="0.55" stopColor="#F6EEFA" />
            <stop offset="1" stopColor="#E4D6F0" />
          </linearGradient>
          <linearGradient id="bx_play" x1="0.1" y1="0.05" x2="0.85" y2="0.95">
            <stop offset="0" stopColor="#FFB48F" />
            <stop offset="0.45" stopColor="#FF7A59" />
            <stop offset="1" stopColor="#EF5A39" />
          </linearGradient>
          <radialGradient id="bx_spec" cx="0.32" cy="0.26" r="0.55">
            <stop offset="0" stopColor="#FFFFFF" stopOpacity="0.9" />
            <stop offset="1" stopColor="#FFFFFF" stopOpacity="0" />
          </radialGradient>
        </defs>
        <rect x="5" y="8.5" width="30" height="28" rx="11" fill="url(#bx_side)" />
        <rect x="5" y="4.5" width="30" height="29" rx="11" fill="url(#bx_face)" />
        <path d="M11 6.5 H29 C32 6.5 33.5 8 33.5 11 V15 C29 12 11 12 6.5 15 V11 C6.5 8 8 6.5 11 6.5 Z" fill="url(#bx_spec)" opacity="0.7" />
        <SodiumPlay />
        <ellipse cx="18.6" cy="16.4" rx="2.1" ry="1.4" fill="#FFFFFF" opacity="0.6" transform="rotate(-32 18.6 16.4)" />
      </svg>
    );
  }
  return (
    <span
      className="brand"
      style={{ display: "inline-flex", alignItems: "center", gap: Math.max(8, size / 4) }}
    >
      <span style={{ display: "inline-flex", lineHeight: 0 }} aria-hidden>
        <MarkSvg size={size} />
      </span>
      <span
        style={{
          fontFamily: "var(--font-display)",
          fontWeight: 700,
          fontSize: size * 0.45,
          letterSpacing: "-.025em",
          display: "inline-flex",
          alignItems: "center",
          gap: Math.max(4, size / 8),
          color: "var(--text)",
        }}
      >
        Clip
        <span
          style={{
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
            background: "var(--signal)",
            color: "var(--on-accent)",
            fontSize: size * 0.32,
            fontWeight: 700,
            letterSpacing: 0,
            padding: "2px 7px 3px",
            borderRadius: Math.max(7, size / 5),
            transform: "rotate(-5deg)",
            boxShadow: "var(--clay-sm)",
          }}
        >
          XD
        </span>
      </span>
    </span>
  );
}

/** Compact 26px logomark — no spec highlight (saves bytes & paint in the sidebar). */
export function Logomark({ size = 26 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 40 40" fill="none" aria-hidden>
      <defs>
        <linearGradient id={`lm_side_${size}`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#19D7A6" />
          <stop offset="1" stopColor="#0B7E5F" />
        </linearGradient>
        <linearGradient id={`lm_face_${size}`} x1="0.2" y1="0" x2="0.7" y2="1">
          <stop offset="0" stopColor="#FFFFFF" />
          <stop offset="1" stopColor="#E4D6F0" />
        </linearGradient>
        <linearGradient id={`lm_play_${size}`} x1="0.1" y1="0.05" x2="0.85" y2="0.95">
          <stop offset="0" stopColor="#FFB48F" />
          <stop offset="1" stopColor="#EF5A39" />
        </linearGradient>
      </defs>
      <rect x="5" y="8.5" width="30" height="28" rx="11" fill={`url(#lm_side_${size})`} />
      <rect x="5" y="4.5" width="30" height="29" rx="11" fill={`url(#lm_face_${size})`} />
      <SodiumPlay />
    </svg>
  );
}
