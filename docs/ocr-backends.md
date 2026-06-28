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

## Next: semantic captions

Even perfect OCR only yields *text*. The `Captioner` trait (currently the heuristic
"summary + on-screen text") is the slot for a **local VLM** that describes the *scene/action*
per salient frame — the upgrade that turns the index from *searchable* into *understood*.
