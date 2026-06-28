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

### Enable Moondream2

Runs on-device. Either the lightweight package, or transformers + the HF weights:

```bash
# Lightweight client (auto-enables clipxd's VLM captioner when importable):
pip install moondream Pillow
#   optionally point at a downloaded model file: export MOONDREAM_MODEL=/path/to/moondream-2b-int8.mf

# Or via transformers (opt-in, to avoid a surprise multi-GB download on a random ingest):
pip install transformers torch Pillow
export CLIPXD_MOONDREAM=1
```

`Pillow` is required (the sidecar opens frames). Detection is conservative: enabled when
`moondream` imports, or when `CLIPXD_MOONDREAM` is set and `transformers` imports — otherwise
it stays on the heuristic captioner. Captioning runs only on veyo's **salient** frames, so a
1.8B model is affordable per clip.

> Same rule as OCR: run the model **locally**. Don't route frames to a hosted VLM endpoint.
