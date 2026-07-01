import { motion } from "framer-motion";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Brand } from "./Brand";
import { vMount, vStagger, usePrefersReducedMotion } from "./motion";
import type { Theme } from "./App";

interface LandingProps {
  theme: Theme;
  toggleTheme: () => void;
  onOpenApp: () => void;
  onImport: () => void;
  onLogin?: () => void;
}

const PIPELINE = [
  { step: "01 · capture", title: "Thin recorder", desc: "Pixels + audio + cursor/click/key. Rust.", kicker: "var(--sodium-text)", last: false },
  { step: "02 · gate", title: "veyo-core", desc: "Salience gate, CPU. Emits a delta only when the scene changes.", kicker: "var(--signal-text)", last: false },
  { step: "03 · enrich", title: "veyo-enrich", desc: "Transcript · caption salient frames · OCR · structure events.", kicker: "var(--signal-text)", last: false },
  { step: "04 · clean", title: "CloakPipe", desc: "Strip PII / secrets from frames + transcript.", kicker: "var(--signal-text)", last: false },
  { step: "05 · store", title: "The artifact", desc: "Video file + structured index, side by side.", kicker: "var(--text-3)", last: false },
  { step: "06 · serve", title: "MCP + JSON", desc: "The link an agent queries. The moat.", kicker: "var(--signal-text)", last: true },
];

const MCP_TOOLS = [
  { fn: "query_clip(url, q)", desc: "natural-language Q&A over the index" },
  { fn: "get_frame_context(t)", desc: "everything known at timestamp t" },
  { fn: "search_text(q)", desc: "transcript + OCR full-text" },
  { fn: "get_events(range)", desc: "clicks, keys, network in a window" },
];

const INDEX_PARTS = [
  { tag: "txt", title: "Transcript", desc: "Time-aligned, speaker-tagged where possible. whisper.cpp, on device.", sample: '0:12 [host] "watch the\n  checkout when I hit pay"', span: "span 3" },
  { tag: "cap", title: "Visual timeline", desc: "Captions for salient moments only — veyo-gated, each timestamped.", sample: "0:41  red error banner\n      appears over form", span: "span 3" },
  { tag: "ocr", title: "On-screen text", desc: "OCR'd, searchable, timestamped — errors, code, labels, URLs.", sample: '0:41  "Payment failed:\n      card_declined"', span: "span 2" },
  { tag: "evt", title: "Event track", desc: "Cursor, clicks, keys, scroll. In browser mode: DOM, a11y, console, network.", sample: '0:40  click "Pay $89.00"\n0:41  POST /charge → 500', span: "span 2" },
  { tag: "sum", title: "Summary + metadata", desc: "Chapters, duration, app focus, redaction manifest. Derived, not source.", sample: "chapters: 4 · pii: 2 redacted\nfocus: shop.acme.test", span: "span 2" },
];

const COMPETITORS = [
  { name: "Loom", note: "human-only, closed", cells: ["✕", "✕", "✕", "✕"], bg: "var(--panel)" },
  { name: "Cap", note: "great recorder, AGPL", cells: ["✓", "✕", "✕", "✕"], bg: "var(--panel)" },
  { name: "Builder's Clips", note: "cloud template, agent-native", cells: ["✕", "✓", "✕", "✕"], bg: "var(--panel)" },
  { name: "clipxd", note: "the index is the product", cells: ["✓", "✓", "✓", "✓"], bg: "var(--signal-wash)", nameColor: "var(--signal-text)" },
];

/* Wipe seam bounds — clamped so the knob always stays visible. */
const WIPE_MIN = 4;
const WIPE_MAX = 96;
const WIPE_AUTO_MIN = 16;
const WIPE_AUTO_MAX = 82;

