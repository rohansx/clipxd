import { AnimatePresence, motion } from "framer-motion";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Brand } from "./Brand";
import { Landing } from "./Landing";
import { Sidebar } from "./Sidebar";
import { SearchBox } from "./SearchBox";
import { useClips } from "./useClipData";
import { useAuth, type Auth } from "./useAuth";
import { initialClipId } from "./api";
import { vMount, usePrefersReducedMotion } from "./motion";
import { Seo, SEO_VIEWS } from "./seo";

// Cloud views are code-split so the landing-page bundle stays tiny.
// The marketing page is the first thing every visitor (and every crawler
// scoring Lighthouse) sees — keeping it small is the single biggest perf
// lever.
const Login = lazy(() => import("./Login").then((m) => ({ default: m.Login })));
const Library = lazy(() => import("./Library").then((m) => ({ default: m.Library })));
const ClipPage = lazy(() => import("./ClipPage").then((m) => ({ default: m.ClipPage })));
const Recording = lazy(() => import("./Recording").then((m) => ({ default: m.Recording })));
const ImportView = lazy(() => import("./Import").then((m) => ({ default: m.ImportView })));
const Chat = lazy(() => import("./Chat").then((m) => ({ default: m.Chat })));
const Settings = lazy(() => import("./Settings").then((m) => ({ default: m.Settings })));

export type View = "landing" | "auth" | "cloud";
export type CloudView = "library" | "recording" | "import" | "chat" | "clip" | "settings";
export type Theme = "light" | "dark";

/** A seek request from the topbar search → consumed by the open ClipPage (nonce forces re-fire). */
export interface SeekRequest {
  t: number;
  nonce: number;
}

/** Persisted across refreshes: "the user has been inside the app before" — lets a hard
 *  refresh on Library/Settings/etc. restore straight back into the app instead of always
 *  falling through to the marketing landing page (which otherwise has no memory that the
 *  user ever left it, since only `?clip=` deep links survive a reload on their own). */
const ENTERED_APP_KEY = "clipxd:enteredApp";
function hasEnteredAppBefore(): boolean {
  try {
    return localStorage.getItem(ENTERED_APP_KEY) === "1";
  } catch {
    return false;
  }
}
/** Persisted theme choice — same origin as the share page (`/clip/:id`), which reads this
 *  key directly to match whatever the user picked in the app instead of only ever following
 *  the OS-level `prefers-color-scheme` (see `share_html`'s inline theme script). */
const THEME_KEY = "clipxd:theme";
function initialTheme(): Theme {
  try {
    const saved = localStorage.getItem(THEME_KEY);
    if (saved === "light" || saved === "dark") return saved;
  } catch {
    /* storage may be unavailable */
  }
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}
function saveTheme(t: Theme): void {
  try {
    localStorage.setItem(THEME_KEY, t);
  } catch {
    /* storage may be unavailable */
  }
}

function markEnteredApp(): void {
  try {
    localStorage.setItem(ENTERED_APP_KEY, "1");
  } catch {
    /* storage may be unavailable */
  }
}

