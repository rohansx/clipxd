import { useEffect, useState } from "react";
import { fetchKeyStatus, fetchNet, githubLoginUrl, saveKeys, type AuthUser, type KeyStatus, type NetInfo } from "./api";
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

        {authEnabled && user && <KeysCard showToast={showToast} />}

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
  // The branded slug form is what shareLink() actually hands out now — mirror its preferred
  // branch rather than showing the legacy /clip/<id> shape.
  const shareBase = net?.user_slug_share_base ?? (user.username ? `${location.origin}/u/${user.username}` : null);

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
          {shareBase ? <>Your clips share at <code className="settings-mono">{shareBase}/&lt;title-slug&gt;</code></> : "Pick one so your share links carry your name instead of a bare id."}
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

/** BYOK: per-user NVIDIA/Gemini/Moondream keys + the server-vs-local captioning toggle. Key
 *  values are write-only from the client's point of view — the server only ever reports
 *  presence/absence (`KeyStatus`), never the stored key, so this card never renders one back. */
function KeysCard({ showToast }: { showToast: (m: string) => void }) {
  const [status, setStatus] = useState<KeyStatus | null>(null);
  const [modeBusy, setModeBusy] = useState(false);
  const [modeErr, setModeErr] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    fetchKeyStatus().then((s) => { if (live) setStatus(s); }).catch(() => {});
    return () => { live = false; };
  }, []);

  const saveField = async (field: "nvidia_api_key" | "gemini_api_key" | "moondream_api_key", value: string | null) => {
    const next = await saveKeys({ [field]: value });
    setStatus(next);
    showToast(value ? "Key saved." : "Key cleared.");
  };

  const setMode = async (mode: "server" | "local") => {
    if (!status || status.caption_mode === mode || modeBusy) return;
    setModeErr(null);
    setModeBusy(true);
    try {
      setStatus(await saveKeys({ caption_mode: mode }));
    } catch (e) {
      setModeErr(e instanceof Error ? e.message : "Couldn't change caption mode.");
    } finally {
      setModeBusy(false);
    }
  };

  if (!status) return null;

  return (
    <div className="settings-card">
      <div className="settings-card-head">
        <span className="settings-card-icon" aria-hidden>⚿</span>
        <b>Bring your own keys</b>
      </div>
      <p className="settings-note" style={{ margin: 0 }}>
        Use your own NVIDIA / Gemini / Moondream keys instead of the shared server ones — your
        usage lands on your own account, not ours. A saved key is never sent back to the
        browser; this page only ever shows whether one is configured.
      </p>

      <KeyRow
        label="NVIDIA API key"
        hint="Powers title / tl;dr / chapters (kimi-k2.6 → minimax-m2.7 → glm4.7 cascade)."
        configured={status.has_nvidia}
        onSave={(v) => saveField("nvidia_api_key", v)}
        onClear={() => saveField("nvidia_api_key", null)}
      />
      <KeyRow
        label="Gemini API key"
        hint="Fallback LLM backend, used if NVIDIA isn't configured or a call fails."
        configured={status.has_gemini}
        onSave={(v) => saveField("gemini_api_key", v)}
        onClear={() => saveField("gemini_api_key", null)}
      />
      <KeyRow
        label="Moondream API key"
        hint="Overrides the server's shared Moondream cloud key for this account's captions."
        configured={status.has_moondream}
        onSave={(v) => saveField("moondream_api_key", v)}
        onClear={() => saveField("moondream_api_key", null)}
      />

      <div className="settings-username">
        <div className="settings-row-label">Captioning</div>
        <div className="settings-row-hint" style={{ marginBottom: 8 }}>Where frame captions get generated for your clips.</div>
        <div className="settings-mode-toggle">
          <button className={status.caption_mode === "server" ? "on" : ""} onClick={() => setMode("server")} disabled={modeBusy}>
            Server default
          </button>
          <button className={status.caption_mode === "local" ? "on" : ""} onClick={() => setMode("local")} disabled={modeBusy}>
            Run captioning locally in my browser
          </button>
        </div>
        {status.caption_mode === "local" && (
          <p className="settings-note" style={{ marginTop: 10 }}>
            Captions are generated on your own device via WebGPU instead of our server — needs a
            WebGPU-capable browser. Falls back to no local captions if your browser doesn't
            support it.
          </p>
        )}
        {modeErr && <div className="auth-err" style={{ marginTop: 8 }}>{modeErr}</div>}
      </div>
    </div>
  );
}

interface KeyRowProps {
  label: string;
  hint: string;
  configured: boolean;
  onSave: (value: string) => Promise<void>;
  onClear: () => Promise<void>;
}

function KeyRow({ label, hint, configured, onSave, onClear }: KeyRowProps) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState<"save" | "clear" | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const save = async () => {
    if (!value.trim() || busy) return;
    setErr(null);
    setBusy("save");
    try {
      await onSave(value.trim());
      setValue("");
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Couldn't save that key.");
    } finally {
      setBusy(null);
    }
  };

  const clear = async () => {
    if (busy) return;
    setErr(null);
    setBusy("clear");
    try {
      await onClear();
      setValue("");
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Couldn't clear that key.");
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="settings-key-row">
      <div className="settings-row">
        <div>
          <div className="settings-row-label">{label}</div>
          <div className="settings-row-hint">{hint}</div>
        </div>
        <span className={`pill${configured ? " signal" : ""}`}>{configured ? "✓ configured" : "not set"}</span>
      </div>
      <div className="settings-username-row">
        <input
          className="input mono"
          type="password"
          autoComplete="off"
          spellCheck={false}
          placeholder={configured ? "set — enter a new key to replace it" : "paste your key"}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && save()}
        />
        <button className="btn btn-pill" onClick={save} disabled={!value.trim() || !!busy} style={{ padding: "0 16px" }}>
          {busy === "save" ? <span className="spin" /> : "Save"}
        </button>
        {configured && (
          <button className="btn btn-ghost btn-pill" onClick={clear} disabled={!!busy} style={{ padding: "0 14px" }}>
            {busy === "clear" ? <span className="spin" /> : "Clear"}
          </button>
        )}
      </div>
      {err && <div className="auth-err" style={{ marginTop: 8 }}>{err}</div>}
    </div>
  );
}
