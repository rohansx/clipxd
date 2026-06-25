import { useEffect, useRef, useState } from "react";

const SAMPLE = `Paste your script here — clipxd scrolls it while you record, so you can read naturally and still look at your camera.

Hit "Start scroll" and adjust the speed to match your pace.

Everything you say still lands in the index: the transcript of your narration and the on-screen text (OCR) both become agent-queryable, so anyone — or any agent — can ask this recording questions afterward.`;

// A floating teleprompter the presenter reads while recording. Edit the script, then auto-
// scroll it at an adjustable speed. Purely a UI aid (it's on the clipxd page, read by you);
// it doesn't go into the recording unless you're capturing this tab.
export function Prompter({ onClose }: { onClose: () => void }) {
  const [text, setText] = useState(SAMPLE);
  const [scrolling, setScrolling] = useState(false);
  const [speed, setSpeed] = useState(40); // px/s
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!scrolling) return;
    let raf = 0;
    let last = 0;
    const tick = (ts: number) => {
      const box = boxRef.current;
      if (box && last) {
        box.scrollTop += (speed * (ts - last)) / 1000;
        if (box.scrollTop + box.clientHeight >= box.scrollHeight - 1) setScrolling(false);
      }
      last = ts;
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [scrolling, speed]);

  return (
    <div className="prompter">
      <div className="prompter-bar">
        <b>Teleprompter</b>
        <button onClick={() => { if (boxRef.current) boxRef.current.scrollTop = 0; setScrolling((s) => !s); }}>
          {scrolling ? "⏸ Pause" : "▶ Start scroll"}
        </button>
        <label className="pspeed">speed<input type="range" min={12} max={140} value={speed} onChange={(e) => setSpeed(+e.target.value)} /></label>
        <span className="tb-spacer" />
        <button onClick={onClose} title="Close prompter">✕</button>
      </div>
      {scrolling ? (
        <div className="prompter-scroll" ref={boxRef} onClick={() => setScrolling(false)}>
          {text.split("\n").map((l, i) => <p key={i}>{l || " "}</p>)}
        </div>
      ) : (
        <textarea className="prompter-edit" value={text} onChange={(e) => setText(e.target.value)} placeholder="Paste your script…" />
      )}
    </div>
  );
}