export default function App() {
  const reduced = usePrefersReducedMotion();
  const deepLink = useMemo(initialClipId, []);
  const [theme, setTheme] = useState<Theme>(initialTheme);
  const [view, setView] = useState<View>(deepLink ? "cloud" : "landing");
  const [cloudView, setCloudView] = useState<CloudView>(deepLink ? "clip" : "library");
  const [activeClipId, setActiveClipId] = useState<string | null>(deepLink);
  const [toast, setToast] = useState<string | null>(null);
  const [seekTo, setSeekTo] = useState<SeekRequest | null>(null);
  const [filter, setFilter] = useState("");
  const [importUrl, setImportUrl] = useState<string | undefined>(undefined);

  // Lazy by default (skip /auth/me while on the marketing landing — keeps it console-clean
  // for first-time visitors and off the critical path for LCP). But a returning visitor who's
  // been in the app before needs the real check to start immediately even while still on
  // "landing", or the restore-on-refresh effect below has nothing to correct itself against
  // and a valid session behind an OAuth redirect never gets discovered.
  const auth = useAuth(view === "cloud" || hasEnteredAppBefore());
  const { clips, reload } = useClips();

  const toggleTheme = useCallback(() => {
    setTheme((t) => (t === "light" ? "dark" : "light"));
  }, []);

  // Timer handle in a ref so a rapid second toast actually cancels the first
  // (a plain function-property handle is lost on the re-render that setToast triggers).
  const toastTimer = useRef<number | undefined>(undefined);
  const showToast = useCallback((msg: string) => {
    setToast(msg);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 2400);
  }, []);

  const openClip = useCallback((id: string) => {
    markEnteredApp();
    setActiveClipId(id);
    setCloudView("clip");
    setView("cloud");
    setFilter("");
  }, []);

  const goCloud = useCallback(
    (v: CloudView = "library") => {
      markEnteredApp();
      // When unauthed, route to the explicit auth view (don't flash through Landing → Login).
      if (auth.authEnabled && !auth.user) {
        setView("auth");
        return;
      }
      setView("cloud");
      setCloudView(v);
    },
    [auth.authEnabled, auth.user],
  );

  const goAuth = useCallback(() => {
    markEnteredApp();
    setView("auth");
  }, []);
  const goLanding = useCallback(() => setView("landing"), []);
  const goImport = useCallback(
    () => goCloud("import"),
    [goCloud],
  );

  // Called by Recording.tsx once the server hands us a clip id (fires on Stop, not just once
  // the final commit lands — by then Recording.tsx has usually already navigated the user to
  // the clip itself via onOpenClip/stopAndOpen, since the instant link works from the moment
  // recording started). This just keeps the URL bar (`?clip=…`) and library list in sync for
  // whichever view the user's actually looking at.
  const afterCreate = useCallback(
    (id: string) => {
      reload();
      const u = new URL(location.href);
      u.searchParams.set("clip", id);
      history.replaceState(null, "", u.toString());
    },
    [reload],
  );

  // Apply the theme on <html> so the env gradient + body vars resolve, and persist it so a
  // refresh (and the separate server-rendered share page, same origin) sees the same choice.
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    saveTheme(theme);
  }, [theme]);

  // Reflect the open clip in the URL (shareable/refreshable) without a reload.
  useEffect(() => {
    const u = new URL(location.href);
    if (view === "cloud" && cloudView === "clip" && activeClipId) u.searchParams.set("clip", activeClipId);
    else u.searchParams.delete("clip");
    history.replaceState(null, "", u.toString());
  }, [view, cloudView, activeClipId]);

  // On login, reload the (now per-user) library and drop the user straight into the app.
  useEffect(() => {
    if (auth.user) {
      markEnteredApp();
      reload();
      if (!deepLink) {
        setView("cloud");
        setCloudView("library");
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auth.user?.id]);

  // Refresh-persistence: a hard reload always re-mounts with `view: "landing"` unless a
  // `?clip=` deep link is present — Library/Settings/Recording/etc. have no URL trace of
  // their own, so without this the app silently dumps a returning user back on the
  // marketing page every refresh. Fires once on mount rather than waiting on `auth.loading`
  // (auth is lazy — `useAuth(view === "cloud")` never even fetches while we're still on
  // "landing" — so `goCloud` here runs optimistically; the effect below corrects course if
  // the real auth check, once it fires, turns out unauthenticated).
  useEffect(() => {
    if (deepLink || view !== "landing") return;
    if (!hasEnteredAppBefore()) return;
    goCloud("library");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // If the optimistic restore above lands us in the cloud view but the real auth check
  // (which only starts once `view === "cloud"`) comes back unauthenticated, don't strand
  // the user on a cloud view they can't use — send them to the auth screen instead.
  useEffect(() => {
    if (view === "cloud" && !auth.loading && auth.authEnabled && !auth.user) {
      setView("auth");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auth.loading, auth.authEnabled, auth.user]);

  // Auth gate — ONLY blocks the cloud view, never the landing. Showing a
  // spinner in place of the landing is the single biggest reason FCP/LCP
  // fail on a slow connection (the page paints a blank div while we wait
  // for /auth/status). The landing is a static marketing page and is
  // available without an account.
  if (view === "cloud" && auth.loading) {
    return (
      <div data-theme={theme} className="auth-screen" aria-busy="true" aria-label="Loading">
        <span className="spin" style={{ width: 22, height: 22 }} />
      </div>
    );
  }
  if (view === "auth") {
    return (
      <div data-theme={theme}>
        <Seo
          title={SEO_VIEWS.auth.title}
          description={SEO_VIEWS.auth.description}
          path={SEO_VIEWS.auth.path}
          noindex
        />
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
      <AnimatePresence mode="wait" initial={false}>
        {view === "landing" ? (
          /* Landing starts visible (no initial="hidden") so the prerendered
             shell has no flash of empty div between SSR-style paint and the
             React tree mounting. The <main> landmark + id="main" keeps the
             skip-link in index.html working after React takes over. */
          <motion.main
            key="landing"
            id="main"
            initial={false}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0, transition: { duration: 0.18 } }}
          >
            <Seo
              title={SEO_VIEWS.landing.title}
              description={SEO_VIEWS.landing.description}
              path={SEO_VIEWS.landing.path}
            />
            <Landing
              theme={theme}
              toggleTheme={toggleTheme}
              onOpenApp={() => goCloud("library")}
              onImport={goImport}
              onLogin={goAuth}
            />
          </motion.main>
        ) : (
          <motion.div
            key="cloud"
            variants={vMount}
            initial={reduced ? false : "hidden"}
            animate="shown"
            exit={{ opacity: 0, transition: { duration: 0.18 } }}
            className="cloud"
          >
            <Seo
              title={cloudView === "clip" ? "Clip" : SEO_VIEWS[cloudView].title}
              description={
                cloudView === "clip"
                  ? SEO_VIEWS.clip.description
                  : SEO_VIEWS[cloudView].description
              }
              path={cloudView === "clip" ? "/clip" : SEO_VIEWS[cloudView].path}
              noindex={cloudView === "clip" || cloudView === "chat"}
            />
            <Sidebar
              cloudView={cloudView}
              clipCount={clips?.length ?? 0}
              onNav={setCloudView}
              onBrand={goLanding}
              user={auth.user}
              onLogout={auth.user ? () => auth.logout() : undefined}
            />
            <main className="main">
              <div className="topbar">
                {cloudView !== "library" && (
                  <button
                    className="topbar-back"
                    onClick={() => setCloudView("library")}
                  >
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
                <button
                  className="btn btn-pill"
                  onClick={() => setCloudView("recording")}
                  style={{ background: "var(--sodium)", color: "var(--on-accent)", border: "none", boxShadow: "var(--pop-sodium)" }}
                >
                  <span className="dot" style={{ background: "var(--on-accent)" }} /> Record
                </button>
              </div>

              {/* inner view transitions are handled by each child keying on cloudView */}
              <ViewBody
                cloudView={cloudView}
                clips={clips}
                filter={filter}
                openClip={openClip}
                importUrl={importUrl}
                setImportUrl={setImportUrl}
                setCloudView={setCloudView}
                activeClipId={activeClipId}
                seekTo={seekTo}
                showToast={showToast}
                afterCreate={afterCreate}
                onClipInCloudView={openClip}
                auth={auth}
                theme={theme}
                toggleTheme={toggleTheme}
              />
            </main>
          </motion.div>
        )}
      </AnimatePresence>

      <AnimatePresence>
        {toast && (
          <motion.div
            key="toast"
            className="toast"
            role="status"
            aria-live="polite"
            initial={reduced ? false : { opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0, transition: tSoftSpring }}
            exit={{ opacity: 0, y: 6, transition: { duration: 0.18 } }}
          >
            {toast}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/* Soft toast spring — used in the toast stack. */
const tSoftSpring = { type: "spring", stiffness: 320, damping: 28, mass: 0.7 } as const;

/** Tiny inner router. Extracted so each route key can mount fresh + animate. */
function ViewBody(p: {
  cloudView: CloudView;
  clips: ReturnType<typeof useClips>["clips"];
  filter: string;
  openClip: (id: string) => void;
  importUrl: string | undefined;
  setImportUrl: (u: string | undefined) => void;
  setCloudView: (v: CloudView) => void;
  activeClipId: string | null;
  seekTo: SeekRequest | null;
  showToast: (m: string) => void;
  afterCreate: (id: string) => void;
  onClipInCloudView: (id: string) => void;
  auth: Auth;
  theme: Theme;
  toggleTheme: () => void;
}) {
  const reduced = usePrefersReducedMotion();
  const baseProps = {
    initial: reduced ? false : { opacity: 0, y: 10 },
    animate: { opacity: 1, y: 0, transition: { duration: 0.32, ease: [0.22, 1, 0.36, 1] } },
    exit: { opacity: 0, y: -6, transition: { duration: 0.18 } },
  };
  return (
    <Suspense
      fallback={
        <div className="auth-screen" aria-busy="true" aria-label="Loading view">
          <span className="spin" style={{ width: 22, height: 22 }} />
        </div>
      }
    >
      <AnimatePresence mode="wait" initial={false}>
        {p.cloudView === "library" && (
          <motion.div key="library" {...baseProps}>
            <Library
              clips={p.clips}
              filter={p.filter}
              onOpen={p.openClip}
              onPasteImport={(url) => {
                p.setImportUrl(url);
                p.setCloudView("import");
              }}
            />
          </motion.div>
        )}
        {p.cloudView === "clip" && (
          <motion.div key={"clip-" + (p.activeClipId ?? "none")} {...baseProps}>
            <ClipPage id={p.activeClipId} seekTo={p.seekTo} showToast={p.showToast} />
          </motion.div>
        )}
        {p.cloudView === "recording" && (
          <motion.div key="recording" {...baseProps}>
            <Recording
              onClipReady={p.afterCreate}
              showToast={p.showToast}
              onOpenClip={p.onClipInCloudView}
              onRetry={p.afterCreate}
            />
          </motion.div>
        )}
        {p.cloudView === "import" && (
          <motion.div key="import" {...baseProps}>
            <ImportView
              initialUrl={p.importUrl}
              onDone={p.afterCreate}
              showToast={p.showToast}
            />
          </motion.div>
        )}
        {p.cloudView === "chat" && (
          <motion.div key="chat" {...baseProps}>
            <Chat clips={p.clips} onOpen={p.openClip} />
          </motion.div>
        )}
        {p.cloudView === "settings" && (
          <motion.div key="settings" {...baseProps}>
            <Settings
              authEnabled={p.auth.authEnabled}
              user={p.auth.user}
              clipCount={p.clips?.length ?? 0}
              theme={p.theme}
              toggleTheme={p.toggleTheme}
              onSetUsername={(u) => p.auth.setUsername(u)}
              onLogout={p.auth.logout}
              showToast={p.showToast}
            />
          </motion.div>
        )}
      </AnimatePresence>
    </Suspense>
  );
}

function ThemeToggle({ theme, toggleTheme }: { theme: Theme; toggleTheme: () => void }) {
  const dark = theme === "dark";
  return (
    <button
      className="theme-pill"
      onClick={toggleTheme}
      title="Light studio ⟷ night studio"
      aria-label={dark ? "Switch to light studio theme" : "Switch to night studio theme"}
      aria-pressed={dark}
    >
      <span
        className={"theme-pill-cell" + (!dark ? " on-sodium" : "")}
        style={{ color: !dark ? "var(--on-accent)" : "var(--text-3)" }}
        aria-hidden
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none">
          <circle cx="12" cy="12" r="5.4" fill="currentColor" />
        </svg>
      </span>
      <span
        className={"theme-pill-cell" + (dark ? " on-signal" : "")}
        style={{ color: dark ? "var(--on-accent)" : "var(--text-3)" }}
        aria-hidden
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none">
          <path
            d="M21 14.3 A8.4 8.4 0 1 1 11.4 3.4 A6.5 6.5 0 0 0 21 14.3 Z"
            fill="currentColor"
          />
        </svg>
      </span>
    </button>
  );
}

export { Brand };
