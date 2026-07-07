// Fully local, in-browser captioning for `caption_mode: "local"` — no network call to any
// captioning service (unlike RemoteCaptioner/Moondream-cloud server-side). Runs Xenova/moondream2
// (a genuinely small, ~1.6-1.8B param VLM — NOT a frontier model, this is the one confirmed to
// actually fit and run under WebGPU in a browser) via Transformers.js, vendored under vendor/
// (see vendor/build.sh) so no npm install/build step is needed to load this extension unpacked —
// MV3's default CSP (script-src 'self') blocks fetching Transformers.js from a CDN at runtime
// anyway, so vendoring is the only viable path, not just the tidiest one.
//
// Loaded as a plain classic script (offscreen.html has no build step) that exposes
// `window.ClipxdLocalCaptioner`; internally it uses a dynamic `import()` of the vendored ESM
// bundle, which works fine from a non-module script and is same-origin (script-src 'self').
//
// Model loading and inference are both best-effort and MUST NEVER block or fail the recording
// itself: WebGPU absence, an adapter request failing, a model load failing (offline, OOM, a
// flaky first-time multi-hundred-MB download) or a single frame's inference throwing all just
// skip that caption (or local captioning entirely) with a console warning.
const ClipxdLocalCaptioner = (() => {
  const MODEL_ID = "Xenova/moondream2";
  const PROMPT = "Describe what is happening on this screen in one short sentence.";
  const CAPTION_INTERVAL_MS = 6000; // a cadence gentle enough for a modest/integrated GPU
  const MAX_BUFFERED = 500; // mirrors the server's /clip/:id/local-captions max-per-call

  /** @type {"idle"|"loading"|"ready"|"unavailable"} */
  let state = "idle";
  /** @type {"webgpu"|"wasm"|null} which device the model actually ended up loaded on, once
   *  state reaches "ready" -- null while idle/loading, and stays null if state is "unavailable". */
  let device = null;
  let modelLoadPromise = null;
  let model = null;
  let processor = null;
  let tokenizer = null;
  let RawImageCls = null;

  let videoEl = null;
  let canvas = null;
  let intervalHandle = null;
  let busy = false; // never let a slow caption call overlap the next tick
  let recordingStartMs = 0;
  let buffer = []; // { t: seconds, caption: string }

  const log = (...a) => console.log("clipxd[local-caption]:", ...a);
  const warn = (...a) => console.warn("clipxd[local-caption]:", ...a);

  /** Dynamic import of the vendored bundle — same-origin, so CSP `script-src 'self'` allows it. */
  function loadTransformers() {
    return import(chrome.runtime.getURL("vendor/transformers.min.js"));
  }

  async function loadModel(device) {
    const mod = await loadTransformers();
    const { AutoProcessor, AutoTokenizer, Moondream1ForConditionalGeneration, RawImage, env } = mod;
    RawImageCls = RawImage;
    // Point the ONNX Runtime Web wasm loader at the vendored copy instead of the default
    // jsdelivr CDN URL — keeps this fully self-contained (no runtime CDN dependency) and one
    // less thing to depend on staying up. See vendor/build.sh for provenance.
    env.backends.onnx.wasm.wasmPaths = {
      mjs: chrome.runtime.getURL("vendor/ort-wasm-simd-threaded.asyncify.mjs"),
      wasm: chrome.runtime.getURL("vendor/ort-wasm-simd-threaded.asyncify.wasm"),
    };
    // dtype choice mirrors the transformers.js team's own Moondream2 WebGPU example: fp16
    // weights + q4 decoder on WebGPU (fp16 arithmetic support), q8/fp32 on wasm (no fp16).
    const dtype = device === "webgpu"
      ? { embed_tokens: "fp16", vision_encoder: "fp16", decoder_model_merged: "q4" }
      : { embed_tokens: "fp32", vision_encoder: "q8", decoder_model_merged: "q4" };
    // Per-file download progress (throttled to whole 10% steps) — the first load fetches
    // ~1-1.7GB of model weights, which can take minutes; a silent multi-minute gap otherwise
    // looks indistinguishable from a hang.
    const lastLoggedPct = {};
    const progress_callback = (p) => {
      if (p && p.status === "progress" && p.file && p.total) {
        const pct = Math.floor((p.loaded / p.total) * 10) * 10;
        if (lastLoggedPct[p.file] !== pct) {
          lastLoggedPct[p.file] = pct;
          log(`downloading ${p.file}: ${pct}%`);
        }
      } else if (p && p.status === "done" && p.file) {
        log(`downloaded ${p.file}`);
      }
    };
    const [proc, tok, mdl] = await Promise.all([
      AutoProcessor.from_pretrained(MODEL_ID, { progress_callback }),
      AutoTokenizer.from_pretrained(MODEL_ID, { progress_callback }),
      Moondream1ForConditionalGeneration.from_pretrained(MODEL_ID, { dtype, device, progress_callback }),
    ]);
    processor = proc;
    tokenizer = tok;
    model = mdl;
  }

  /** Pick a device and load the model, trying WebGPU first and falling back to wasm; sets
   * `state` to "ready" or "unavailable" (never throws). */
  async function doLoad() {
    state = "loading";
    let triedWebgpu = false;
    if ("gpu" in navigator) {
      try {
        const adapter = await navigator.gpu.requestAdapter();
        if (adapter) {
          triedWebgpu = true;
          await loadModel("webgpu");
          state = "ready";
          device = "webgpu";
          log("model ready on webgpu");
          return true;
        }
        warn("navigator.gpu present but no adapter available — falling back to wasm");
      } catch (e) {
        warn("webgpu path failed — falling back to wasm", e);
      }
    } else {
      warn("navigator.gpu not available — falling back to wasm");
    }
    try {
      await loadModel("wasm");
      state = "ready";
      device = "wasm";
      log("model ready on wasm" + (triedWebgpu ? " (webgpu attempt failed)" : ""));
      return true;
    } catch (e) {
      warn("model load failed on both webgpu and wasm — local captioning disabled for this recording", e);
      state = "unavailable";
      device = null;
      return false;
    }
  }

  function ensureModelLoaded() {
    if (state === "ready") return Promise.resolve(true);
    if (state === "unavailable") return Promise.resolve(false);
    if (!modelLoadPromise) modelLoadPromise = doLoad();
    return modelLoadPromise;
  }

  // Moondream1's vision encoder splits its (fixed) 378x378 input into 14px patches — 27x27 =
  // 729 patch embeddings per image. `_merge_input_ids_with_image_features` requires the text's
  // tokenized input_ids to contain *exactly* that many occurrences of the image placeholder
  // token (one literal "<image>" is NOT enough — confirmed live: a single placeholder throws
  // "Number of tokens and features do not match: tokens: 1, features 729"). 729 is an
  // architecture constant (patch grid size), not something derived per-input.
  const IMAGE_PATCH_TOKENS = 729;

  /** Run one caption over the current canvas frame; pushes { t, caption } onto the buffer. */
  async function captionFrame(tSeconds) {
    try {
      const image = await RawImageCls.read(canvas);
      const text = `${"<image>".repeat(IMAGE_PATCH_TOKENS)}\n\nQuestion: ${PROMPT}\n\nAnswer:`;
      const textInputs = tokenizer(text);
      const visionInputs = await processor(image);
      const output = await model.generate({ ...textInputs, ...visionInputs, do_sample: false, max_new_tokens: 64 });
      const decoded = tokenizer.batch_decode(output, { skip_special_tokens: false });
      const raw = decoded[0] || "";
      // Moondream wraps the answer between "Answer:" ... "<END>" — extract just the answer.
      const idx = raw.lastIndexOf("Answer:");
      const caption = (idx >= 0 ? raw.slice(idx + "Answer:".length) : raw)
        .replace(/<END>|<\|endoftext\|>/gi, "")
        .trim();
      if (caption && buffer.length < MAX_BUFFERED) {
        buffer.push({ t: tSeconds, caption });
      }
    } catch (e) {
      warn("caption inference failed for one frame (skipping)", e);
    }
  }

  async function tick() {
    if (busy || state === "unavailable") return;
    busy = true;
    try {
      const ok = await ensureModelLoaded();
      if (!ok || !videoEl) return;
      const w = videoEl.videoWidth;
      const h = videoEl.videoHeight;
      if (!w || !h) return; // not decoding frames yet
      canvas.width = w;
      canvas.height = h;
      canvas.getContext("2d").drawImage(videoEl, 0, 0, w, h);
      await captionFrame((Date.now() - recordingStartMs) / 1000);
    } finally {
      busy = false;
    }
  }

  /** Begin periodic sampling of `tabStream`'s video track. Never throws — a failure here just
   * means no local captions for this recording. */
  async function start(tabStream) {
    buffer = [];
    recordingStartMs = Date.now();
    if (!("gpu" in navigator)) {
      warn("navigator.gpu not available in this context — will attempt wasm fallback");
    }
    try {
      videoEl = document.createElement("video");
      videoEl.srcObject = new MediaStream(tabStream.getVideoTracks());
      videoEl.muted = true;
      await videoEl.play();
    } catch (e) {
      warn("could not start sampling the tab video for local captioning", e);
      videoEl = null;
      return;
    }
    canvas = document.createElement("canvas");
    intervalHandle = setInterval(tick, CAPTION_INTERVAL_MS);
    // Kick off model loading immediately rather than waiting for the first tick, so the
    // (slow, first-time) load overlaps with the recording instead of delaying the first caption
    // by a full interval on top of the load time.
    ensureModelLoaded();
  }

  /** Stop sampling and return the buffered { t, caption } pairs (and clear the buffer). */
  function stop() {
    if (intervalHandle) {
      clearInterval(intervalHandle);
      intervalHandle = null;
    }
    if (videoEl) {
      videoEl.pause();
      videoEl.srcObject = null;
      videoEl = null;
    }
    canvas = null;
    const out = buffer;
    buffer = [];
    return out;
  }

  /** Snapshot of captions buffered so far, without stopping sampling — e.g. for a live status UI,
   * or a test that wants to observe the first real caption land without tearing anything down. */
  function peek() {
    return buffer.slice();
  }

  /** Current { state, device } — the thing the popup polls so a user can actually see whether
   *  local captioning landed on real GPU acceleration, slower CPU fallback, or didn't load at
   *  all, instead of that only ever showing up in this offscreen document's own devtools console. */
  function status() {
    return { state, device };
  }

  return { start, stop, peek, status };
})();
