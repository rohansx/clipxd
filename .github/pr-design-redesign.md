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

---

# Follow-up: SEO + agent-browser sweep

## What this follow-up adds

### Real SEO
The SPA now actually looks like a meta-rich surface to crawlers.

* `app/index.html` is filled with: `<title>`, `<meta description>`, robots,
  `og:title/description/url/image/type/width/height/alt/site_name/locale`,
  `twitter:card/site/creator/title/description/image/alt`, canonical,
  `apple-mobile-web-app-*`, theme color (light + dark), manifest link,
  preconnects, plus **two JSON-LD blobs**: a `SoftwareApplication` schema
  with featureList / offers / license and an `Organization` schema.
* `public/favicon.svg` — the new clay tablet mark in vector form (scales
  from 16px favicon to 512px PWA icon).
* `public/og-image.svg` — 1200x630 SVG social card mirroring the hero.
* `public/manifest.json` — PWA manifest with three shortcuts.
* `public/robots.txt` — allows all good crawlers, blocks `/api /clip /u`,
  exposes sitemap. Explicit allow for GPTBot, ClaudeBot, PerplexityBot,
  Google-Extended.
* `public/sitemap.xml` — root + three shortcut URLs, image entry.
* `src/seo.tsx` — `<Seo>` component on `react-helmet-async` with
  `prioritizeSeoTags` and a `SEO_VIEWS` table. The Landing/Auth/Cloud views
  each emit their own per-view meta. The ClipPage owns its own SEO with
  a dynamically-loaded title and a `VideoObject` JSON-LD blob.
* `src/main.tsx` — wired `HelmetProvider`.

### UI bugs found with agent-browser

While driving the dev server with [`vercel-labs/agent-browser`](https://github.com/vercel-labs/agent-browser)
I caught (and fixed) the following:

1. **MCP code block rendered as literal `<span>` text** — the original
   template string embedded JSX span tags. React printed them as text.
   Rewrote with proper JSX spans + `pre.code` styling.
2. **Wipe parse-box labels clipped by the seam** — labels were positioned
   at `top:-9px; left:-1px` (just outside the box), so when the box
   straddled the seam and got clipped, the label went with it. Moved them
   INSIDE the box at `top:6px; left:8px` so they always stay visible.
3. **Auth card phantom second card** — `Login.tsx` was rendering its own
   `.auth-screen > .auth-card` wrapper, nested inside the one in
   `App.tsx`. The page looked like two cards stacked. Split: the shell
   belongs to App (where the back button lives); Login is just the form.
4. **Auth card overflowed the viewport** — reduced internal padding and
   switched the grid from `place-items: center` to `align-content: center;
   justify-items: center` so the centered card never ends up above the
   fold.
5. **"Paste a Loom instead" was a no-op** — wired it to navigate to the
   import view (the obvious behaviour). "Read the docs" now opens the
   GitHub repo in a new tab.
6. **Wipe seam was too thin to find** — widened to 3px with a stronger
   glass glow; the knob now has explicit z-index so it always sits above
   the read overlay.

### Agent-browser wiring

The CLI is now a dev dep so the team (or future agents) can spin up a
live browser against the dev server for UI audits:

    npx agent-browser install        # one-time, downloads Chrome
    npx agent-browser open http://localhost:5174/
    npx agent-browser snapshot -i    # accessibility tree with @refs
    npx agent-browser screenshot /tmp/x.png
    npx agent-browser set viewport 1440 900
    npx agent-browser diff screenshot --baseline before.png

### Other small cleanups

* `Brand.tsx` now accepts `withWord` and uses **per-size SVG gradient
  IDs** so multiple Brand instances on one page don't collide.
* The `Wordmark` export is gone (folded into Brand with `withWord`).
