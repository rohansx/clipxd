// Sample clip data mirroring the Rust `clipxd-index` schema, so the UI renders the real
// product story (record → auto-zoom → ask the clip) before the capture backend lands.
// Replace with live data from `clipxd-mcp` / the JSON sidecar once `clipxd-web` is wired.

export type ZoomEpisode = { start: number; end: number; label: string };
export type IndexEvent = { t: number; kind: string; text: string };
export type ClipQA = { q: string; a: string; cites: number[] };
export type Caption = { start: number; end: number; text: string };

export type Clip = {
  id: string;
  title: string;
  source: "screen" | "browser" | "import";
  duration: number;
  resolution: [number, number];
  episodes: ZoomEpisode[];
  events: IndexEvent[];
  onScreenText: { t: number; text: string }[];
  transcript: Caption[];
  qa: ClipQA[];
};

export const clip: Clip = {
  id: "clp_4ebc340b",
  title: "Checkout flow — 500 on submit",
  source: "browser",
  duration: 16,
  resolution: [1280, 720],
  episodes: [
    { start: 6.6, end: 9.0, label: 'auto-zoom → "Place order"' },
    { start: 8.6, end: 12.0, label: "auto-zoom → error toast" },
  ],
  events: [
    { t: 7.0, kind: "click", text: 'clicked "Place order"' },
    { t: 9.0, kind: "network", text: "POST /api/checkout → 500" },
    { t: 9.2, kind: "console_error", text: "Checkout failed: Payment failed (500)" },
  ],
  onScreenText: [
    { t: 0.2, text: "Checkout — app.example.com" },
    { t: 9.0, text: "ERROR: Payment failed (500)" },
  ],
  transcript: [
    { start: 6.2, end: 8.6, text: "Okay, let me place this order." },
    { start: 8.8, end: 12.0, text: "…and it failed with a payment error, a 500." },
  ],
  qa: [
    {
      q: "what error showed up and what was the user doing right before it",
      a: 'At 9.0s the on-screen text shows "ERROR: Payment failed (500)". Just before, at 7.0s, the user clicked "Place order" → POST /api/checkout returned 500.',
      cites: [9.0, 7.0],
    },
    {
      q: "summarize this clip",
      a: "A user submits a checkout; the POST /api/checkout request returns 500 and a 'Payment failed' error toast appears.",
      cites: [7.0, 9.0],
    },
  ],
};

export const fmt = (t: number) => `${Math.floor(t / 60)}:${String(Math.floor(t % 60)).padStart(2, "0")}`;