export function Landing({ theme, toggleTheme, onOpenApp, onImport, onLogin }: LandingProps) {
  const reduced = usePrefersReducedMotion();
  const [wipe, setWipe] = useState(50);
  const [autoSweep, setAutoSweep] = useState(true);
  const dir = useRef(1);

  // auto-sweep between two anchors; only runs while user is idle.
  useEffect(() => {
    if (reduced || !autoSweep) return;
    const id = window.setInterval(() => {
      setWipe((w) => {
        let n = w + dir.current * 0.55;
        if (n >= WIPE_AUTO_MAX) { n = WIPE_AUTO_MAX; dir.current = -1; }
        if (n <= WIPE_AUTO_MIN) { n = WIPE_AUTO_MIN; dir.current = 1; }
        return n;
      });
    }, 26);
    return () => window.clearInterval(id);
  }, [autoSweep, reduced]);

  const setFromX = useCallback((clientX: number, el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    const pct = Math.max(WIPE_MIN, Math.min(WIPE_MAX, ((clientX - r.left) / r.width) * 100));
    setWipe(pct);
    setAutoSweep(false);
  }, []);

  // pointer-based drag: attach to window for full gesture range, then tear down on up
  const startDrag = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const frame = e.currentTarget;
      const move = (ev: PointerEvent) => setFromX(ev.clientX, frame);
      const up = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
      setFromX(e.clientX, frame);
    },
    [setFromX],
  );

  const revealClip = useMemo(() => `inset(0 0 0 ${wipe}%)`, [wipe]);
  const dark = theme === "dark";

  return (
    <div className="landing">
      {/* ===================  FLOATING CLAY-GLASS NAV  =================== */}
      <nav className="landing-nav">
        <div className="landing-nav-inner lglass">
          <div className="landing-brand" onClick={(e) => e.stopPropagation()}>
            <span style={{ display: "inline-flex", lineHeight: 0, filter: "drop-shadow(0 8px 10px rgba(60,30,90,.32))" }}>
              <Brand size={40} />
            </span>
          </div>

          <span className="landing-nav-rec">
            <span
              className="dot sodium"
              style={{
                width: 7,
                height: 7,
                animation: reduced ? undefined : "recPulse 2s var(--ease-clip) infinite",
              }}
            />
            rec_ready
          </span>

          <div style={{ flex: 1 }} />

          <div className="landing-nav-tabs">
            <span className="landing-nav-tab on">
              <span style={{ color: "var(--sodium-text)" }}>◆</span>&nbsp;product
            </span>
            <span className="landing-nav-tab">
              <span style={{ color: "var(--grape)" }}>↗</span>&nbsp;cloud
            </span>
          </div>

          <ThemePill dark={dark} onClick={toggleTheme} />

          <button className="landing-cta-app" onClick={onOpenApp}>
            Open app
          </button>
        </div>
      </nav>

      {/* ===================  HERO  =================== */}
      <section className="hero">
        <div className="hero-grid">
          <motion.div
            variants={vMount}
            initial="hidden"
            animate="shown"
            style={{ minWidth: 0 }}
          >
            <div className="kicker">
              <span className="led" />powered by the veyo visual-event codec
            </div>
            <h1 className="hero-title">
              A screen recording
              <br />
              an agent can <span className="g">read</span>.
            </h1>
            <p className="hero-sub">
              Record once. Humans get a beautiful video. Agents get a structured index —{" "}
              <b>transcript, on-screen text, clicks, network, the moments that matter</b>{" "}
              — queryable straight from the link.{" "}
              <span className="em">Drag the seam below</span> to see one second, both ways.
            </p>
          </motion.div>

          <motion.div
            variants={vMount}
            initial="hidden"
            animate="shown"
            transition={{ delay: 0.06 }}
            style={{ minWidth: 0 }}
          >
            <div className="wipe-head">
              <span className="w">
                <span className="dot sodium" />WATCH — what a human sees
              </span>
              <span className="ts">t=0:41 · drag to parse ↔</span>
              <span className="r">
                how the machine reads it — READ
                <span className="dot signal" />
              </span>
            </div>

            <Wipe revealClip={revealClip} seamLeft={`${wipe}%`} onPointerDown={startDrag} />

            <div className="cta-row">
              <button className="btn-signal" onClick={onOpenApp} style={{ borderRadius: 14, fontSize: 15, padding: "13px 22px" }}>
                <span className="dot" style={{ background: "var(--on-accent)" }} />
                Record a clip
              </button>
              <button className="btn" onClick={onImport} style={{ borderRadius: 13, fontSize: 15, padding: "13px 22px" }}>
                Paste a Loom instead
              </button>
              <span
                className="seam-status"
                onClick={() => setAutoSweep(true)}
                title="Auto-sweep seam"
              >
                seam: {autoSweep ? "auto" : "drag"} · click to auto-sweep
              </span>
            </div>

            <div className="stat-ledger">
              {[
                { big: "0 px", sub: "leave your device — veyo runs on-CPU" },
                { big: "<1%", sub: "of frames enriched — salience-gated" },
                { big: "1 link", sub: "video + queryable index, same URL" },
              ].map((s) => (
                <div key={s.big} className="cell">
                  <div className="big">{s.big}</div>
                  <div className="sub">{s.sub}</div>
                </div>
              ))}
            </div>
          </motion.div>
        </div>

        {/* TRUST STRIP */}
        <div className="trust-strip">
          <div className="trust-pill lglass">
            <span>reads &amp; speaks</span>
            <span style={{ color: "var(--text-2)" }}>MCP</span>·<span style={{ color: "var(--text-2)" }}>Claude</span>·
            <span style={{ color: "var(--text-2)" }}>Loom imports</span>·<span style={{ color: "var(--text-2)" }}>Cap imports</span>·
            <span style={{ color: "var(--text-2)" }}>whisper.cpp</span>·<span style={{ color: "var(--text-2)" }}>CloakPipe</span>·
            <span style={{ color: "var(--text-2)" }}>local SQLite + FTS</span>
          </div>
        </div>

        {/* TWO-BODY THESIS */}
        <div className="landing-section" style={{ paddingTop: 50, paddingBottom: 6 }}>
          <div style={{ maxWidth: 620, marginBottom: 32 }}>
            <div className="section-kicker">the thesis</div>
            <h2 className="section-title" style={{ fontWeight: 500 }}>
              Two bodies, one recording.
            </h2>
            <p className="section-lede">
              A clip has a body a human <span style={{ color: "var(--sodium-text)" }}>watches</span> and a body an agent{" "}
              <span style={{ color: "var(--signal-text)" }}>reads</span>. Same content, two voices — warm and
              optical for people, structured and monospaced for machines.
            </p>
          </div>
          <TwoBody />
        </div>
      </section>

      {/* ===================  INDEX BREAKDOWN  =================== */}
      <section className="landing-section" style={{ paddingTop: 56 }}>
        <div style={{ maxWidth: 640 }}>
          <div className="section-kicker signal">the moat</div>
          <h2 className="section-title">An agent should never need the pixels.</h2>
          <p className="section-lede">
            Every clip resolves from one URL into a structured object — five tracks, all timestamped, all
            text. If the agent ever has to watch the video, the index failed.
          </p>
        </div>
        <motion.div
          className="index-grid"
          variants={vStagger(0.06, 0.05)}
          initial="hidden"
          whileInView="shown"
          viewport={{ once: true, margin: "-80px" }}
        >
          {INDEX_PARTS.map((p) => (
            <motion.div
              key={p.tag}
              variants={vMount}
              className="lift index-card"
              style={{ gridColumn: p.span }}
            >
              <div className="row">
                <span className="tag">{p.tag}</span>
                <span style={{ fontFamily: "var(--font-display)", fontWeight: 500, fontSize: 17 }}>{p.title}</span>
              </div>
              <p style={{ fontSize: 13.5, color: "var(--text-2)", lineHeight: 1.5 }}>{p.desc}</p>
              <div className="sample">{p.sample}</div>
            </motion.div>
          ))}
        </motion.div>
      </section>

      {/* ===================  PIPELINE  =================== */}
      <section className="landing-section" style={{ paddingTop: 50 }}>
        <motion.div
          className="pipe-card"
          variants={vMount}
          initial="hidden"
          whileInView="shown"
          viewport={{ once: true, margin: "-80px" }}
        >
          <div className="pipe-head">
            <div>
              <div className="section-kicker" style={{ marginBottom: 10 }}>
                architecture — thin recorder, fat index
              </div>
              <h2 className="section-title" style={{ fontSize: "clamp(24px,3vw,36px)" }}>
                How a recording becomes queryable
              </h2>
            </div>
            <div className="pill signal">no imagery leaves the device →</div>
          </div>
          <div className="pipe-row">
            {PIPELINE.map((n, i) => (
              <div className="pipe-node-wrap" key={n.step}>
                <div
                  className="pipe-node"
                  style={{
                    background: n.last ? "var(--signal-wash)" : "var(--panel)",
                  }}
                >
                  <span className="step" style={{ color: n.kicker }}>{n.step}</span>
                  <span className="ttl">{n.title}</span>
                  <span className="desc">{n.desc}</span>
                </div>
                {i < PIPELINE.length - 1 && <span className="pipe-arrow">→</span>}
              </div>
            ))}
          </div>
        </motion.div>
      </section>

      {/* ===================  FEATURES: cinematic + import  =================== */}
      <section
        className="landing-section"
        style={{ paddingTop: 24, paddingBottom: 18, display: "grid", gridTemplateColumns: "1fr 1fr", gap: 18 }}
      >
        <motion.div
          className="lift feat-card"
          variants={vMount}
          initial="hidden"
          whileInView="shown"
          viewport={{ once: true, margin: "-80px" }}
        >
          <div className="ttl-kicker" style={{ color: "var(--sodium-text)" }}>the adoption hook · watch</div>
          <h3>Cinematic by default</h3>
          <p className="lede">
            Auto-zoom follows the cursor and clicks — soft in, settled out, one easing curve across the
            whole product. Backgrounds, padding, device frames. Raw capture looks produced with zero effort.
          </p>
          <div className="demo">
            <div className="cinema-demo">
              <div className="cinema-chip">
                <div className="lbl">cursor → auto-zoom 2.4×</div>
                <div className="bar w1" />
                <div className="bar w2" />
              </div>
            </div>
          </div>
        </motion.div>

        <motion.div
          className="lift feat-card"
          variants={vMount}
          initial="hidden"
          whileInView="shown"
          viewport={{ once: true, margin: "-80px" }}
          transition={{ delay: 0.06 }}
        >
          <div className="ttl-kicker" style={{ color: "var(--signal-text)" }}>fastest path to value · read</div>
          <h3>Already have Looms? Paste them.</h3>
          <p className="lede">
            Drop any Loom, Cap, or video URL → clipxd reads it → same index, same MCP query. No recording
            required. "Stop sending me Looms I can't pass to an agent" — now you can.
          </p>
          <div className="demo">
            <div className="url-paste">
              <div className="box">loom.com/share/9c2f…</div>
              <button className="btn-signal" style={{ borderRadius: 12, padding: "0 18px" }}>
                Read it
              </button>
            </div>
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 9,
                fontFamily: "var(--font-mono)",
                fontSize: 11.5,
                color: "var(--text-3)",
                marginTop: 12,
              }}
            >
              <span style={{ color: "var(--signal-text)" }}>✓</span>transcript{" "}
              <span style={{ color: "var(--signal-text)" }}>✓</span>OCR{" "}
              <span style={{ color: "var(--signal-text)" }}>✓</span>events{" "}
              <span style={{ color: "var(--signal-text)" }}>✓</span>MCP
            </div>
          </div>
        </motion.div>
      </section>

      {/* ===================  MCP SECTION  =================== */}
      <section className="landing-section" style={{ paddingTop: 18, paddingBottom: 28 }}>
        <motion.div
          className="mcp-panel"
          variants={vMount}
          initial="hidden"
          whileInView="shown"
          viewport={{ once: true, margin: "-80px" }}
        >
          <div className="left">
            <div className="ttl-kicker">exposed over MCP</div>
            <h3>The link is an API</h3>
            <p className="lede">
              Claude or any agent reasons over a clip natively — and a per-clip{" "}
              <span className="mono" style={{ fontSize: 13, color: "var(--text)" }}>.json</span> sidecar covers everything else.
            </p>
            <div className="mcp-tools">
              {MCP_TOOLS.map((t) => (
                <div className="mcp-tool" key={t.fn}>
                  <span className="fn">{t.fn}</span>
                  <span className="d">{t.desc}</span>
                </div>
              ))}
            </div>
          </div>
          <pre className="mcp-code">
            <span className="c3">$ agent query</span>
{"\n"}
            <span className="name">&gt; query_clip(</span>
{"\n"}
            <span className="name">&gt;&nbsp;&nbsp;</span>
            <span className="sig">"clipxd.com/c/8fa2e1"</span>
            <span className="name">,</span>
{"\n"}
            <span className="name">&gt;&nbsp;&nbsp;</span>
            <span className="sig">"what error showed up and what was the</span>
{"\n"}
            <span className="name">&gt;&nbsp;&nbsp;&nbsp;</span>
            <span className="sig">user doing right before?"</span>
            <span className="name">)</span>
{"\n"}
            <span className="c3">↳ resolving index … (0 frames fetched)</span>
{"\n\n"}
            <span className="name">{"{"}</span>
{"\n"}
            <span>&nbsp;&nbsp;</span>
            <span className="c3">"answer"</span>
            <span className="name">: </span>
            <span className="sig">"500 at 0:41 — 'card_declined' in a red</span>
{"\n"}
            <span>&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;</span>
            <span className="sig">banner. User clicked Pay $89.00 at 0:40."</span>
            <span className="name">,</span>
{"\n"}
            <span>&nbsp;&nbsp;</span>
            <span className="c3">"grounds"</span>
            <span className="name">: [</span>
            <span className="sig">"ocr@0:41"</span>
            <span className="name">,</span>
            <span className="sig">"net@0:41"</span>
            <span className="name">,</span>
{"\n"}
            <span>&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;</span>
            <span className="sig">"event:click@0:40"</span>
            <span className="name">],</span>
{"\n"}
            <span>&nbsp;&nbsp;</span>
            <span className="c3">"watched_video"</span>
            <span className="name">: </span>
            <span className="warn">false</span>
{"\n"}
            <span className="name">{"}"}</span>
          </pre>
        </motion.div>
      </section>

      {/* ===================  COMPARISON TABLE  =================== */}
      <section className="landing-section" style={{ paddingTop: 38 }}>
        <motion.div
          variants={vMount}
          initial="hidden"
          whileInView="shown"
          viewport={{ once: true, margin: "-80px" }}
        >
          <div className="section-kicker">vs the field</div>
          <h2 className="section-title" style={{ fontWeight: 500, marginBottom: 24 }}>
            The only one where the index <span style={{ fontStyle: "italic", color: "var(--signal-text)" }}>is</span> the product.
          </h2>
          <div className="compare">
            <div className="compare-row head">
              <div />
              <div>Local-first</div>
              <div>Agent index</div>
              <div>On-device codec</div>
              <div>Redaction</div>
            </div>
            {COMPETITORS.map((row) => (
              <div
                className="compare-row body"
                key={row.name}
                style={{ background: row.bg }}
              >
                <div style={{ color: row.nameColor ?? "var(--text)" }}>
                  {row.name}
                  <span className="note">{row.note}</span>
                </div>
                {row.cells.map((c, i) => (
                  <div
                    key={i}
                    style={{ color: c === "✓" ? "var(--signal-text)" : "var(--text-3)" }}
                  >
                    {c}
                  </div>
                ))}
              </div>
            ))}
          </div>
        </motion.div>
      </section>

      {/* ===================  CTA + FOOTER  =================== */}
      <section className="landing-section" style={{ paddingTop: 28, paddingBottom: 84 }}>
        <motion.div
          className="final-cta"
          variants={vMount}
          initial="hidden"
          whileInView="shown"
          viewport={{ once: true, margin: "-80px" }}
        >
          <div className="grid-bg" />
          <div style={{ position: "relative" }}>
            <h2>
              Record once.{" "}
              <span className="badge-soda">Watch&nbsp;it</span> or{" "}
              <span className="badge-signal">read&nbsp;it</span>
            </h2>
            <p className="lede">
              Local-first and free to start. Open-core, Apache-2.0. Your pixels never leave your machine.
            </p>
            <div className="row-buttons">
              <button
                className="btn-sodium"
                onClick={onOpenApp}
                style={{ borderRadius: 14, fontSize: 15, padding: "13px 24px" }}
              >
                Open the app
              </button>
              <button
                className="btn"
                onClick={() => window.open("https://github.com/rohansx/clipxd", "_blank", "noopener,noreferrer")}
                style={{ borderRadius: 13, fontSize: 15, padding: "13px 22px" }}
              >
                Read the docs
              </button>
              {onLogin && (
                <button className="btn-ghost" onClick={onLogin} style={{ fontSize: 14 }}>
                  Log in
                </button>
              )}
            </div>
          </div>
        </motion.div>

        <Footer />
      </section>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*                       sub-components                                 */
