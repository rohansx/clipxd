import { useState } from "react";

interface Section {
  id: string;
  label: string;
}

const SECTIONS: Section[] = [
  { id: "overview", label: "Overview" },
  { id: "recording", label: "Recording" },
  { id: "index", label: "The index" },
  { id: "watching", label: "Watching & reading" },
  { id: "asking", label: "Asking your clip" },
  { id: "mcp", label: "MCP — connect an agent" },
  { id: "sharing", label: "Sharing" },
  { id: "comments", label: "Comments" },
  { id: "docgen", label: "Clip → document" },
  { id: "cinematic", label: "Cinematic editor" },
  { id: "extension", label: "Browser extension" },
  { id: "byok", label: "Bring your own keys" },
  { id: "privacy", label: "Privacy — CloakPipe" },
];

export function Docs() {
  const [active, setActive] = useState("overview");

  const jump = (id: string) => {
    setActive(id);
    document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  return (
    <div className="view docs-view">
      <div className="view-head">
        <div>
          <h1 className="view-title">Docs</h1>
          <p className="view-sub">Every recording is agent-queryable — how it all works.</p>
        </div>
      </div>

      <div className="docs-layout">
        <nav className="docs-toc" aria-label="Sections">
          {SECTIONS.map((s) => (
            <button key={s.id} className={"docs-toc-item" + (active === s.id ? " on" : "")} onClick={() => jump(s.id)}>
              {s.label}
            </button>
          ))}
        </nav>

        <div className="docs-body">
          <DocSection id="overview" title="Overview">
            <p>
              clipxd records a screen or browser session and turns it into two things at once: a normal,
              watchable video, and a <b>structured index</b> — transcript, on-screen text, a captioned
              timeline of what happened, and (for browser recordings) the real clicks/console/network
              events. The video is for humans. The index is for agents. A link to a clip is both.
            </p>
            <p>
              Every clip gets a share URL the moment you hit record (the <i>instant-link</i> architecture) —
              the page is live and shareable while the recording is still running, and fills in as
              enrichment finishes in the background.
            </p>
          </DocSection>

          <DocSection id="recording" title="Recording">
            <h3>Screen mode</h3>
            <p>
              Captures your screen via the browser's native <code>getDisplayMedia</code>, with a short
              countdown before it starts. Because this is a browser security sandbox, Screen mode can only
              see cursor/clicks while the pointer is over the clipxd tab itself — a recording of another
              app or window has no interaction track from Screen mode alone (that's what Browser mode, via
              the extension, is for).
            </p>
            <p>Stop takes you straight to the clip's page — no waiting for the upload to "finish" first.</p>
            <h3>Browser mode (extension)</h3>
            <p>
              The <a href="#extension" onClick={(e) => { e.preventDefault(); jump("extension"); }}>browser extension</a> records
              a specific tab's video + audio <i>and</i> a structured trace of everything that happened in
              it — real clicks, typed input, console errors, network requests, and navigation — fused into
              one clip that's both watchable and fully queryable.
            </p>
          </DocSection>

          <DocSection id="index" title="The index">
            <p>Every clip's <code>index.json</code> is built from whichever of these streams are available:</p>
            <ul className="docs-list">
              <li><b>Transcript</b> — speech-to-text via <code>whisper.cpp</code>, timestamped.</li>
              <li><b>On-screen text</b> — OCR (PaddleOCR / oar-ocr, Rust-native ONNX) for screen recordings; read straight from the DOM for browser recordings (no OCR needed, more accurate).</li>
              <li><b>Visual timeline</b> — a captioned moment for each salient keyframe, from a vision-language model (Moondream2) describing what's on screen.</li>
              <li><b>Event track</b> — clicks, key presses, console errors, network requests, and navigation, for browser recordings.</li>
              <li><b>Search corpus</b> — all of the above flattened into one greppable block, rebuilt every time the index changes.</li>
            </ul>
            <p>
              Degenerate or repetitive model output (a caption stuck repeating one phrase) gets collapsed
              automatically before it ever reaches the index or the player.
            </p>
          </DocSection>

          <DocSection id="watching" title="Watching & reading">
            <p>
              The clip page has two panes. <b>Watch</b> is a single glass control bar over the video —
              play/pause, a seek track with the visual timeline's salient moments marked on it, elapsed
              time, mute, fullscreen — plus the manual cinematic editor. <b>Read</b> has five tabs:
            </p>
            <ul className="docs-list">
              <li><b>Moments</b> — every captioned keyframe with its real thumbnail, timestamp, and caption text; click any row to seek there.</li>
              <li><b>Transcript</b> — the speech track, click a line to jump to it.</li>
              <li><b>On-screen</b> — every OCR/DOM text span found, with error-looking lines highlighted.</li>
              <li><b>Events</b> — the click/key/network/navigation track (browser recordings).</li>
              <li><b>Summary</b> — the tl;dr and chapters, when the deep pass has run, plus the CloakPipe redaction note.</li>
            </ul>
          </DocSection>

          <DocSection id="asking" title="Asking your clip">
            <p>
              The <b>Ask agent</b> box on the clip page answers natural-language questions grounded in the
              clip's own index — "what error happened," "what did they click before it broke" — and cites
              the timestamps its answer came from. This is the same grounded search the MCP tools below
              expose to any external agent.
            </p>
          </DocSection>

          <DocSection id="mcp" title="MCP — connect an agent">
            <p>
              clipxd.com is itself a hosted <b>MCP server</b> (Streamable HTTP) at{" "}
              <code>https://clipxd.com/mcp</code> — one endpoint for your whole library, not one per clip.
              Add it to Claude Code, Claude Desktop, or any other MCP-speaking client and paste in a clip's
              share URL to let the agent query it directly.
            </p>
            <pre className="docs-code">{`claude mcp add clipxd https://clipxd.com/mcp --transport http`}</pre>
            <p>Five tools, each parameterized by a <code>clip_id</code> (from the clip's share URL, e.g. <code>clp_1efc6ad3</code>):</p>
            <table className="docs-table">
              <tbody>
                <tr><td><code>query_clip</code></td><td>clip_id, question</td><td>Grounded natural-language Q&amp;A about the clip.</td></tr>
                <tr><td><code>search_text</code></td><td>clip_id, query</td><td>Search across transcript + on-screen text + captions.</td></tr>
                <tr><td><code>get_frame_context</code></td><td>clip_id, t</td><td>What was on screen / said at a specific time.</td></tr>
                <tr><td><code>get_events</code></td><td>clip_id, start?, end?</td><td>The click/key/network/nav track in a time window.</td></tr>
                <tr><td><code>get_summary</code></td><td>clip_id</td><td>The tl;dr and chapters.</td></tr>
              </tbody>
            </table>
            <p>
              Prefer a raw, no-server file? <code>clipxd-mcp</code> is a local, single-clip, stdio-transport
              MCP server you run by hand against a downloaded <code>index.json</code> — same idea, offline.
            </p>
          </DocSection>

          <DocSection id="sharing" title="Sharing">
            <p>
              Every clip's link works the moment it exists — no "processing, check back later." Owned
              clips get a canonical <code>clipxd.com/u/&lt;you&gt;/clip/&lt;id&gt;</code> URL once you claim
              a username in Settings; the bare <code>/clip/&lt;id&gt;</code> form still works and redirects.
            </p>
            <p>
              The share page has one-click copy for the link itself, a GIF-embed (plays inline in email), an
              iframe embed, a direct agent.md/MCP link, a scan-to-phone QR code, and a running view count.
            </p>
          </DocSection>

          <DocSection id="comments" title="Comments">
            <p>
              Timestamped, Fathom-style chat lives on both the clip page and the public share page —
              anyone with the link can read the thread, and a logged-in viewer can post. Writing{" "}
              <code>@1:23</code> anywhere in a comment turns it into a click-to-seek link for that moment.
              An "@ this moment" button inserts the current playhead time for you.
            </p>
          </DocSection>

          <DocSection id="docgen" title="Clip → document">
            <p>
              Turn a clip straight into a real markdown document from its share page — no re-watching
              required, generated from the clip's own transcript/OCR/captions:
            </p>
            <ul className="docs-list">
              <li><b>PR description</b> (<code>/clip/:id/doc/pr-description</code>) — summary + test plan.</li>
              <li><b>SOP</b> (<code>/clip/:id/doc/sop</code>) — numbered repro/how-to steps.</li>
              <li><b>QA steps</b> (<code>/clip/:id/doc/qa-steps</code>) — a test checklist.</li>
            </ul>
          </DocSection>

          <DocSection id="cinematic" title="Cinematic editor">
            <p>
              Every clip's Watch pane has a manual cinematic editor: add zoom regions (overriding the
              automatic cursor-follow zoom), cut spans, ramp speed 2×, pick a background wallpaper, then
              render to a real MP4 or export a re-editable <code>.clipxd</code> project file. Auto-zoom
              already runs on every clip by default, following clicks/cursor dwell — the manual editor is
              for when you want to override that by hand.
            </p>
          </DocSection>

          <DocSection id="extension" title="Browser extension">
            <p>
              An MV3 Chrome extension (load unpacked from <code>extension/</code> for now) that records a
              specific tab's video + audio via <code>chrome.tabCapture</code>, plus the full interaction
              trace — clicks, input, console, network, navigation — that Screen mode structurally can't see
              in another tab. The two are fused into one clip server-side: watchable video, real event
              track, same clip.
            </p>
            <ul className="docs-list">
              <li>Optional <b>camera bubble</b> — your webcam composited into the corner, mic narration mixed with the tab's own audio.</li>
              <li>Optional <b>local captioning</b> — see Bring your own keys below.</li>
              <li>Falls back gracefully to a trace-only clip (no video) if tab capture isn't available.</li>
            </ul>
          </DocSection>

          <DocSection id="byok" title="Bring your own keys">
            <p>
              In Settings, you can use your own NVIDIA, Gemini, or Moondream API keys instead of the shared
              server ones — your usage lands on your own account and quota, not ours. A saved key is never
              sent back to the browser; the page only ever shows whether one is configured.
            </p>
            <ul className="docs-list">
              <li><b>NVIDIA key</b> — powers title / tl;dr / chapters generation (kimi-k2.6 → minimax-m2.7 → glm4.7 cascade).</li>
              <li><b>Gemini key</b> — fallback LLM backend if NVIDIA isn't configured or a call fails.</li>
              <li><b>Moondream key</b> — overrides the server's shared cloud-captioning key for your account's clips.</li>
              <li><b>Local captioning</b> — instead of any key at all, run captioning fully on your own device via WebGPU (in the browser extension) — nothing leaves your machine, zero cost to anyone.</li>
            </ul>
          </DocSection>

          <DocSection id="privacy" title="Privacy — CloakPipe">
            <p>
              Every index passes through CloakPipe before it's shared — a redaction pass that checks for
              secrets before the link goes live. The clip page's Summary tab always shows the result: either
              "redaction ran" with the policy used, or an explicit "no secrets detected before this index
              was shared."
            </p>
          </DocSection>
        </div>
      </div>
    </div>
  );
}

function DocSection({ id, title, children }: { id: string; title: string; children: React.ReactNode }) {
  return (
    <section id={id} className="docs-section">
      <h2>{title}</h2>
      {children}
    </section>
  );
}
