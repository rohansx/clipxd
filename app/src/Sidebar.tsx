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
  icon: string;
  label: string;
  view: CloudView;
  /** Hex / token for the nav chip background when the row is active. */
  accent: string;
  count?: string;
}

export function Sidebar({ cloudView, clipCount, onNav, onBrand, user, onLogout }: SidebarProps) {
  const items: NavDef[] = [
    { icon: "▦", label: "Library", view: "library", count: clipCount > 0 ? String(clipCount) : "", accent: "var(--grape)" },
    { icon: "●", label: "Recording", view: "recording", accent: "var(--sodium)" },
    { icon: "↧", label: "Import", view: "import", accent: "var(--signal)" },
    { icon: "◈", label: "Ask agent", view: "chat", accent: "var(--signal)" },
    { icon: "⚙", label: "Settings", view: "library", accent: "var(--text-3)" },
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

      <div className="side-workspace">WORKSPACE · local</div>

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
        <div className="sub">0 px egress · {clipCount} clips indexed</div>
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