/* ------------------------------------------------------------------ */

function ThemePill({ dark, onClick }: { dark: boolean; onClick: () => void }) {
  // Two glass cells — one shows the chosen theme in sodium, the other in signal.
  // Clicking either toggles; the highlight follows the current state.
  return (
    <button
      className="theme-pill"
      onClick={onClick}
      title="Light studio ⟷ night studio"
      aria-label={dark ? "Switch to light studio theme" : "Switch to night studio theme"}
      aria-pressed={dark}
    >
      <span
        className={"theme-pill-cell" + (!dark ? " on-sodium" : "")}
        style={{ color: !dark ? "var(--on-accent)" : "var(--text-3)" }}
        aria-hidden
      >
        <svg width="17" height="17" viewBox="0 0 24 24" fill="none">
          <defs>
            <radialGradient id="theme-sun" cx="0.35" cy="0.3" r="0.75">
              <stop offset="0" stopColor="#FFF4CC" />
              <stop offset="0.5" stopColor="#FFC75A" />
              <stop offset="1" stopColor="#FF9036" />
            </radialGradient>
          </defs>
          <g
            stroke={!dark ? "#FFFFFF" : "#FF9036"}
            strokeWidth="2.1"
            strokeLinecap="round"
          >
            <line x1="12" y1="1.8" x2="12" y2="4.4" />
            <line x1="12" y1="19.6" x2="12" y2="22.2" />
            <line x1="1.8" y1="12" x2="4.4" y2="12" />
            <line x1="19.6" y1="12" x2="22.2" y2="12" />
            <line x1="4.8" y1="4.8" x2="6.6" y2="6.6" />
            <line x1="17.4" y1="17.4" x2="19.2" y2="19.2" />
            <line x1="19.2" y1="4.8" x2="17.4" y2="6.6" />
            <line x1="6.6" y1="17.4" x2="4.8" y2="19.2" />
          </g>
          <circle cx="12" cy="12" r="5.4" fill="url(#theme-sun)" />
          <ellipse cx="10.2" cy="10.1" rx="1.7" ry="1.1" fill="#fff" opacity="0.65" transform="rotate(-30 10.2 10.1)" />
        </svg>
      </span>
      <span
        className={"theme-pill-cell" + (dark ? " on-signal" : "")}
        style={{ color: dark ? "var(--on-accent)" : "var(--text-3)" }}
        aria-hidden
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none">
          <defs>
            <linearGradient id="theme-moon" x1="0.1" y1="0" x2="0.9" y2="1">
              <stop offset="0" stopColor="#F3EEFF" />
              <stop offset="1" stopColor="#B5A4E6" />
            </linearGradient>
          </defs>
          <path d="M21 14.3 A8.4 8.4 0 1 1 11.4 3.4 A6.5 6.5 0 0 0 21 14.3 Z" fill="url(#theme-moon)" />
          <ellipse cx="9.6" cy="8.4" rx="1.5" ry="1" fill="#fff" opacity="0.5" transform="rotate(-30 9.6 8.4)" />
          <circle cx="16.5" cy="5.5" r="0.9" fill={dark ? "#FFFFFF" : "#B5A4E6"} />
        </svg>
      </span>
    </button>
  );
}

