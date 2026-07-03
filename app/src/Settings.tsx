import { useEffect, useState } from "react";
import { fetchNet, githubLoginUrl, type AuthUser, type NetInfo } from "./api";
import type { Theme } from "./App";

interface SettingsProps {
  authEnabled: boolean;
  user: AuthUser | null;
  clipCount: number;
  theme: Theme;
  toggleTheme: () => void;
  onSetUsername: (username: string) => Promise<AuthUser>;
  onLogout: () => Promise<void>;
  showToast: (m: string) => void;
}

const SLUG_RE = /^[a-z0-9_-]{3,30}$/;

export function Settings({ authEnabled, user, clipCount, theme, toggleTheme, onSetUsername, onLogout, showToast }: SettingsProps) {
  const [net, setNet] = useState<NetInfo | null>(null);
  useEffect(() => {
    let live = true;
    fetchNet().then((n) => { if (live) setNet(n); });
    return () => { live = false; };
  }, []);

  return (
    <div className="view">
      <div className="view-head">
        <div>
          <h1 className="view-title">Settings</h1>
          <p className="view-sub">
            {authEnabled ? "Your account, share links, and this app's appearance." : "Local mode — everything stays on this machine."}
          </p>
        </div>
      </div>

      <div className="settings-grid">
        {authEnabled && user && (
          <AccountCard user={user} onSetUsername={onSetUsername} onLogout={onLogout} showToast={showToast} net={net} />
        )}

        <div className="settings-card">
          <div className="settings-card-head">
            <span className="settings-card-icon" aria-hidden>◐</span>
            <b>Appearance</b>
          </div>
          <div className="settings-row">
            <div>
              <div className="settings-row-label">Theme</div>
              <div className="settings-row-hint">Switch between light and night studio.</div>
            </div>
            <button className="btn btn-pill" onClick={toggleTheme}>
              {theme === "dark" ? "🌙 Night" : "☀ Light"} — switch
            </button>
          </div>
        </div>

        <div className="settings-card">
          <div className="settings-card-head">
            <span className="settings-card-icon" aria-hidden>◈</span>
            <b>{authEnabled ? "This server" : "This machine"}</b>
          </div>
          <div className="settings-row">
            <div>
              <div className="settings-row-label">Clips indexed</div>
              <div className="settings-row-hint">Every one queryable from its link — 0 px video egress by default.</div>
            </div>
            <span className="pill signal">{clipCount}</span>
          </div>
          {net?.lan_ip && (
            <div className="settings-row">
              <div>
                <div className="settings-row-label">LAN address</div>
                <div className="settings-row-hint">Anyone on this network can open your unlisted share links.</div>
              </div>
              <code className="settings-mono">{net.lan_ip}</code>
            </div>
          )}
          {net?.public_base && (
            <div className="settings-row">
              <div>
                <div className="settings-row-label">Public base</div>
                <div className="settings-row-hint">Share links resolve here.</div>
              </div>
              <code className="settings-mono">{net.public_base}</code>
            </div>
          )}
          {!authEnabled && (
            <p className="settings-note">
              This server has no accounts — it's single-user local/LAN mode. Every clip you
              record or import belongs to whoever can reach this box.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

function AccountCard({
  user,
  onSetUsername,
  onLogout,
  showToast,
  net,
}: {
  user: AuthUser;
  onSetUsername: (username: string) => Promise<AuthUser>;
  onLogout: () => Promise<void>;
  showToast: (m: string) => void;
  net: NetInfo | null;
}) {
  const [usernameInput, setUsernameInput] = useState(user.username ?? "");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [loggingOut, setLoggingOut] = useState(false);

  const dirty = usernameInput.trim() !== (user.username ?? "");
  const shareBase = net?.user_share_base ?? (user.username ? `${location.origin}/u/${user.username}/clip` : null);

  const saveUsername = async () => {
    const slug = usernameInput.trim();
    if (!SLUG_RE.test(slug)) {
      setErr("3-30 chars: lowercase letters, digits, '-' or '_'.");
      return;
    }
    setErr(null);
    setBusy(true);
    try {
      await onSetUsername(slug);
      showToast("Username updated — your share links now use it.");
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Couldn't save that username — it may be taken.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="settings-card">
      <div className="settings-card-head">
        <span className="settings-card-icon" aria-hidden>◎</span>
        <b>Account</b>
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-row-label">Signed in as</div>
          <div className="settings-row-hint">{user.name ? `${user.name} · ${user.email}` : user.email}</div>
        </div>
      </div>

      <div className="settings-row">
        <div>
          <div className="settings-row-label">GitHub</div>
          <div className="settings-row-hint">{user.github ? "Connected — you can sign in with GitHub." : "Not connected."}</div>
        </div>
        {user.github ? (
          <span className="pill signal">✓ connected</span>
        ) : (
          <a className="btn btn-pill" href={githubLoginUrl()}>Connect GitHub</a>
        )}
      </div>

      <div className="settings-username">
        <div className="settings-row-label">Username (your share-link slug)</div>
        <div className="settings-row-hint" style={{ marginBottom: 8 }}>
          {shareBase ? <>Your clips share at <code className="settings-mono">{shareBase}/&lt;id&gt;</code></> : "Pick one so your share links carry your name instead of a bare id."}
        </div>
        <div className="settings-username-row">
          <input
            className="input"
            value={usernameInput}
            autoComplete="off"
            spellCheck={false}
            placeholder="yourname"
            onChange={(e) => setUsernameInput(e.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, ""))}
            onKeyDown={(e) => e.key === "Enter" && dirty && !busy && saveUsername()}
          />
          <button className="btn-signal btn-pill" onClick={saveUsername} disabled={!dirty || busy} style={{ padding: "0 18px" }}>
            {busy ? <span className="spin" /> : user.username ? "Update" : "Claim"}
          </button>
        </div>
        {err && <div className="auth-err" style={{ marginTop: 8 }}>{err}</div>}
      </div>

      <div className="settings-row" style={{ marginTop: 4 }}>
        <div>
          <div className="settings-row-label">Session</div>
          <div className="settings-row-hint">Sign out of clipxd on this device.</div>
        </div>
        <button
          className="btn btn-pill"
          onClick={async () => {
            setLoggingOut(true);
            await onLogout().catch(() => {});
            setLoggingOut(false);
          }}
          disabled={loggingOut}
        >
          {loggingOut ? <span className="spin" /> : "Sign out"}
        </button>
      </div>
    </div>
  );
}
