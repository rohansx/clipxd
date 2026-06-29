import { useState } from "react";
import { fmt, type Index } from "./types";

type Tab = "transcript" | "ocr" | "events" | "summary";

interface ReadBodyProps {
  index: Index;
  t: number;
  seek: (t: number) => void;
}

export function ReadBody({ index, t, seek }: ReadBodyProps) {
  const [tab, setTab] = useState<Tab>(defaultTab(index));

  const tabs: { key: Tab; label: string; n: number }[] = [
    { key: "transcript", label: "Transcript", n: index.transcript.length },
    { key: "ocr", label: "On-screen", n: index.on_screen_text.length },
    { key: "events", label: "Events", n: index.event_track.length },
    { key: "summary", label: "Summary", n: index.summary.chapters?.length ?? 0 },
  ];

  return (
    <div className="read-body">
      <div className="read-head">
        <span className="lbl">INDEX</span>
        <span className="engines mono">veyo · whisper.cpp · PaddleOCR · Moondream2</span>
      </div>
      <div className="read-tabs">
        {tabs.map((tb) => (
          <button key={tb.key} className={"read-tab" + (tab === tb.key ? " on" : "")} onClick={() => setTab(tb.key)}>
            {tb.label}
            {tb.n > 0 ? ` ·${tb.n}` : ""}
          </button>
        ))}
      </div>
      <div className="read-scroll">
        {tab === "transcript" && <Transcript index={index} t={t} seek={seek} />}
        {tab === "ocr" && <OnScreen index={index} t={t} seek={seek} />}
        {tab === "events" && <Events index={index} seek={seek} />}
        {tab === "summary" && <SummaryTab index={index} />}
      </div>
    </div>
  );
}

function defaultTab(index: Index): Tab {
  if (index.transcript.length) return "transcript";
  if (index.on_screen_text.length) return "ocr";
  if (index.event_track.length) return "events";
  return "summary";
}

function Empty({ what }: { what: string }) {
  return <div className="empty" style={{ padding: 30 }}>{what}</div>;
}

function Transcript({ index, t, seek }: ReadBodyProps) {
  if (!index.transcript.length) return <Empty what="No transcript — this clip has no audio track." />;
  return (
    <>
      {index.transcript.map((s, i) => (
        <div
          key={i}
          className={"read-row clk"}
          style={{ animationDelay: `${i * 0.03}s`, background: t >= s.start && t <= s.end ? "var(--panel-2)" : undefined }}
          onClick={() => seek(s.start)}
        >
          <span className="t">{fmt(s.start)}</span>
          <div>
            {s.speaker && <span className="who">{s.speaker} </span>}
            <div className="x">{s.text}</div>
          </div>
        </div>
      ))}
    </>
  );
}

function OnScreen({ index, t, seek }: ReadBodyProps) {
  if (!index.on_screen_text.length) return <Empty what="No on-screen text detected." />;
  return (
    <>
      {index.on_screen_text.map((o, i) => {
        const danger = /error|fail|500|declin|denied/i.test(o.text);
        return (
          <div
            key={i}
            className="read-row clk"
            style={{ animationDelay: `${Math.min(i, 20) * 0.02}s`, background: Math.abs(o.start - t) < 0.6 ? "var(--panel-2)" : undefined }}
            onClick={() => seek(o.start)}
          >
            <span className="t">{fmt(o.start)}</span>
            <span className={"mono x" + (danger ? " danger" : "")}>{o.text}</span>
          </div>
        );
      })}
    </>
  );
}

function Events({ index, seek }: { index: Index; seek: (t: number) => void }) {
  if (!index.event_track.length)
    return <Empty what="No interaction events — import clips have no click/keystroke track. Record in Browser mode to capture them." />;
  return (
    <>
      {index.event_track.map((e, i) => (
        <div key={i} className="read-row clk" onClick={() => seek(e.t)}>
          <span className="t">{fmt(e.t)}</span>
          <span className="etype">{e.kind}</span>
          <span className="mono x">{e.text ?? e.kind}</span>
        </div>
      ))}
    </>
  );
}

function SummaryTab({ index }: { index: Index }) {
  return (
    <>
      <div className="summary-text">{index.summary.tldr || "No summary yet."}</div>
      {(index.summary.chapters?.length ?? 0) > 0 && (
        <div className="summary-points">
          {index.summary.chapters!.map((ch, i) => (
            <div key={i} className="pt">
              <span className="arr">→</span>
              <span>
                <b className="mono" style={{ color: "var(--signal-text)" }}>{fmt(ch.start)}</b> {ch.title}
              </span>
            </div>
          ))}
        </div>
      )}
      <div className="redaction-note">
        <span className="led-on" />
        CloakPipe: {index.redaction.ran ? `redaction ran (${index.redaction.policy})` : "no secrets detected before this index was shared."}
      </div>
    </>
  );
}
