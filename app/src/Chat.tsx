import { AnimatePresence, motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import { queryClip, searchClip } from "./api";
import type { ClipSummary } from "./types";
import { usePrefersReducedMotion } from "./motion";

interface ChatProps {
  clips: ClipSummary[] | null;
  onOpen: (id: string) => void;
}

interface Msg {
  who: "user" | "agent";
  text: string;
  cites?: { id: string; label: string }[];
  thinking?: boolean;
}

export function Chat({ clips, onOpen }: ChatProps) {
  const reduced = usePrefersReducedMotion();
  const [thread, setThread] = useState<Msg[]>([
    {
      who: "agent",
      text:
        "Ask anything across your library — I search every clip's index (transcript, on-screen text, captions) and answer from the best match. No video is fetched.",
    },
  ]);
  const [q, setQ] = useState("");
  const [busy, setBusy] = useState(false);
  const endRef = useRef<HTMLDivElement>(null);

  // keep the newest message in view
  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: reduced ? "auto" : "smooth", block: "end" });
  }, [thread, reduced]);

  const ask = async () => {
    const question = q.trim();
    if (!question || busy) return;
    const list = clips ?? [];
    setThread((t) => [...t, { who: "user", text: question }, { who: "agent", text: "searching the library…", thinking: true }]);
    setQ("");
    setBusy(true);

    try {
      const scored = await Promise.all(
        list.map(async (c) => {
          const hits = await searchClip(c.id, question);
          return { c, score: hits.reduce((s, h) => s + h.score, 0), n: hits.length };
        }),
      );
      const ranked = scored.filter((s) => s.score > 0).sort((a, b) => b.score - a.score).slice(0, 2);

      let reply: Msg;
      if (!ranked.length) {
        reply = { who: "agent", text: "No clip in your library matches that. Try different words, or import/record the relevant clip first." };
      } else {
        const answers = await Promise.all(
          ranked.map(async (r) => ({ r, a: await queryClip(r.c.id, question) })),
        );
        const text = answers
          .map(({ r, a }) => `In “${r.c.metadata.title || r.c.id}”: ${a.text}`)
          .join("\n\n");
        reply = {
          who: "agent",
          text,
          cites: ranked.map((r) => ({ id: r.c.id, label: `clip:${r.c.id.replace(/^clp_/, "")}` })),
        };
      }
      setThread((t) => [...t.filter((m) => !m.thinking), reply]);
    } catch {
      setThread((t) => [...t.filter((m) => !m.thinking), { who: "agent", text: "Something went wrong querying the library." }]);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="chat">
      <div className="chat-head">
        <span className="mk">◈</span>
        <div>
          <div className="ttl">Ask an agent</div>
          <div className="sub">querying the whole library over HTTP · no video fetched</div>
        </div>
      </div>
      <div className="chat-thread">
        <AnimatePresence initial={false}>
          {thread.map((m, i) => (
            <motion.div
              key={i}
              className={"msg " + m.who}
              initial={reduced ? false : { opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0, transition: { duration: 0.28, ease: [0.22, 1, 0.36, 1] } }}
              exit={{ opacity: 0, transition: { duration: 0.16 } }}
            >
              <div className="bubble" style={{ whiteSpace: "pre-wrap" }}>
                {m.thinking ? (
                  <span>
                    <span className="spin" /> {m.text}
                  </span>
                ) : (
                  m.text
                )}
              </div>
              {m.cites && m.cites.length > 0 && (
                <div className="cites">
                  <span className="lead">grounded in:</span>
                  {m.cites.map((c) => (
                    <button key={c.id} className="cite" onClick={() => onOpen(c.id)}>
                      {c.label}
                    </button>
                  ))}
                  <span className="lead" style={{ marginLeft: 4 }}>
                    watched_video: <span style={{ color: "var(--danger)" }}>false</span>
                  </span>
                </div>
              )}
            </motion.div>
          ))}
        </AnimatePresence>
        <div ref={endRef} />
      </div>
      <div className="chat-input">
        <div className="row">
          <input
            className="input"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && ask()}
            placeholder="Ask across every clip in your library…"
          />
          <button className="btn-signal btn-pill" onClick={ask} disabled={busy} style={{ padding: "0 22px" }}>
            {busy ? <span className="spin" /> : "Ask"}
          </button>
        </div>
      </div>
    </div>
  );
}
