# PR — design: port updated-3d-ClipXD.dc.html — puffy clay redesign + subtle framer-motion

## What this PR does

Ports the SPA's design language to match `docs/design/updated-3d-ClipXD.dc.html`
(warm pastel "playground" light, deep plum "night" dark, sodium/signal on
clay rounded surfaces). Subtle framer-motion everywhere — one ease, one
distance, one spring. Keeps every existing feature (recording, import,
clip page, MCP query, auth, share, deep-links) working.

## Design system (full rewrite of `app/src/styles.css`)

| Token family | What it does |
| --- | --- |
| `--sodium / --signal / --grape / --sun` | The four brand colors |
| `--clay / --clay-sm / --clay-in` / `--raise / --raise-sm` | The clay shadow stack (soft + inset + drop) |
| `--pop / --pop-signal / --pop-sodium` | One per button variant, derived from the clay stack |
| `--ease-clip: cubic-bezier(.34,1.56,.42,1)` | The single cinematic ease used everywhere surfaces lift in |
| `--cinema` | Warm pastel gradient behind the watch surface + landing wipe |
| `--lg-*` | Liquid-glass nav + theme-pill tints |

`prefers-reduced-motion` is honoured at the CSS level AND in every component
that mounts with framer-motion.

## Component changes

* **Brand.tsx** — new clay tablet SVG (sodium play wedge, signal halo, signal
  "XD" pill). Inline so first paint is fast and there's no asset fetch.
* **Landing.tsx** — floating clay-glass nav (::after sheen sweep + glass
  theme pill), hero wipe with two-layer compositing and parse-box overlays
  (ocr/event/ocr+net boxes with clay labels), 5-track INDEX card,
  CloakPipe redacted pill, ~340-token pill, draggable glass seam knob. All
  sections from the design ported: two-bodies thesis, index breakdown,
  pipeline, cinematic + Loom-import features, MCP panel with JSON sample,
  comparison table, final CTA, 4-column footer.
* **Sidebar.tsx** — logomark bubble, pill nav rows with active chip-accent
  (sodium/signal/grape), MCP connected row at the bottom.
* **App.tsx** (topbar) — pill search, glass theme pill, sodium pill Record
  button. ViewBody is its own component for clean AnimatePresence between
  routes; landing ↔ cloud cross-fades.
* **Library.tsx** — pill filters, paste-link pill bar, rounded clip cards
  with hover-lift spring, key-based stagger in.
* **ClipPage.tsx** — pill seam toggle, pill share link.
* **WatchBody / ReadBody** — pill scrubber, pill play button, pill chapter
  rows, pill tabs (unchanged data flow, only markup).
* **Recording.tsx / Import.tsx / Chat.tsx / Login.tsx / ShareModal.tsx** —
  pill treatments throughout, AnimatePresence on dynamic lists & modals,
  AnimatePresence on the import step stack and chat thread.

## Motion primitives (`app/src/motion.ts`)

A tiny module — one easing, one spring, one fade-up Variants, plus a
`usePrefersReducedMotion()` hook (window.matchMedia, no SSR drift). Kept
intentionally small so the design stays subtle, not chatty.

## React best practices applied

* `useCallback` on top-level App setters (`goCloud`, `openClip`,
  `afterCreate`, `toggleTheme`, `showToast`) so the ViewBody subtree
  doesn't tear down on theme toggles or toast pops.
* `React.memo` on `ClipCard` so the grid stays inert during filter typing.
* `useRef`-backed timer handle for the toast so a rapid second toast
  actually cancels the first.
* `useMemo`-derived `revealClip` only recomputes when `wipe` changes.
* `key`-based transitions on each cloud route → each view animates in once
  per mount, not per redirect.
* Strict mode is on (`tsc --noEmit` clean + `vite build` clean).
* Bundle: 53.7 kB CSS / 341 kB JS (108 kB gzipped). framer-motion dominates
  the JS delta and tree-shakes unused features automatically.

## Not in scope (intentional, easy follow-ups)

* No new backend changes — index.json, query, MCP, ingest all keep their
  existing shape.
* No new analytics / telemetry.
* The pulse/scan sweep keyframes on recording/scan-sweep are kept from the
  old CSS for visual continuity.

## How to verify

1. `cd app && npm install && npm run dev`
2. Visit `/` — verify the landing has the clay nav, hero wipe is
   draggable, two-bodies + index + pipeline cards render.
3. Click "Open app" — sidebar appears, library loads.
4. Open any clip — watch/read dual body with the pill seam toggle.
5. Toggle theme — pill theme indicator flips between sodium (light)
   and signal (dark); env gradient + env tokens swap.
6. Enable "Reduce motion" in your OS accessibility settings — animations
   become instant transitions.
