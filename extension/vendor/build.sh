#!/usr/bin/env bash
# One-time (re-run-when-upgrading) vendoring step for local, in-browser Moondream2 captioning.
#
# This extension has NO build step — plain JS files loaded directly via manifest.json. Two
# constraints rule out the simpler options:
#   1. MV3's default CSP (script-src 'self') blocks fetching a remote bundler/CDN script at
#      runtime, so "npm install + bundle on every load" is out.
#   2. @huggingface/transformers's own published browser dist (dist/transformers.web.js, what
#      the package's package.json "exports" resolves the bare specifier to) is NOT actually
#      dependency-free: it keeps two bare module specifiers unresolved by design —
#      `import ... from "onnxruntime-web/webgpu"` and `import { Tensor } from "onnxruntime-common"`
#      — expecting a real bundler or jsdelivr's specifier-rewriting CDN to fill them in. Neither
#      applies to a plain file loaded by a browser's native module loader, so just copying that
#      file verbatim (an earlier version of this script did exactly that) fails at runtime with
#      "Failed to resolve module specifier". An import-map-based workaround was tried next and
#      also ruled out empirically: MV3 forbids hash/nonce CSP sources for inline scripts (so an
#      inline <script type="importmap"> can't be allow-listed), and an *external* import map
#      (`<script type="importmap" src="...">`) didn't apply in time for the dynamic `import()`
#      that follows moments later in the same document — confirmed live, the file never even got
#      requested.
#
# So: this script does the actual bundling itself (esbuild, real `npm install` so its resolver
# sees onnxruntime-web/onnxruntime-common in node_modules), producing ONE self-contained file
# with zero remaining bare specifiers — verified by the grep sanity check below, not just assumed.
#
# `--alias:onnxruntime-common=onnxruntime-web/webgpu` forces BOTH bare specifiers to resolve to
# the exact same physical file (onnxruntime-web's own already-self-contained "webgpu bundle"
# variant, ort.webgpu.bundle.min.mjs) rather than each independently bundling their own copy of
# onnxruntime-common's Tensor class — which would otherwise produce two structurally-identical
# but distinct classes, breaking `instanceof` checks between a tensor made by one and checked
# against the other.
#
# Output (checked into git, ~24MB total — almost all of it the one ORT wasm binary):
#   extension/vendor/transformers.min.js                  — the bundle described above
#   extension/vendor/ort-wasm-simd-threaded.asyncify.mjs  — onnxruntime-web wasm factory (webgpu-capable)
#   extension/vendor/ort-wasm-simd-threaded.asyncify.wasm — onnxruntime-web wasm binary (webgpu-capable)
#
# Model weights (Xenova/moondream2, ~1GB in the dtypes we request) are NOT vendored — they're
# fetched from huggingface.co at first use and cached by the browser (IndexedDB/Cache Storage),
# same as the upstream transformers.js demos. Vendoring multi-hundred-MB model weights into a
# git repo would be its own mistake.
#
# IMPORTANT — also requires a manifest.json content_security_policy override: MV3's *actual*
# enforced default for extension pages that don't declare their own content_security_policy was
# found (empirically, not just per the docs) to be stricter than "script-src 'self'
# 'wasm-unsafe-eval'" — WebAssembly.compile()/.instantiate() were blocked with "neither 'wasm-eval'
# nor 'unsafe-eval' is an allowed source". Explicitly declaring
# `{"extension_pages": "script-src 'self' 'wasm-unsafe-eval'; object-src 'self'"}` in
# manifest.json (i.e. just restating the documented default) is what actually turns
# 'wasm-unsafe-eval' on. See manifest.json's own comment.

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

# Pinned exact versions — TRANSFORMERS_VERSION drives which onnxruntime-web build we need;
# ORT_VERSION must match transformers' own package.json dependency on onnxruntime-web exactly
# (that's an exact pin upstream, not a range) or the wasm binary and its JS glue can drift apart.
TRANSFORMERS_VERSION="${TRANSFORMERS_VERSION:-4.2.0}"
ORT_VERSION="${ORT_VERSION:-1.26.0-dev.20260416-b7804b056c}"

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

echo "==> npm install (real install, not npm pack — esbuild needs node_modules resolution)"
( cd "$workdir" \
  && npm init -y >/dev/null \
  && npm install --no-save --silent \
       "@huggingface/transformers@${TRANSFORMERS_VERSION}" \
       "onnxruntime-web@${ORT_VERSION}" \
       esbuild )

cat > "$workdir/entry.js" <<'EOF'
export * from "@huggingface/transformers";
EOF

echo "==> bundling with esbuild"
( cd "$workdir" && npx --no-install esbuild entry.js \
    --bundle --format=esm --platform=browser --minify \
    --target=chrome120 \
    --alias:onnxruntime-common=onnxruntime-web/webgpu \
    --outfile=transformers.bundle.min.js )

echo "==> sanity check: the bundle must have zero remaining bare-specifier imports"
got="$(grep -oE 'from"[a-zA-Z@][^"]*"' "$workdir/transformers.bundle.min.js" || true)"
if [ -n "$got" ]; then
  echo "!! unresolved bare specifiers remain in the bundle — esbuild didn't inline everything:"
  echo "$got"
  exit 1
fi

echo "==> copying vendored files"
cp "$workdir/transformers.bundle.min.js" ./transformers.min.js
cp "$workdir/node_modules/onnxruntime-web/dist/ort-wasm-simd-threaded.asyncify.mjs" ./ort-wasm-simd-threaded.asyncify.mjs
cp "$workdir/node_modules/onnxruntime-web/dist/ort-wasm-simd-threaded.asyncify.wasm" ./ort-wasm-simd-threaded.asyncify.wasm

cat > VERSIONS.txt <<EOF
# Generated by build.sh — do not edit by hand.
@huggingface/transformers ${TRANSFORMERS_VERSION}
onnxruntime-web ${ORT_VERSION}
EOF

echo "==> done"
ls -la ./transformers.min.js ./ort-wasm-simd-threaded.asyncify.mjs ./ort-wasm-simd-threaded.asyncify.wasm
