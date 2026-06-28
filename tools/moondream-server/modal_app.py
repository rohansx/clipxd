"""clipxd Moondream2 caption service on Modal (serverless GPU).

Why Modal: a T4 captions in well under a second, the model + 3.7GB weights download on
Modal's infra (cached in a Volume, not your local/VPS link), it scales to zero so you only
pay while captioning, and it never touches your VPS. clipxd talks to it via the same
`/caption` contract as the self-hosted Flask server, so the `RemoteCaptioner` backend works
unchanged — just point `CLIPXD_CAPTION_URL` at the deployed URL.

Deploy (once):
    pip install modal
    modal token new                                   # auth your account
    # optional auth so the endpoint isn't open:
    #   modal secret create clipxd-token CLIPXD_TOKEN=$(openssl rand -hex 16)
    modal deploy tools/moondream-server/modal_app.py
    # → prints a URL like  https://<you>--clipxd-moondream-web.modal.run

Use from clipxd:
    export CLIPXD_CAPTION_URL=https://<you>--clipxd-moondream-web.modal.run
    export CLIPXD_TOKEN=...        # only if you created the secret above
    # next ingest logs `caption=remote` and stores real Moondream2 scene captions.

Endpoints (same contract as the Flask server):
    GET  /health     -> {"ok":true, "model":..., "cuda":true}
    POST /caption    -> ["caption", ...]   body: {"prompt"?, "frames":[{"b64": <base64 png>}]}
"""
import modal

app = modal.App("clipxd-moondream")

image = (
    modal.Image.debian_slim(python_version="3.11")
    .pip_install("torch", "transformers", "einops", "pillow", "accelerate", "fastapi[standard]")
)

# weights cache so the 3.7GB download happens once, not on every cold start
cache = modal.Volume.from_name("clipxd-moondream-cache", create_if_missing=True)
MODEL = "vikhyatk/moondream2"

# attach the auth secret if it exists (optional)
try:
    _secrets = [modal.Secret.from_name("clipxd-token")]
except Exception:
    _secrets = []


@app.function(
    image=image,
    gpu="T4",
    volumes={"/cache": cache},
    secrets=_secrets,
    scaledown_window=300,  # stay warm 5 min after the last request
    timeout=600,
)
@modal.concurrent(max_inputs=4)
@modal.asgi_app()
def web():
    import base64
    import io
    import os

    import torch
    from fastapi import FastAPI, HTTPException, Request
    from PIL import Image
    from transformers import AutoModelForCausalLM, AutoTokenizer

    os.environ.setdefault("HF_HOME", "/cache/hf")
    model = AutoModelForCausalLM.from_pretrained(MODEL, trust_remote_code=True, torch_dtype=torch.float16).to("cuda")
    try:
        tok = AutoTokenizer.from_pretrained(MODEL)
    except Exception:
        tok = None
    cache.commit()  # persist the downloaded weights
    token = os.environ.get("CLIPXD_TOKEN", "")

    def cap(img, prompt):
        if hasattr(model, "caption"):
            try:
                return model.caption(img, length="short")["caption"]
            except Exception:
                pass
        if hasattr(model, "query"):
            try:
                return model.query(img, prompt)["answer"]
            except Exception:
                pass
        enc = model.encode_image(img)
        return model.answer_question(enc, prompt, tok)

    api = FastAPI()

    @api.get("/health")
    def health():
        return {"ok": True, "model": MODEL, "cuda": torch.cuda.is_available()}

    @api.post("/caption")
    async def caption(req: Request):
        if token and req.headers.get("authorization", "") != f"Bearer {token}":
            raise HTTPException(status_code=401, detail="unauthorized")
        body = await req.json()
        prompt = body.get("prompt") or "Describe what is happening on this screen in one concise sentence."
        out = []
        for f in body.get("frames", []):
            c = ""
            try:
                img = Image.open(io.BytesIO(base64.b64decode(f["b64"]))).convert("RGB")
                c = (cap(img, prompt) or "").strip()
            except Exception:
                c = ""
            out.append(c)
        return out

    return api
