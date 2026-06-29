import { useEffect, useRef, useState } from "react";
import { Brand } from "./Brand";
import type { Theme } from "./App";

interface LandingProps {
  theme: Theme;
  toggleTheme: () => void;
  onOpenApp: () => void;
}

const PIPELINE = [
  { step: "01 · capture", title: "Thin recorder", desc: "Pixels + audio + cursor/click/key. Rust.", kicker: "var(--sodium-text)" },
  { step: "02 · gate", title: "veyo-core", desc: "Salience gate, CPU. Emits a delta only when the scene changes.", kicker: "var(--signal-text)" },
  { step: "03 · enrich", title: "veyo-enrich", desc: "Transcript · caption salient frames · OCR · structure events.", kicker: "var(--signal-text)" },
  { step: "04 · clean", title: "CloakPipe", desc: "Strip PII / secrets from frames + transcript.", kicker: "var(--signal-text)" },
  { step: "05 · store", title: "The artifact", desc: "Video file + structured index, side by side.", kicker: "var(--text-3)" },
  { step: "06 · serve", title: "MCP + JSON", desc: "The link an agent queries. The moat.", kicker: "var(--signal-text)" },
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
  { tag: "evt", title: "Event track", desc: "Cursor, clicks, keys, scroll. In browser mode: DOM, a11y, console, network.", sample: "0:40  click \"Pay $89.00\"\n0:41  POST /charge → 500", span: "span 2" },
  { tag: "sum", title: "Summary + metadata", desc: "Chapters, duration, app focus, redaction manifest. Derived, not source.", sample: "chapters: 4 · pii: 2 redacted\nfocus: shop.acme.test", span: "span 2" },
];

