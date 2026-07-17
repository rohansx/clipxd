import { Logomark } from "./Brand";
import type { CloudView } from "./App";
import type { AuthUser } from "./api";

interface SidebarProps {
  cloudView: CloudView;
  clipCount: number;
  onNav: (v: CloudView) => void;
  onBrand: () => void;
  user?: AuthUser | null;
  onLogout?: () => void;
}

interface NavDef {
  /** Inline SVG path data for the chip icon — a 14x14 viewBox so it scales with the chip
   *  size and inherits its `currentColor`.  Replaces the previous unicode blocks
   *  (▦ ● ↧ ◈ ▤ ⚙) which rendered as font fallback glyphs of wildly varying weight. */
  icon: React.ReactNode;
  label: string;
  view: CloudView;
  /** Token for the nav chip background when the row is active. */
  accent: string;
  count?: string;
}

/** A 14×14 stroked icon set, hand-picked to feel cohesive at the chip size. Each shape is
 *  closed enough to read at 16px but not so busy that it competes with the row label. */
function NavIcon({ d }: { d: string }) {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden focusable="false">
      <path d={d} stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

const ICONS = {
  // Library = a 2×2 grid of clips
  library: <NavIcon d="M2 3.2 H7 V8 H2 Z M9 3.2 H14 V8 H9 Z M2 9 H7 V13.8 H2 Z M9 9 H14 V13.8 H9 Z" />,
  // Recording = a filled circle inside a ring (the on-air dot)
  recording: (
    <NavIcon d="M8 2.5 A5.5 5.5 0 1 0 8 13.5 A5.5 5.5 0 1 0 8 2.5 Z M8 5.2 A2.8 2.8 0 1 0 8 10.8 A2.8 2.8 0 1 0 8 5.2 Z" />
  ),
  // Import = a tray with a down arrow into it
  import: <NavIcon d="M8 2 V9.2 M4.8 6 L8 9.2 L11.2 6 M2.6 11.6 H13.4 V13.8 H2.6 Z" />,
  // Ask agent = a speech bubble with a sparkle (the assistant)
  chat: <NavIcon d="M2.6 4.4 A1.6 1.6 0 0 1 4.2 2.8 H11.8 A1.6 1.6 0 0 1 13.4 4.4 V9.4 A1.6 1.6 0 0 1 11.8 11 H7.6 L4.8 13.6 V11 H4.2 A1.6 1.6 0 0 1 2.6 9.4 Z M6 6.6 H10 M6 8.6 H8.4" />,
  // Docs = a page with lines
  docs: <NavIcon d="M3.6 2 H10 L13 5 V14 H3.6 Z M9.6 2 V5.6 H13 M5.6 8.4 H11 M5.6 10.6 H11 M5.6 12.8 H8.6" />,
  // Settings = a gear (simplified)
  settings: (
    <NavIcon d="M8 5.6 A2.4 2.4 0 1 0 8 10.4 A2.4 2.4 0 1 0 8 5.6 Z M8 1.6 V3.2 M8 12.8 V14.4 M14.4 8 H12.8 M3.2 8 H1.6 M12.4 3.6 L11.3 4.7 M4.7 11.3 L3.6 12.4 M12.4 12.4 L11.3 11.3 M4.7 4.7 L3.6 3.6" />
  ),
};

export function Sidebar({ cloudView, clipCount, onNav, onBrand, user, onLogout }: SidebarProps) {
  const items: NavDef[] = [
    { icon: ICONS.library, label: "Library", view: "library", count: clipCount > 0 ? String(clipCount) : "", accent: "var(--grape)" },
    { icon: ICONS.recording, label: "Recording", view: "recording", accent: "var(--sodium)" },
    { icon: ICONS.import, label: "Import", view: "import", accent: "var(--signal)" },
    { icon: ICONS.chat, label: "Ask agent", view: "chat", accent: "var(--signal)" },
    { icon: ICONS.docs, label: "Docs", view: "docs", accent: "var(--grape)" },
    { icon: ICONS.settings, label: "Settings", view: "settings", accent: "var(--text-3)" },
  ];
  // "Library" stays highlighted while viewing a clip (clips live under the library).
  const isActive = (v: CloudView) =>
    cloudView === v || (v === "library" && cloudView === "clip");

  return (
    <aside className="sidebar">
      <div className="side-bubble" onClick={onBrand}>
        <span className="logo-glow">
          <Logomark size={26} />
        </span>
        <span className="name">
          Clip
          <span
            style={{
              display: "inline-flex",
              background: "var(--signal)",
              color: "var(--on-accent)",
              fontSize: 11,
              fontWeight: 700,
              padding: "1px 5px 2px",
              borderRadius: 7,
              transform: "rotate(-5deg)",
              boxShadow: "var(--clay-sm)",
              marginLeft: 3,
            }}
          >
            XD
          </span>
        </span>
      </div>

      {/* Tell the truth about where the clips actually live. Signed into the hosted service,
          "local" was simply false — and claiming local-first while running in our cloud
          undercuts the exact trust story the badge exists to tell. */}
      <div className="side-workspace">
        {user ? `WORKSPACE · ${user.username || "cloud"}` : "WORKSPACE · local"}
      </div>

      {items.map((n) => {
        const active = isActive(n.view);
        return (
          <button
            key={n.label}
            className={"side-row" + (active ? " active" : "")}
            onClick={() => onNav(n.view)}
            aria-pressed={active}
          >
            <span
              className="chip"
              style={{
                background: active ? n.accent : undefined,
                color: active ? "var(--on-accent)" : undefined,
              }}
            >
              <span className="chip-icon">{n.icon}</span>
            </span>
            {n.label}
            {n.count && <span className="count">{n.count}</span>}
          </button>
        );
      })}

      <div className="side-foot">
        <div className="row">
          <span className="dot signal" style={{ width: 8, height: 8, boxShadow: "0 0 8px var(--signal)" }} />
          MCP server · connected
        </div>
        {/* "0 px egress" is a claim about LOCAL mode (your pixels never leave the machine). On
            the hosted service the video is on our box, so state what's actually true there. */}
        <div className="sub">
          {user ? `${clipCount} clip${clipCount === 1 ? "" : "s"} indexed` : `0 px egress · ${clipCount} clips indexed`}
        </div>
        {user && (
          <div className="side-user">
            <span className="who" title={user.email}>{user.name || user.email}</span>
            {onLogout && (
              <button className="auth-link" onClick={onLogout}>
                Sign out
              </button>
            )}
          </div>
        )}
      </div>
    </aside>
  );
}