/** The watch/parse wipe — a single composite surface with two stacked layers
 *  and a draggable seam. Memoised inputs keep paint minimal during drags. */
function Wipe({
  revealClip,
  seamLeft,
  onPointerDown,
}: {
  revealClip: string;
  seamLeft: string; // pre-rendered "12.3%" string from parent state
  onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => void;
}) {
  // Boxes are absolute-positioned on the read overlay; coords are mirrored
  // from the design (16:9.5 viewport + centered card).  Labels live INSIDE
  // the box (left:8px) instead of at left:-1px so they stay readable when
  // the box straddles the seam and gets clipped by `revealClip`.
  const boxes = [
    { id: "url",    top: "13%", left: "16%", w: "30%", h: "10%", label: 'ocr · url',          color: "var(--signal)", ink: "var(--on-accent)", danger: false },
    { id: "title",  top: "25%", left: "18%", w: "26%", h: "9%",  label: 'ocr · "Payment"',    color: "var(--signal)", ink: "var(--on-accent)", danger: false },
    { id: "card",   top: "38%", left: "18%", w: "64%", h: "11%", label: 'ocr · card · pii',   color: "var(--signal)", ink: "var(--on-accent)", danger: false },
    { id: "click",  top: "62%", left: "18%", w: "64%", h: "11%", label: 'event · click',      color: "var(--sodium)", ink: "var(--on-accent)", danger: false },
    { id: "err",    top: "78%", left: "18%", w: "64%", h: "13%", label: 'ocr+net · 500',      color: "var(--danger)", ink: "#fff",            danger: true  },
  ];

  // The seam position is driven by parent state; we just need its % to render
  // the knob left-position. The parent owns drag handlers.
  return (
    <div className="wipe" onPointerDown={onPointerDown}>
      {/* base: the produced video (watch) */}
      <div style={{ position: "absolute", inset: 0 }}>
        <div className="wipe-grid" />
        <div className="wipe-hud tl">
          <span className="pill">
            <span
              className="dot sodium"
              style={{ animation: "recPulse 2s var(--ease-clip) infinite" }}
            />
            REC · 00:41 · 1080p
          </span>
          <span className="pill sodium">⌖ cinematic auto-zoom 2.4×</span>
        </div>
        <div className="wipe-stage">
          <MockCheckout />
        </div>
        <span className="wipe-corner" style={{ left: 14, bottom: 12, color: "var(--text-3)" }}>
          the pixels — what a human watches
        </span>
      </div>

      {/* read overlay: parsed, clipped to right of seam */}
      <div style={{ position: "absolute", inset: 0, clipPath: revealClip }}>
        <div style={{ position: "absolute", inset: 0, background: "var(--panel-2)" }} />
        <div className="wipe-grid" style={{ opacity: 0.6 }} />
        <div className="wipe-read-bg" />
        <div className="wipe-read-scan" />

        <div className="wipe-stage">
          <ParsedBoxes />
        </div>

        <div className="wipe-hud tr">
          <div className="wipe-index-card">
            <span className="head">INDEX · 5 TRACKS</span>
            <span><span style={{ color: "var(--signal-text)" }}>●</span> transcript · whisper.cpp</span>
            <span><span style={{ color: "var(--signal-text)" }}>●</span> on-screen text · ocr</span>
            <span><span style={{ color: "var(--sodium-text)" }}>●</span> events · click / key / scroll</span>
            <span><span style={{ color: "var(--grape)" }}>●</span> network · DOM · a11y</span>
            <span><span style={{ color: "var(--text-3)" }}>●</span> summary · chapters</span>
          </div>
          <span className="pill" style={{ color: "var(--danger)" }}>
            <span className="dot" style={{ background: "var(--danger)" }} />
            CloakPipe · 1 secret redacted
          </span>
        </div>
        <span className="wipe-corner br">~340 tokens · queryable over MCP</span>

        {/* parse boxes */}
        {boxes.map((b) => (
          <div
            key={b.id}
            className={"wipe-box" + (b.color.includes("danger") ? " danger" : b.color.includes("--sodium") ? " sodium" : "")}
            style={{
              top: b.top,
              left: b.left,
              width: b.w,
              height: b.h,
              borderColor: b.color,
            }}
          >
            <span
              className="wipe-box-label"
              style={{
                background: b.color,
                color: b.ink,
              }}
            >
              {b.label}
            </span>
          </div>
        ))}
      </div>

      {/* seam */}
      <div className="wipe-seam" style={{ left: seamLeft }} />
      <div className="wipe-seam-knob lglass" style={{ left: seamLeft }} aria-hidden>
        ⟺
      </div>
      <div
        className="wipe-seam-lens"
        style={{ left: seamLeft }}
        aria-hidden
      />
    </div>
  );
}

