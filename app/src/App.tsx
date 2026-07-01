import { AnimatePresence, motion } from "framer-motion";
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
import { vMount, usePrefersReducedMotion } from "./motion";
import { Seo, SEO_VIEWS } from "./seo";

export type View = "landing" | "auth" | "cloud";
export type CloudView = "library" | "recording" | "import" | "chat" | "clip";
export type Theme = "light" | "dark";

/** A seek request from the topbar search → consumed by the open ClipPage (nonce forces re-fire). */
export interface SeekRequest {
  t: number;
  nonce: number;
}

export default function App() {
  const reduced = usePrefersReducedMotion();
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
    setActiveClipId(id);
    setCloudView("clip");
    setView("cloud");
    setFilter("");
  }, []);

  const goCloud = useCallback(
    (v: CloudView = "library") => {
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

  const goAuth = useCallback(() => setView("auth"), []);
  const goLanding = useCallback(() => setView("landing"), []);
  const goImport = useCallback(
    () => goCloud("import"),
    [goCloud],
  );

  // Called by Recording.tsx once the server hands us a clip id.
  //
  //   1. No auto-navigate.  The user just spent 30+ s recording; jumping
  //      away from the "Link ready" card is hostile.  Recording's view
  //      keeps the URL visible, the Copy / Open buttons reachable, AND
  //      the Library banner live.
  //   2. No toast (3 s toasts are invisible feedback).  The link-ready
  //      card on Recording + the banner on Library do the work.
  //
  // We DO update the URL bar to `?clip=…` so a refresh keeps the user
  // on their new clip (and the indexing banner stays mounted).
  const afterCreate = useCallback(
    (id: string) => {
      reload();
      const u = new URL(location.href);
      u.searchParams.set("clip", id);
      history.replaceState(null, "", u.toString());
    },
    [reload],
  );

  // Apply the theme on <html> so the env gradient + body vars resolve.
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

  // On login, reload the (now per-user) library and drop the user straight into the app.
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
          <motion.div
            key="landing"
            variants={vMount}
            initial={reduced ? false : "hidden"}
            animate="shown"
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
          </motion.div>
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
}) {
  const reduced = usePrefersReducedMotion();
  const baseProps = {
    initial: reduced ? false : { opacity: 0, y: 10 },
    animate: { opacity: 1, y: 0, transition: { duration: 0.32, ease: [0.22, 1, 0.36, 1] } },
    exit: { opacity: 0, y: -6, transition: { duration: 0.18 } },
  };
  return (
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
    </AnimatePresence>
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
