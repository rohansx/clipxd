import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Brand } from "./Brand";
import { Landing } from "./Landing";
import { Sidebar } from "./Sidebar";
import { Library } from "./Library";
import { ClipPage } from "./ClipPage";
import { Recording } from "./Recording";
import { ImportView } from "./Import";
import { Chat } from "./Chat";
import { SearchBox } from "./SearchBox";
import { Login } from "./Login";
import { useClips } from "./useClipData";
import { useAuth } from "./useAuth";
import { initialClipId } from "./api";

export type View = "landing" | "auth" | "cloud";
export type CloudView = "library" | "recording" | "import" | "chat" | "clip";
export type Theme = "light" | "dark";

/** A seek request from the topbar search → consumed by the open ClipPage (nonce forces re-fire). */
export interface SeekRequest {
  t: number;
  nonce: number;
}

export default function App() {
  const deepLink = useMemo(initialClipId, []);
  const [theme, setTheme] = useState<Theme>("light");
  const [view, setView] = useState<View>(deepLink ? "cloud" : "landing");
  const [cloudView, setCloudView] = useState<CloudView>(deepLink ? "clip" : "library");
  const [activeClipId, setActiveClipId] = useState<string | null>(deepLink);
  const [toast, setToast] = useState<string | null>(null);
  const [seekTo, setSeekTo] = useState<SeekRequest | null>(null);
  const [filter, setFilter] = useState("");
  const [importUrl, setImportUrl] = useState<string | undefined>(undefined);

  const auth = useAuth();
  const { clips, reload } = useClips();

  const toggleTheme = () => setTheme((t) => (t === "light" ? "dark" : "light"));
  // Timer handle in a ref so a rapid second toast actually cancels the first (a plain function
  // property is lost on the re-render that setToast triggers).
  const toastTimer = useRef<number | undefined>(undefined);
  const showToast = useCallback((msg: string) => {
    setToast(msg);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 3000);
  }, []);

  const openClip = (id: string) => {
    setActiveClipId(id);
    setCloudView("clip");
    setView("cloud");
    setFilter("");
  };
  const goCloud = (v: CloudView = "library") => {
    // When unauthed, route to the explicit auth view (don't flash through Landing → Login).
    if (auth.authEnabled && !auth.user) {
      setView("auth");
      return;
    }
    setView("cloud");
    setCloudView(v);
  };

  const goAuth = () => setView("auth");
  const goLanding = () => setView("landing");
  const afterCreate = (id: string) => {
    reload();
    const username = auth.user?.username;
    const url = username
      ? `${location.origin}/u/${username}/clip/${id}`
      : `${location.origin}/?clip=${id}`;
    navigator.clipboard.writeText(url).catch(() => {});
    showToast("Link copied to clipboard");
    openClip(id);
  };

  // Apply the theme on <html> so `html,body{background:var(--env),var(--bg)}` resolves the
  // theme variables (they're defined on [data-theme]). Without this the body background is empty
  // and translucent panel washes paint over the browser's default white.
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
  }, [theme]);

  // Reflect the open clip in the URL (shareable/refreshable) without a reload.
  useEffect(() => {
    const u = new URL(location.href);
    if (view === "cloud" && cloudView === "clip" && activeClipId) u.searchParams.set("clip", activeClipId);
    else u.searchParams.delete("clip");
    history.replaceState(null, "", u.toString());
  }, [view, cloudView, activeClipId]);

  // On login, reload the (now per-user) library and drop the user straight into the app
  // (a hosted account shouldn't land on the marketing page). Deep links keep their target.
  useEffect(() => {
    if (auth.user) {
      reload();
      if (!deepLink) {
        setView("cloud");
        setCloudView("library");
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auth.user?.id]);

  // Loading / auth gate.
  // - auth.loading → spinner.
  // - explicit auth view → Login screen (always reachable, with back-to-landing).
  // - deep link (someone shared a /u/me/clip/abc URL) → still publicly watchable.
  // - otherwise → Landing (which has its own Login button for discoverability).
  if (auth.loading) {
    return (
      <div data-theme={theme} className="auth-screen">
        <span className="spin" style={{ width: 22, height: 22 }} />
      </div>
    );
  }
  if (view === "auth") {
    return (
      <div data-theme={theme}>
        <div className="auth-screen">
          <div className="auth-card" style={{ position: "relative" }}>
            <button
              className="auth-back"
              onClick={goLanding}
              title="Back to landing"
              aria-label="Back to landing"
            >
              ← landing
            </button>
            <Login onLogin={auth.login} onSignup={auth.signup} />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div data-theme={theme}>
      {view === "landing" ? (
        <Landing
          theme={theme}
          toggleTheme={toggleTheme}
          onOpenApp={() => goCloud("library")}
          onLogin={goAuth}
        />
      ) : (
        <div className="cloud">
          <Sidebar
            cloudView={cloudView}
            clipCount={clips?.length ?? 0}
            onNav={(v) => setCloudView(v)}
            onBrand={() => setView("landing")}
            user={auth.user}
            onLogout={auth.user ? () => auth.logout() : undefined}
          />
          <main className="main">
            <div className="topbar">
              {cloudView !== "library" && (
                <button className="btn-ghost" onClick={() => setCloudView("library")} style={{ borderRadius: 0, padding: "6px 12px", fontSize: 13 }}>
                  ← Library
                </button>
              )}
              <SearchBox
                cloudView={cloudView}
                clipId={activeClipId}
                filter={filter}
                onFilter={setFilter}
                onSeek={(t) => setSeekTo({ t, nonce: Date.now() })}
              />
              <div style={{ flex: 1 }} />
              <ThemeToggle theme={theme} toggleTheme={toggleTheme} />
              <button className="btn-sodium" onClick={() => setCloudView("recording")} style={{ borderRadius: 0 }}>
                <span className="dot" style={{ background: "var(--on-accent)" }} /> Record
              </button>
            </div>

            {cloudView === "library" && (
              <Library
                clips={clips}
                filter={filter}
                onOpen={openClip}
                onPasteImport={(url) => {
                  setImportUrl(url);
                  setCloudView("import");
                }}
              />
            )}
            {cloudView === "clip" && (
              <ClipPage key={activeClipId ?? "none"} id={activeClipId} seekTo={seekTo} showToast={showToast} />
            )}
            {cloudView === "recording" && <Recording onClipReady={afterCreate} showToast={showToast} />}
            {cloudView === "import" && <ImportView initialUrl={importUrl} onDone={afterCreate} showToast={showToast} />}
            {cloudView === "chat" && <Chat clips={clips} onOpen={openClip} />}
          </main>
        </div>
      )}

      {toast && (
        <div className="toast" role="status" aria-live="polite">
          {toast}
        </div>
      )}
    </div>
  );
}

function ThemeToggle({ theme, toggleTheme }: { theme: Theme; toggleTheme: () => void }) {
  const dark = theme === "dark";
  return (
    <button
      className="theme-toggle"
      onClick={toggleTheme}
      title="Light studio ⟷ night studio"
      aria-label={dark ? "Switch to light studio theme" : "Switch to night studio theme"}
      aria-pressed={dark}
    >
      <span aria-hidden style={{ background: !dark ? "var(--sodium)" : "transparent", color: !dark ? "var(--on-accent)" : "var(--text-3)" }}>W</span>
      <span aria-hidden style={{ background: dark ? "var(--signal)" : "transparent", color: dark ? "var(--on-accent)" : "var(--text-3)" }}>R</span>
    </button>
  );
}

export { Brand };