export function Landing({ theme, toggleTheme, onOpenApp }: LandingProps) {
  const [wipe, setWipe] = useState(50);
  const [autoSweep, setAutoSweep] = useState(true);
  const dirRef = useRef(1);

  useEffect(() => {
    const h = window.setInterval(() => {
      if (!autoSweep) return;
      setWipe((w) => {
        let n = w + dirRef.current * 0.55;
        if (n >= 82) {
          n = 82;
          dirRef.current = -1;
        }
        if (n <= 16) {
          n = 16;
          dirRef.current = 1;
        }
        return n;
      });
    }, 26);
    return () => window.clearInterval(h);
  }, [autoSweep]);

  const setFromX = (clientX: number, el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    const pct = Math.max(4, Math.min(96, ((clientX - r.left) / r.width) * 100));
    setWipe(pct);
    setAutoSweep(false);
  };
  const startDrag = (e: React.PointerEvent) => {
    const frame = e.currentTarget as HTMLElement;
    const move = (ev: PointerEvent) => setFromX(ev.clientX, frame);
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    setFromX(e.clientX, frame);
  };

  const dark = theme === "dark";

  return (
    <div className="landing">
      <nav className="nav">
        <div className="nav-inner">
          <Brand />
          <span className="pill">
            <span className="dot sodium" style={{ animation: "recPulse 2s var(--ease-clip) infinite" }} />
            rec_ready
          </span>
          <div className="nav-links">
            <span className="nav-link">
              <span style={{ color: "var(--sodium-text)" }}>◆</span> product
            </span>
            <span className="nav-link">
              <span style={{ color: "var(--signal-text)" }}>▶</span> how_it_reads
            </span>
            <span className="nav-link" onClick={onOpenApp}>
              <span style={{ color: "var(--text-3)" }}>↗</span> cloud
            </span>
          </div>
          <button className="theme-toggle" onClick={toggleTheme} title="light ⟷ night studio">
            <span style={{ background: !dark ? "var(--sodium)" : "transparent", color: !dark ? "var(--on-accent)" : "var(--text-3)" }}>[+]</span>
            <span style={{ background: dark ? "var(--signal)" : "transparent", color: dark ? "var(--on-accent)" : "var(--text-3)" }}>[-]</span>
          </button>
          <button className="btn-signal" onClick={onOpenApp} style={{ borderRadius: 0 }}>
            Open app →
          </button>
        </div>
      </nav>

      {/* HERO */}
      <section className="hero">
        <div className="hero-wrap">
          <div style={{ maxWidth: 760 }}>
            <div className="kicker">
              <span className="led" />
              powered by the veyo visual-event codec
            </div>
            <h1 className="hero-title">
              A screen recording
              <br />
              an agent can <span className="g">read</span> —<br />
              without watching it.
            </h1>
            <p className="hero-sub">
              Record once. Humans get a beautiful video. Agents get a structured index —{" "}
              <b>transcript, on-screen text, clicks, network, the moments that matter</b> — queryable straight from the link.{" "}
              <span className="em">Drag the seam below</span> to see one second, both ways.
            </p>
          </div>

          <div>
            <div className="wipe-head">
              <span className="w">
                <span className="dot sodium" />
                WATCH — what a human sees
              </span>
              <span style={{ color: "var(--text-3)" }}>checkout-bug.clip · t=0:41 · drag to parse →</span>
              <span className="r">
                how the machine reads it — READ
                <span className="dot signal" />
              </span>
            </div>

            <div className="wipe" onPointerDown={startDrag}>
              {/* base: the produced video (watch) */}
              <div style={{ position: "absolute", inset: 0, background: "var(--cinema)", display: "grid", placeItems: "center", padding: "7%" }}>
                <MockCheckout />
              </div>
              {/* read overlay: parsed, clipped to right of the seam */}
              <div style={{ position: "absolute", inset: 0, clipPath: `inset(0 0 0 ${wipe}%)` }}>
                <div style={{ position: "absolute", inset: 0, background: "var(--cinema)" }} />
                <div style={{ position: "absolute", inset: 0, background: "var(--scrim)" }} />
                <div style={{ position: "absolute", inset: 0, backgroundImage: "repeating-linear-gradient(0deg,transparent 0 3px,rgba(91,83,230,.05) 3px 4px)" }} />
                <div style={{ position: "absolute", inset: 0, display: "grid", placeItems: "center", padding: "7%" }}>
                  <ParsedCheckout />
                </div>
                <span style={{ position: "absolute", right: 14, bottom: 12, fontFamily: "var(--font-mono)", fontSize: 11, color: "#fff", background: "rgba(40,30,90,.55)", padding: "3px 8px" }}>
                  ~340 tokens · 4 spans · the index
                </span>
              </div>
              <div className="seam-line" style={{ left: `${wipe}%` }}>
                <span className="seam-knob">⟺</span>
              </div>
            </div>

            <div className="cta-row">
              <button className="btn-signal" onClick={onOpenApp} style={{ borderRadius: 10, fontSize: 15, padding: "13px 22px" }}>
                <span className="dot" style={{ background: "var(--on-accent)" }} />
                Record a clip
              </button>
              <button className="btn" onClick={onOpenApp} style={{ borderRadius: 9, fontSize: 15, padding: "13px 22px" }}>
                Paste a Loom instead
              </button>
              <span className="pill" style={{ marginLeft: "auto", cursor: "pointer" }} onClick={() => setAutoSweep(true)}>
                seam: {autoSweep ? "auto" : "drag"} · click to auto-sweep
              </span>
            </div>

            <div className="stat-ledger">
              <div className="cell">
                <div className="big">0 px</div>
                <div className="sub">leave your device — veyo runs on-CPU</div>
              </div>
              <div className="cell">
                <div className="big">&lt;1%</div>
                <div className="sub">of frames enriched — salience-gated</div>
              </div>
              <div className="cell">
                <div className="big">1 link</div>
                <div className="sub">video + queryable index, same URL</div>
              </div>
            </div>
          </div>
        </div>
      </section>

      <div className="trust">
        <div className="trust-inner">
          <span>reads &amp; speaks</span>
          <b>MCP</b>·<b>Claude</b>·<b>Loom imports</b>·<b>Cap imports</b>·<b>whisper.cpp</b>·<b>PaddleOCR</b>·<b>Moondream2</b>·<b>local SQLite + FTS</b>
        </div>
      </div>

      {/* index breakdown */}
      <section className="section">
        <div style={{ maxWidth: 640 }}>
          <div className="section-kicker" style={{ color: "var(--signal-text)" }}>
            the moat
          </div>
          <h2 className="section-title">An agent should never need the pixels.</h2>
          <p className="section-lede">
            Every clip resolves from one URL into a structured object — five tracks, all timestamped, all text. If the agent ever has to watch the video, the index failed.
          </p>
        </div>
        <div className="index-grid">
          {INDEX_PARTS.map((p) => (
            <div key={p.tag} className="index-card" style={{ gridColumn: p.span }}>
              <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                <span className="tag">{p.tag}</span>
                <span style={{ fontFamily: "var(--font-display)", fontWeight: 500, fontSize: 17 }}>{p.title}</span>
              </div>
              <p style={{ fontSize: 13.5, color: "var(--text-2)", lineHeight: 1.5 }}>{p.desc}</p>
              <div className="sample">{p.sample}</div>
            </div>
          ))}
        </div>
      </section>

      {/* PIPELINE */}
      <section className="section">
        <div className="pipe-card">
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-end", flexWrap: "wrap", gap: 14, marginBottom: 30 }}>
            <div>
              <div className="section-kicker" style={{ marginBottom: 10 }}>architecture — thin recorder, fat index</div>
              <h2 className="section-title" style={{ fontSize: "clamp(24px,3vw,36px)" }}>How a recording becomes queryable</h2>
            </div>
            <div className="mono" style={{ fontSize: 12, color: "var(--signal-text)", border: "1px solid color-mix(in oklab,var(--signal) 35%,transparent)", padding: "7px 11px" }}>
              no imagery leaves the device →
            </div>
          </div>
          <div className="pipe-row">
            {PIPELINE.map((n, i) => (
              <div className="pipe-node-wrap" key={n.step}>
                <div className={"pipe-node" + (i === PIPELINE.length - 1 ? " last" : "")}>
                  <span className="step" style={{ color: n.kicker }}>{n.step}</span>
                  <span className="ttl">{n.title}</span>
                  <span className="desc">{n.desc}</span>
                </div>
                {i < PIPELINE.length - 1 && <span className="pipe-arrow">→</span>}
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* MCP */}
      <section className="section" style={{ paddingTop: 8 }}>
        <div className="mcp-panel">
          <div className="mcp-left">
            <div className="section-kicker" style={{ color: "var(--signal-text)", marginBottom: 12 }}>exposed over MCP</div>
            <h3 style={{ fontFamily: "var(--font-display)", fontSize: 25, letterSpacing: "-.01em", fontWeight: 500 }}>The link is an API</h3>
            <p style={{ marginTop: 10, fontSize: 15, color: "var(--text-2)", lineHeight: 1.55 }}>
              Claude or any agent reasons over a clip natively — and a per-clip <span className="mono" style={{ fontSize: 13, color: "var(--text)" }}>.json</span> sidecar covers everything else.
            </p>
            <div style={{ marginTop: 18 }}>
              {MCP_TOOLS.map((t) => (
                <div className="mcp-tool" key={t.fn}>
                  <span className="fn">{t.fn}</span>
                  <span className="d">{t.desc}</span>
                </div>
              ))}
            </div>
          </div>
          <div className="mcp-code">
{`$ agent query
> query_clip(
>   "clipxd.com/c/8fa2e1",
>   "what error showed up and what
>    was the user doing right before?")
↳ resolving index … (0 frames fetched)

{
  "answer": "500 at 0:41 — 'card_declined'
           in a red banner. User clicked
           Pay $89.00 at 0:40.",
  "grounds": ["ocr@0:41","net@0:41",
              "event:click@0:40"],
  "watched_video": false
}`}
          </div>
        </div>
      </section>

      {/* CTA */}
      <section className="section" style={{ paddingTop: 8 }}>
        <div style={{ position: "relative", overflow: "hidden", border: "1px solid var(--border)", background: "var(--panel)", padding: "54px 40px", textAlign: "center" }}>
          <div style={{ position: "absolute", inset: 0, backgroundImage: "linear-gradient(var(--grid) 1px,transparent 1px),linear-gradient(90deg,var(--grid) 1px,transparent 1px)", backgroundSize: "40px 40px", maskImage: "radial-gradient(60% 60% at 50% 50%,#000,transparent)" }} />
          <div style={{ position: "relative" }}>
            <h2 className="section-title" style={{ fontWeight: 500 }}>
              Record once.{" "}
              <span style={{ display: "inline-block", background: "var(--sodium)", color: "var(--on-accent)", padding: "0 .14em", transform: "rotate(-1.5deg)", boxShadow: "5px 5px 0 var(--border-2)" }}>Watch&nbsp;it</span> or{" "}
              <span style={{ display: "inline-block", background: "var(--signal)", color: "var(--on-accent)", padding: "0 .14em", transform: "rotate(1.5deg)", boxShadow: "5px 5px 0 var(--border-2)" }}>read&nbsp;it</span>
            </h2>
            <p style={{ marginTop: 14, fontSize: 17, color: "var(--text-2)", maxWidth: 480, margin: "14px auto 0" }}>
              Local-first and free to start. Open-core. Your pixels never leave your machine.
            </p>
            <div style={{ display: "flex", gap: 12, justifyContent: "center", marginTop: 26, flexWrap: "wrap" }}>
              <button className="btn-sodium" onClick={onOpenApp} style={{ borderRadius: 0, fontSize: 15, padding: "13px 24px" }}>
                Open the app
              </button>
              <button className="btn" style={{ borderRadius: 0, fontSize: 15, padding: "13px 24px" }}>
                Read the docs
              </button>
            </div>
          </div>
        </div>

        <div className="landing-footer">
          <div style={{ display: "flex", alignItems: "center", gap: 9 }}>
            <span style={{ fontFamily: "var(--font-display)", fontWeight: 600, fontSize: 17, color: "var(--text-2)" }}>clipxd</span>
            <span className="mono" style={{ fontSize: 16, letterSpacing: "-.06em" }}>
              <span style={{ color: "var(--sodium)" }}>▶</span>
              <span style={{ color: "var(--signal)" }}>]</span>
            </span>
            <span className="mono">· clip + index</span>
          </div>
          <div className="mono">engine: veyo · CloakPipe redaction · MCP native</div>
        </div>
      </section>
    </div>
  );
}

function MockCheckout() {
  return (
    <div className="mock-card">
      <div className="mock-bar">
        <i style={{ background: "#ec6a5e" }} />
        <i style={{ background: "#f4be4f" }} />
        <i style={{ background: "#61c454" }} />
        <span className="mock-url">shop.acme.test/checkout</span>
      </div>
      <div style={{ padding: "18px 20px", background: "#fff", color: "#222" }}>
        <div style={{ fontSize: 14, fontWeight: 700, color: "#111" }}>Payment</div>
        <div style={{ marginTop: 11, height: 34, border: "1px solid #ddd", borderRadius: 7, display: "flex", alignItems: "center", padding: "0 11px", fontFamily: "var(--font-mono)", fontSize: 12, color: "#777" }}>4242 4242 4242 4242</div>
        <div style={{ marginTop: 9, display: "flex", gap: 9 }}>
          <div style={{ flex: 1, height: 30, border: "1px solid #ddd", borderRadius: 7 }} />
          <div style={{ width: 84, height: 30, border: "1px solid #ddd", borderRadius: 7 }} />
        </div>
        <div style={{ marginTop: 12, background: "#1a1a1a", color: "#fff", borderRadius: 7, padding: 11, textAlign: "center", fontSize: 13, fontWeight: 700 }}>Pay $89.00</div>
        <div style={{ marginTop: 12, background: "#fdecec", border: "1px solid #f5b5b5", color: "#c0392b", borderRadius: 7, padding: "10px 12px", fontSize: 12.5, fontWeight: 600, display: "flex", alignItems: "center", gap: 8 }}>
          <span style={{ width: 6, height: 6, borderRadius: "50%", background: "#c0392b" }} />
          Payment failed: card_declined
        </div>
      </div>
    </div>
  );
}

function ParsedCheckout() {
  const box = (color: string): React.CSSProperties => ({ position: "absolute", inset: -2, border: `1.5px solid ${color}`, borderRadius: 4 });
  const tag = (color: string): React.CSSProperties => ({ position: "absolute", top: -9, left: -1, fontFamily: "var(--font-mono)", fontSize: 9, fontWeight: 600, color: "var(--on-accent)", background: color, padding: "2px 5px", whiteSpace: "nowrap" });
  return (
    <div style={{ width: "78%", maxWidth: 600 }}>
      <div style={{ padding: "18px 20px" }}>
        <div style={{ position: "relative", display: "inline-block", marginBottom: 14 }}>
          <div style={{ fontSize: 14, fontWeight: 700, color: "transparent" }}>Payment</div>
          <div style={box("var(--signal)")}>
            <span style={tag("var(--signal)")}>ocr · "Payment"</span>
          </div>
        </div>
        <div style={{ position: "relative", height: 34, marginBottom: 16 }}>
          <div style={{ ...box('var(--signal)'), animation: "boxIn .3s var(--ease-clip)" }}>
            <span style={tag("var(--signal)")}>ocr · "4242 4242 4242 4242" · pii?</span>
          </div>
        </div>
        <div style={{ position: "relative", height: 40, marginBottom: 14 }}>
          <div style={{ ...box('var(--sodium)'), animation: "boxIn .3s var(--ease-clip)" }}>
            <span style={tag("var(--sodium)")}>event · click "Pay $89.00" @0:40</span>
          </div>
        </div>
        <div style={{ position: "relative", height: 40 }}>
          <div style={{ ...box('var(--danger)'), animation: "boxIn .3s var(--ease-clip)" }}>
            <span style={{ ...tag("var(--danger)"), color: "#fff" }}>ocr+net · 500 "card_declined"</span>
          </div>
        </div>
      </div>
    </div>
  );
}