function MockCheckout() {
  return (
    <div className="wipe-card">
      <div className="wipe-card-bar">
        <i style={{ background: "#ec6a5e" }} />
        <i style={{ background: "#f4be4f" }} />
        <i style={{ background: "#61c454" }} />
        <span className="wipe-card-url">shop.acme.test/checkout</span>
      </div>
      <div className="wipe-card-body">
        <div className="wipe-card-title">Payment</div>
        <div className="wipe-card-input">4242 4242 4242 4242</div>
        <div className="wipe-card-row">
          <div />
          <div />
        </div>
        <div className="wipe-card-pay">Pay $89.00</div>
        <div className="wipe-card-err">
          <i />Payment failed: card_declined
        </div>
      </div>
    </div>
  );
}

function ParsedBoxes() {
  /* A transparent card mock — boxes are drawn over real text by `Wipe`. */
  return (
    <div style={{ width: "78%", maxWidth: 560 }}>
      <div style={{ padding: "18px 20px" }}>
        <div style={{ position: "relative", display: "inline-block", marginBottom: 14 }}>
          <div style={{ fontSize: 14, fontWeight: 700, color: "transparent" }}>Payment</div>
        </div>
        <div style={{ position: "relative", height: 34, marginBottom: 16 }} />
        <div style={{ position: "relative", height: 40, marginBottom: 14 }} />
        <div style={{ position: "relative", height: 40 }} />
      </div>
    </div>
  );
}

