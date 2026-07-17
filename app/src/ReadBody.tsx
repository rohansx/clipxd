import { useEffect, useState } from "react";
import { fmt, type Comment, type Index, type VisualMoment } from "./types";
import { fetchComments, frameUrl, postComment } from "./api";

type Tab = "moments" | "transcript" | "ocr" | "events" | "comments" | "summary";

interface ReadBodyProps {
  id: string;
  index: Index;
  t: number;
  seek: (t: number) => void;
}

export function ReadBody({ id, index, t, seek }: ReadBodyProps) {
  const [tab, setTab] = useState<Tab>(defaultTab(index));
  const [comments, setComments] = useState<Comment[]>([]);

  useEffect(() => {
    let live = true;
    fetchComments(id).then((c) => live && setComments(c));
    return () => {
      live = false;
    };
  }, [id]);

  const tabs: { key: Tab; label: string; n: number }[] = [
    { key: "moments", label: "Moments", n: index.visual_timeline.length },
    { key: "transcript", label: "Transcript", n: index.transcript.length },
    { key: "ocr", label: "On-screen", n: index.on_screen_text.length },
    { key: "events", label: "Events", n: index.event_track.length },
    { key: "comments", label: "Comments", n: comments.length },
    { key: "summary", label: "Summary", n: index.summary.chapters?.length ?? 0 },
  ];

  return (
    <div className="read-body">
      <div className="read-head">
        <span className="lbl">INDEX</span>
        <span className="engines mono">transcript · on-screen text · moments</span>
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
        {tab === "moments" && <Moments id={id} index={index} t={t} seek={seek} />}
        {tab === "transcript" && <Transcript id={id} index={index} t={t} seek={seek} />}
        {tab === "ocr" && <OnScreen id={id} index={index} t={t} seek={seek} />}
        {tab === "events" && <Events index={index} seek={seek} />}
        {tab === "comments" && <Comments id={id} t={t} seek={seek} comments={comments} setComments={setComments} />}
        {tab === "summary" && <SummaryTab index={index} />}
      </div>
    </div>
  );
}

function defaultTab(index: Index): Tab {
  // Lead with the most human-readable stream that actually has content: the captioned
  // moments (what each frame shows) first, then transcript, then raw OCR.
  if (index.visual_timeline.length) return "moments";
  if (index.transcript.length) return "transcript";
  if (index.on_screen_text.length) return "ocr";
  if (index.event_track.length) return "events";
  return "summary";
}

function Empty({ what }: { what: string }) {
  return <div className="empty" style={{ padding: 30 }}>{what}</div>;
}

/** Resolve a moment's `frame_ref` ("frames/00003.jpg") to a servable frame URL. The
 *  `/clip/:id/frames/:name` route takes the bare filename, so strip the `frames/` prefix. */
function momentFrameUrl(id: string, m: VisualMoment): string | null {
  if (!m.frame_ref) return null;
  const name = m.frame_ref.replace(/^frames\//, "");
  return frameUrl(id, name);
}

function Moments({ id, index, t, seek }: ReadBodyProps) {
  if (!index.visual_timeline.length)
    return <Empty what="No captioned moments yet — the vision model runs as the clip finishes indexing." />;
  return (
    <>
      {index.visual_timeline.map((m, i) => {
        const src = momentFrameUrl(id, m);
        const active = Math.abs(m.t - t) < 1.2;
        return (
          <button
            key={i}
            className={"moment-row" + (active ? " on" : "")}
            style={{ animationDelay: `${Math.min(i, 20) * 0.02}s` }}
            onClick={() => seek(m.t)}
            title={`Jump to ${fmt(m.t)}`}
          >
            {src ? (
              <img className="moment-thumb" src={src} alt="" loading="lazy" />
            ) : (
              <span className="moment-thumb moment-thumb-empty" aria-hidden />
            )}
            {/* Prefer the cleaned action label ("Introduces the elephants") over the raw vision
                caption; older clips (and any indexed without an LLM key) have no label. */}
            <div className="moment-text">
              <span className="moment-t mono">{fmt(m.t)}</span>
              <span className="moment-cap" title={m.label ? m.caption : undefined}>{m.label || m.caption}</span>
            </div>
          </button>
        );
      })}
    </>
  );
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

/** Render comment text, turning `@m:ss` / `@h:mm:ss` mentions into click-to-seek links —
 *  the "@ about that minute" behavior. Everything else renders as plain text. */
function renderCommentText(text: string, seek: (t: number) => void): React.ReactNode[] {
  const out: React.ReactNode[] = [];
  const re = /@(\d{1,2}:)?\d{1,2}:\d{2}/g; // @m:ss or @h:mm:ss
  let last = 0;
  let m: RegExpExecArray | null;
  let k = 0;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const stamp = m[0].slice(1); // drop the @
    const secs = stamp.split(":").reduce((acc, p) => acc * 60 + Number(p), 0);
    out.push(
      <button key={`m${k++}`} className="cmt-mention" onClick={() => seek(secs)} title={`Jump to ${stamp}`}>
        @{stamp}
      </button>,
    );
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

interface CommentsProps {
  id: string;
  t: number;
  seek: (t: number) => void;
  comments: Comment[];
  setComments: React.Dispatch<React.SetStateAction<Comment[]>>;
}

function Comments({ id, t, seek, comments, setComments }: CommentsProps) {
  const [text, setText] = useState("");
  const [posting, setPosting] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const sorted = [...comments].sort((a, b) => a.t - b.t);

  const submit = async () => {
    const body = text.trim();
    if (!body || posting) return;
    setPosting(true);
    setErr(null);
    try {
      const c = await postComment(id, t, body);
      setComments((cs) => [...cs, c]);
      setText("");
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Couldn't post");
    } finally {
      setPosting(false);
    }
  };

  return (
    <div className="cmt-wrap">
      <div className="cmt-list">
        {sorted.length === 0 ? (
          <Empty what="No comments yet — drop a note anchored to the moment you're watching." />
        ) : (
          sorted.map((c) => (
            <div key={c.id} className="cmt">
              <button className="cmt-t mono" onClick={() => seek(c.t)} title={`Jump to ${fmt(c.t)}`}>
                {fmt(c.t)}
              </button>
              <div className="cmt-body">
                <div className="cmt-author">{c.author}</div>
                <div className="cmt-text">{renderCommentText(c.text, seek)}</div>
              </div>
            </div>
          ))
        )}
      </div>
      <div className="cmt-compose">
        {err && <div className="cmt-err">{err}</div>}
        <textarea
          className="cmt-input"
          placeholder={`Comment on ${fmt(t)}…  (type @${fmt(t)} to link a moment)`}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) submit();
          }}
          rows={2}
        />
        <div className="cmt-actions">
          <button className="cmt-atbtn" type="button" onClick={() => setText((x) => `${x}${x && !x.endsWith(" ") ? " " : ""}@${fmt(t)} `)} title="Reference the current moment">
            @ this moment
          </button>
          <span style={{ flex: 1 }} />
          <button className="cmt-post" onClick={submit} disabled={posting || !text.trim()}>
            {posting ? "…" : `Comment on ${fmt(t)}`}
          </button>
        </div>
      </div>
    </div>
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
