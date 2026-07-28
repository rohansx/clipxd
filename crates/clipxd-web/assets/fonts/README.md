# Bundled fonts

Two variable-weight woff2 files, latin subset only, served by clipxd-web at `/fonts/*.woff2`
and referenced by the share page's `@font-face` block.

| File | Family | Upstream | Licence |
|---|---|---|---|
| `space-grotesk.woff2` | Space Grotesk | https://fonts.google.com/specimen/Space+Grotesk | SIL Open Font License 1.1 |
| `jetbrains-mono.woff2` | JetBrains Mono | https://fonts.google.com/specimen/JetBrains+Mono | SIL Open Font License 1.1 |

Both are OFL 1.1, which permits redistribution (including bundled inside another work) provided
the fonts are not sold on their own and the licence travels with them. Full text:
https://openfontlicense.org/open-font-license-official-text/

## Why they're in the repo

The share page used to `<link>` them from `fonts.googleapis.com`, which meant every recipient of
every clip made two requests to Google before the page could paint — a third-party dependency on
a page strangers open, the kind of thing that shows up in an enterprise security review, and one
more origin to allow in the Content-Security-Policy. Serving them ourselves removes the request,
removes `fonts.gstatic.com` from `font-src`, and makes the page render identically offline.

Each file is a *variable* font: Google returns byte-identical woff2 for every weight it lists, so
one file per family covers 400–700 via `font-weight: 100 900` in the `@font-face` rule.

## Refreshing them

Re-download the latin subset (`U+0000-00FF`) for each family from the Google Fonts CSS API with a
modern browser User-Agent (it serves woff2 only to browsers that support it), then drop the files
in here under the same names. Nothing else needs to change.