function TwoBody() {
  return (
    <div className="two-body">
      <div className="half watch-half">
        <div className="half-tag" style={{ color: "var(--sodium-text)" }}>
          <span className="led" style={{ background: "var(--sodium)" }} />
          WATCH · for a person
        </div>
        <div
          style={{
            fontFamily: "var(--font-display)",
            fontSize: 25,
            fontWeight: 500,
            letterSpacing: "-.01em",
            lineHeight: 1.2,
          }}
        >
          Checkout 500 — the card declines on pay
        </div>
        <p style={{ marginTop: 14, fontSize: 14.5, color: "var(--text-2)", lineHeight: 1.6 }}>
          Cinematic auto-zoom follows the cursor into the form, settles on the Pay button, and holds as the
          red banner appears. Beautiful, optical, human.
        </p>
        <div
          style={{
            marginTop: 18,
            display: "flex",
            gap: 14,
            fontSize: 13,
            color: "var(--text-3)",
          }}
        >
          <span style={{ fontFamily: "var(--font-display)", fontStyle: "italic" }}>serif display</span>·
          <span style={{ fontFamily: "var(--font-display)", fontStyle: "italic" }}>warm sodium</span>·
          <span style={{ fontFamily: "var(--font-display)", fontStyle: "italic" }}>cinematic ease</span>
        </div>
      </div>
      <div className="half read-half">
        <div className="half-tag" style={{ color: "var(--signal-text)" }}>
          <span className="led" style={{ background: "var(--signal)" }} />
          READ · for an agent
        </div>
        <div
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: 12.5,
            lineHeight: 1.85,
            color: "var(--text-2)",
          }}
        >
          <span style={{ color: "var(--text-3)" }}>0:40</span> event:click <span style={{ color: "var(--text)" }}>"Pay $89.00"</span>
          <br />
          <span style={{ color: "var(--text-3)" }}>0:41</span> net        <span style={{ color: "var(--text)" }}>POST /charge → 500</span>
          <br />
          <span style={{ color: "var(--text-3)" }}>0:41</span> ocr        <span style={{ color: "var(--danger)" }}>"card_declined"</span>
          <br />
          <span style={{ color: "var(--text-3)" }}>0:42</span> transcript <span style={{ color: "var(--text)" }}>"it just fails…"</span>
        </div>
        <div
          style={{
            marginTop: 18,
            display: "flex",
            gap: 14,
            fontFamily: "var(--font-mono)",
            fontSize: 11.5,
            color: "var(--text-3)",
          }}
        >
          <span>mono utility</span>·<span>cool signal</span>·<span>scan reveal</span>
        </div>
      </div>
    </div>
  );
}

