import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import type { Plugin } from "vite";

// Vite injects `<link rel="stylesheet">` for the main CSS bundle, which is
// render-blocking. We rewrite it to a non-blocking preload pattern:
//   <link rel="preload" as="style" href="…" onload="this.rel='stylesheet'">
//   <noscript><link rel="stylesheet" href="…"></noscript>
// The inlined critical CSS in index.html paints the hero immediately, so
// we don't lose anything by deferring the rest.
function nonBlockingCss(): Plugin {
  return {
    name: "clipxd-nonblocking-css",
    apply: "build",
    transformIndexHtml: {
      enforce: "post",
      transform(html) {
        return html.replace(
          /<link rel="stylesheet" crossorigin href="(\/assets\/index-[^"]+\.css)">/g,
          (_m, href) =>
            `<link rel="preload" as="style" crossorigin href="${href}" ` +
            `onload="this.rel='stylesheet';this.onload=null">` +
            `<noscript><link rel="stylesheet" crossorigin href="${href}"></noscript>`,
        );
      },
    },
  };
}

// Tauri-ready: a fixed dev port, no clearing the screen so Rust logs stay visible.
//
// Build config: code-split the heavy views so the landing-page bundle stays
// small. Landing is the entry page and the one every visitor / Lighthouse
// run scores on. Library/Clip/Recording/etc. are lazy()'d in App.tsx and
// land in their own chunks, only fetched when the user enters the app.
export default defineConfig({
  plugins: [react(), nonBlockingCss()],
  clearScreen: false,
  server: { port: 5174, strictPort: true },
  build: {
    target: "es2020",
    cssMinify: true,
    minify: "esbuild",
    sourcemap: false,
    // Split framer-motion into its own chunk so any change to motion code
    // doesn't bust the main bundle's hash, and so the landing doesn't
    // pull the full motion runtime until it's actually used.
    rollupOptions: {
      output: {
        manualChunks: {
          "react-vendor": ["react", "react-dom", "react-helmet-async"],
          "motion-vendor": ["framer-motion"],
        },
      },
    },
  },
});