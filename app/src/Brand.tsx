interface BrandProps {
  onClick?: () => void;
  size?: number;
  showName?: boolean;
}

/** The clipxd mark (play ▶ + bracket ]) plus wordmark. Shared by nav + sidebar. */
export function Brand({ onClick, size = 32, showName = true }: BrandProps) {
  return (
    <div className="brand" onClick={onClick} role={onClick ? "button" : undefined} tabIndex={onClick ? 0 : undefined}>
      <span className="mark">
        <svg width={size} height={size} viewBox="0 0 32 32" fill="none" aria-label="ClipXD">
          <rect width="32" height="32" rx="2" fill="#17151E" />
          <rect x="0.6" y="0.6" width="30.8" height="30.8" rx="1.6" fill="none" stroke="rgba(255,255,255,.10)" />
          <path d="M10 9.5 L10 22.5 L18 16 Z" fill="#FF6A45" />
          <path d="M21 10 H24.6 V22 H21" fill="none" stroke="#2DD4A8" strokeWidth="2.4" strokeLinecap="square" strokeLinejoin="miter" />
        </svg>
      </span>
      {showName && (
        <span className="name">
          Clip<b>XD</b>
        </span>
      )}
    </div>
  );
}
