#!/usr/bin/env python3
"""clipxd Moondream2 caption server — run this on your VPS.

Loads Moondream2 ONCE and serves batched scene captions over HTTP, so the heavy model
lives on your infrastructure (its bandwidth + compute) and clipxd just POSTs salient frames.

Setup (on the VPS):
    pip install flask transformers torch einops pillow
    # optional auth:  export CLIPXD_TOKEN=some-secret
    # optional model: export MOONDREAM_MODEL=vikhyatk/moondream2   (default)
    python server.py            # listens on 0.0.0.0:8799

Then point clipxd at it:
    export CLIPXD_CAPTION_URL=https://your-vps:8799     # (and CLIPXD_TOKEN if set)

Endpoints:
    GET  /health            -> {"ok":true,"model":...,"cuda":bool}
    POST /caption           -> ["caption", ...]   body: {"prompt"?, "frames":[{"b64":...}]}
"""
import base64
import io
import os

from flask import Flask, jsonify, request
from PIL import Image
import torch
from transformers import AutoModelForCausalLM, AutoTokenizer

MODEL = os.environ.get("MOONDREAM_MODEL", "vikhyatk/moondream2")
TOKEN = os.environ.get("CLIPXD_TOKEN", "")
DEVICE = "cuda" if torch.cuda.is_available() else "cpu"
# fp16 fits a ~4GB CPU box (~3.7GB) where fp32 (~7GB) would OOM; override via MOONDREAM_DTYPE.
WANT_DTYPE = os.environ.get("MOONDREAM_DTYPE", "float16" if DEVICE == "cpu" else "float16")

tok = None


def caption_image_with(m, img, prompt):
    if hasattr(m, "caption"):
        try:
            return m.caption(img, length="short")["caption"]
        except Exception:
            pass
    if hasattr(m, "query"):
        try:
            return m.query(img, prompt)["answer"]
        except Exception:
            pass
    enc = m.encode_image(img)
    return m.answer_question(enc, prompt, tok)


def _load(dtype_name):
    global tok
    dt = getattr(torch, dtype_name)
    m = AutoModelForCausalLM.from_pretrained(MODEL, trust_remote_code=True, torch_dtype=dt)
    if DEVICE == "cuda":
        m = m.to(DEVICE)
    try:
        tok = AutoTokenizer.from_pretrained(MODEL)
    except Exception:
        tok = None
    return m


def _selftest(m):
    img = Image.new("RGB", (64, 64), (20, 20, 30))
    return caption_image_with(m, img, "Describe this.")


print(f"[clipxd] loading {MODEL} on {DEVICE} (dtype={WANT_DTYPE}) …", flush=True)
model = None
for dt in [WANT_DTYPE, "float32"]:
    try:
        model = _load(dt)
        _ = _selftest(model)  # validate this dtype actually runs inference on this device
        print(f"[clipxd] ready (dtype={dt})", flush=True)
        break
    except Exception as e:
        print(f"[clipxd] dtype {dt} failed: {str(e)[:160]}", flush=True)
        model = None
if model is None:
    raise SystemExit("[clipxd] could not load Moondream2 in any dtype")

app = Flask(__name__)


def authed():
    if not TOKEN:
        return True
    return request.headers.get("authorization", "") == f"Bearer {TOKEN}"


@app.get("/health")
def health():
    return jsonify({"ok": True, "model": MODEL, "cuda": torch.cuda.is_available()})


@app.post("/caption")
def caption():
    if not authed():
        return jsonify({"error": "unauthorized"}), 401
    body = request.get_json(force=True, silent=True) or {}
    prompt = body.get("prompt") or "Describe what is happening on this screen in one concise sentence."
    out = []
    for f in body.get("frames", []):
        cap = ""
        try:
            img = Image.open(io.BytesIO(base64.b64decode(f["b64"]))).convert("RGB")
            cap = (caption_image_with(model, img, prompt) or "").strip()
        except Exception:
            cap = ""
        out.append(cap)
    return jsonify(out)


if __name__ == "__main__":
    app.run(host="0.0.0.0", port=int(os.environ.get("PORT", 8799)))