function Footer() {
  const col = (heading: string, items: string[]) => (
    <div className="landing-footer-col">
      <h4>{heading}</h4>
      <ul>
        {items.map((i) => (
          <li key={i}>{i}</li>
        ))}
      </ul>
    </div>
  );
  return (
    <div className="landing-footer">
      <div className="landing-footer-grid">
        <div>
          <span
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: 6,
              fontFamily: "var(--font-display)",
              fontWeight: 700,
              fontSize: 19,
            }}
          >
            Clip
            <span
              style={{
                display: "inline-flex",
                background: "var(--signal)",
                color: "var(--on-accent)",
                fontSize: 13,
                fontWeight: 700,
                padding: "2px 6px 3px",
                borderRadius: 9,
                transform: "rotate(-5deg)",
                boxShadow: "var(--clay-sm)",
                marginLeft: 2,
              }}
            >
              XD
            </span>
          </span>
          <p className="landing-footer-tag">
            Record once. Humans watch it. Agents read it. Local-first, open-core.
          </p>
        </div>
        {col("Product", ["Recorder", "Agent index", "Import from URL", "MCP server"])}
        {col("Resources", ["Documentation", "GitHub", "Changelog"])}
        {col("Company", ["About", "Privacy", "Security"])}
      </div>
      <div className="landing-footer-meta">
        <span>© 2026 ClipXD · clip + index</span>
        <span>engine: veyo · CloakPipe redaction · MCP native</span>
      </div>
    </div>
  );
}
