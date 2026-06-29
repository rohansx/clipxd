import { Brand } from "./Brand";
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
  count?: number;
}

export function Sidebar({ cloudView, clipCount, onNav, onBrand, user, onLogout }: SidebarProps) {
  const items: NavDef[] = [
    { icon: "▦", label: "Library", view: "library", count: clipCount },
    { icon: "●", label: "Recording", view: "recording" },
    { icon: "↧", label: "Import", view: "import" },
    { icon: "◈", label: "Ask agent", view: "chat" },
  ];
  // "Library" stays highlighted while viewing a clip (clips live under the library).
  const isActive = (v: CloudView) => cloudView === v || (v === "library" && cloudView === "clip");

  return (
    <aside className="sidebar">
      <div className="side-brand" onClick={onBrand}>
        <Brand onClick={onBrand} size={26} />
      </div>
      <div className="side-head">WORKSPACE · local</div>
      {items.map((n) => (
        <button key={n.view} className={"side-item" + (isActive(n.view) ? " active" : "")} onClick={() => onNav(n.view)}>
          <span className="ico">{n.icon}</span>
          {n.label}
          {n.count != null && n.count > 0 && <span className="count">{n.count}</span>}
        </button>
      ))}
      <div className="side-foot">
        <div className="row">
          <span className="led-on" />
          clipxd-web · connected
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
