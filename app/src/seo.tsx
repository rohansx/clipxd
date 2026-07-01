/**
 * Tiny SEO wrapper around react-helmet-async. The static index.html already
 * ships rich defaults (title, description, OG, Twitter, JSON-LD), so this
 * component only overrides what changes per view:
 *
 *   <Seo title="…" description="…" />
 *
 * It also emits JSON-LD `BreadcrumbList` for nested views so Google can show
 * the path in search results, and a `noindex` flag for routes we don't want
 * crawlers to surface (deep-link clip pages, the auth gate).
 */
import { Helmet } from "react-helmet-async";

const BASE_URL = "https://clipxd.com";
const OG_IMAGE = `${BASE_URL}/og-image.svg`;
const DEFAULT_DESCRIPTION =
  "clipxd is a local-first screen recorder that produces a structured, agent-queryable index alongside every video. Record once. Humans get a beautiful video. Agents get transcript, OCR, events, and chapters — queryable from the same link over MCP.";
const DEFAULT_TITLE = "clipxd — record once, agents read it";
const TWITTER_HANDLE = "@clipxd";

export interface SeoProps {
  title?: string;
  description?: string;
  /** Path under the origin — used to set the canonical URL. */
  path?: string;
  /** When true, add <meta name="robots" content="noindex">. */
  noindex?: boolean;
  /** Optional JSON-LD blob to inject. */
  jsonLd?: object | object[];
  ogType?: "website" | "article" | "profile" | "video.other";
}

export function Seo({ title, description, path, noindex, jsonLd, ogType }: SeoProps) {
  const fullTitle = title ? `${title} · clipxd` : DEFAULT_TITLE;
  const desc = description ?? DEFAULT_DESCRIPTION;
  const canonical = path ? `${BASE_URL}${path}` : BASE_URL;
  const lds = Array.isArray(jsonLd) ? jsonLd : jsonLd ? [jsonLd] : [];

  return (
    <Helmet prioritizeSeoTags>
      <title>{fullTitle}</title>
      <meta name="description" content={desc} />
      {noindex && <meta name="robots" content="noindex,nofollow" />}
      {!noindex && <link rel="canonical" href={canonical} />}

      {/* Open Graph — only override what changes; the rest comes from index.html */}
      <meta property="og:title" content={fullTitle} />
      <meta property="og:description" content={desc} />
      <meta property="og:url" content={canonical} />
      {ogType && <meta property="og:type" content={ogType} />}
      <meta property="og:image" content={OG_IMAGE} />

      {/* Twitter */}
      <meta name="twitter:title" content={fullTitle} />
      <meta name="twitter:description" content={desc} />
      <meta name="twitter:image" content={OG_IMAGE} />
      <meta name="twitter:site" content={TWITTER_HANDLE} />

      {/* JSON-LD blobs — replaced wholesale on each render */}
      {lds.map((ld, i) => (
        <script key={i} type="application/ld+json">
          {JSON.stringify(ld)}
        </script>
      ))}
    </Helmet>
  );
}

/* ------------------------------------------------------------------ */
/*                      Pre-built per-view metadata                    */
/* ------------------------------------------------------------------ */

export const SEO_VIEWS = {
  landing: {
    title: "clipxd — a screen recording an agent can read",
    description:
      "Local-first screen recorder with a structured, MCP-queryable index per clip. Transcript, OCR, events, chapters. The link is the API. Open-core, Apache-2.0.",
    path: "/",
  },
  library: {
    title: "Library",
    description:
      "Browse every clip you've recorded or imported. Filter by source, search transcripts, and open the watch/read dual view on any clip.",
    path: "/library",
  },
  recording: {
    title: "Record",
    description:
      "Record your screen with system audio, cursor, and click/keystroke capture. Auto-zoom follows the cursor; OCR, whisper.cpp transcription, and the structured index are built as you go.",
    path: "/record",
  },
  import: {
    title: "Import",
    description:
      "Drop any Loom, Cap, YouTube, or MP4 URL. clipxd reads it and ships the same queryable index and MCP endpoint as native recordings.",
    path: "/import",
  },
  chat: {
    title: "Ask an agent",
    description:
      "Ask a question across every clip in your library. clipxd searches transcript, OCR, captions, and events — no video is fetched.",
    path: "/chat",
  },
  clip: {
    title: undefined as string | undefined, // set dynamically with the clip title
    description:
      "Watch the recording, read the structured index, and ask the clip a question. Every clip has transcript, OCR, event, and chapter tracks — queryable from the same link.",
    path: "/clip",
    noindex: true,
  },
  auth: {
    title: "Sign in",
    description: "Sign in or create a clipxd account.",
    path: "/auth",
    noindex: true,
  },
} as const;
