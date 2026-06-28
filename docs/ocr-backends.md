# OCR backends — extracting on-screen text per frame

clipxd runs OCR on the **salient frames** veyo flags (not every frame — that's the efficiency
win), turning each into timestamped, located `on_screen_text` the agent can query. The OCR
engine is a pluggable `veyo_enrich::Ocr` trait; `Enricher::with_local_defaults()` auto-selects
the best **local** engine — nothing is ever sent off-device.

## Selection order (automatic)

1. **PaddleOCR** (`paddleocr`) — preferred. Far stronger than tesseract on real screens: UI
   chrome, code, tables, low-contrast/antialiased text, mixed layouts. Works with PaddleOCR
   **2.x** (`.ocr`) and **3.x / PP-OCRv5** (`.predict`), and the PaddleOCR-VL pipeline.
2. **tesseract** (`tesseract` CLI) — fallback. Fine on clean high-contrast text.
3. **null** — no engine installed → empty `on_screen_text` (the rest of the index still works).

On ingest, the server logs which one it picked, e.g.:
`enrich backends: transcriber=null ocr=paddleocr caption=heuristic`

## Enable PaddleOCR (recommended)

It's open-source (Apache-2.0) and runs **locally**. The `paddleocr` wheel does **not** pull in
the framework — install `paddlepaddle` (or `paddlepaddle-gpu`) too.

```bash
# Current (3.x, PP-OCRv5):
pip install paddleocr paddlepaddle

# Or pin 2.x:
pip install "paddleocr>=2.6,<3.0" "paddlepaddle>=2.5,<3.0"
```

clipxd detects it by probing `from paddleocr import PaddleOCR`. Models auto-download to
`~/.paddleocr` on first run (PP-OCRv4 mobile is ~16 MB total). No clipxd config needed — the
next ingest will log `ocr=paddleocr`.

> **Do not** use the hosted "Unlimited OCR" Hugging Face Space for this — it would ship every
> screen frame to a third party, breaking clipxd's local-first/privacy guarantee, and isn't
> built for per-frame batch use. Run the open model on-device instead (above).

## Enable tesseract (lightweight fallback)

```bash
# Debian/Ubuntu
sudo apt install tesseract-ocr
# macOS
brew install tesseract
# Arch
sudo pacman -S tesseract tesseract-data-eng
```

clipxd auto-points `TESSDATA_PREFIX` at a found tessdata dir, so OCR works out of the box.

## How it works (for maintainers)

`veyo-enrich/src/ocr.rs` holds the backends. `PaddleOcr` shells out to a bundled Python
sidecar (written to a temp file on `detect()`) that normalizes every PaddleOCR version's
output to one stable JSON contract — `[{"text": str, "conf": 0..1, "bbox": [x,y,w,h]}]` — which
`parse_paddle_json` turns into `OcrSpan`s (confidence normalized to `0..100`). Adding another
engine = implement `Ocr` and slot it into `with_local_defaults`.

## Semantic captions — Moondream2 VLM (local)

OCR only yields *text*; the **captioner** describes the *scene/action* of each salient frame —
the difference between *searchable* and *understood*. The `Captioner` trait selects, locally:

1. **Moondream2** (`MoondreamCaptioner`) — a ~1.8B vision-language model. Captions like
   *"A build dashboard with a red 'deploy failed (exit 1)' error banner"* instead of
   *"56 regions changed."* Loads the model **once** and captions all salient frames in one
   batch (the `Captioner::caption_batch` path) via a bundled Python sidecar.
2. **heuristic** — the templated summary + on-screen text (fallback; also used per-frame when
   the VLM returns nothing).

The ingest log shows it: `enrich backends: … caption=moondream2`.

### Enable Moondream2 (transformers + your GPU)

Runs on-device via transformers. **Pin transformers <5** — 5.x's model-loading refactor breaks
moondream2's `trust_remote_code` (`all_tied_weights_keys` error):

```bash
pip install torch "transformers==4.46.3" pillow einops
# weights (~3.7GB) download once to ~/.cache/huggingface:
python -c "from huggingface_hub import snapshot_download; snapshot_download('vikhyatk/moondream2')"
```

**Auto-detection:** once the model is cached, clipxd enables the local captioner automatically
— no env needed (the next ingest logs `caption=moondream2`). `CLIPXD_MOONDREAM=1` forces it on
before caching. The sidecar loads **fp16 on CUDA**, fp32 on CPU; `Pillow` opens the frames.
Captioning runs only on veyo's **salient** frames, so a 1.8B model is affordable per clip.

**Measured on an RTX 4050 Laptop (6 GB VRAM):** ~4.7 GB peak VRAM, ~3 s load (once per ingest),
~2 s/frame inference. CPU-only also works (fp32, ~8 GB RAM, ~10–30 s/frame). Run the server with
the venv on `PATH` so the sidecars' `python3` is the one with torch/transformers/paddleocr.

### Caption backend priority

`with_local_defaults()` picks the first available, in this order:

1. **Remote** — a self-hosted caption server at `CLIPXD_CAPTION_URL` (+ optional `CLIPXD_TOKEN`).
   Run `tools/moondream-server/server.py` on a box with RAM/GPU, or deploy
   `tools/moondream-server/modal_app.py` to **Modal** (serverless T4 GPU — best for weak local
   internet / no local GPU; the 3.7 GB download + compute live on Modal, scales to zero).
2. **OpenRouter** — set `OPENROUTER_API_KEY` (+ optional `OPENROUTER_VLM_MODEL`, default
   `openai/gpt-4o-mini`). The fastest way to **test** the feature: no download, no GPU, real
   VLM captions in minutes. Frames go to a third party — testing/opt-in only.
3. **Moondream2 local** — on-device (above).
4. **heuristic** — templated fallback.

The ingest log names the winner: `caption=remote | openrouter | moondream2 | heuristic`.

Privacy: remote/OpenRouter are **opt-in**; the private default is local. Pick the tier that
fits — **OpenRouter to test, local Moondream for privacy, Modal/VPS for scale** — all behind
the same `Captioner` trait, so switching is just an env var.
