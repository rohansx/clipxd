#!/usr/bin/env python3
"""
clipxd-yt-forwarder — runs on your machine (typically the home/GPU box). It listens on
Tailscale, accepts a {url, callback} POST from clipxd-web on the Hetzner box, downloads
the video with yt-dlp locally (so it runs from your residential-IP egress instead of the
Hetzner datacenter), then POSTs the bytes back to clipxd-web's /ingest/tunneled.

Run by `tools/clipxd-yt-forwarder.service` (systemd unit, this directory). Config in
`/etc/clipxd-yt-forwarder.env` (CLIPXD_FORWARDER_TOKEN, CLIPXD_FORWARDER_PORT).

Usage (one-shot, no systemd):
  CLIPXD_FORWARDER_TOKEN=$(openssl rand -hex 16) \\
  CLIPXD_FORWARDER_PORT=8911 \\
  python3 tools/clipxd-yt-forwarder.py
"""

from __future__ import annotations

import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TOKEN = os.environ.get("CLIPXD_FORWARDER_TOKEN", "").strip()
PORT = int(os.environ.get("CLIPXD_FORWARDER_PORT", "8911"))
BIND = os.environ.get("CLIPXD_FORWARDER_BIND", "0.0.0.0")
LOG = print


def log(msg: str) -> None:
    LOG(f"[yt-forwarder {time.strftime('%H:%M:%S')}] {msg}", flush=True)


def resolve_yt_dlp() -> str:
    """`shutil.which` first (respects whatever PATH the process actually has), then the
    common `pip install --user` location — systemd user services get a minimal PATH that
    usually excludes `~/.local/bin`, so a bare `"yt-dlp"` on PATH silently 404s here even
    when it's genuinely installed."""
    found = shutil.which("yt-dlp")
    if found:
        return found
    fallback = pathlib.Path.home() / ".local" / "bin" / "yt-dlp"
    if fallback.exists():
        return str(fallback)
    raise FileNotFoundError(
        "yt-dlp not found on PATH or in ~/.local/bin — install it with "
        "`pip install --user yt-dlp` (or `pipx install yt-dlp`)"
    )


class Handler(BaseHTTPRequestHandler):
    def log_message(self, format: str, *args) -> None:  # noqa: A002
        log(format % args)

    def _json(self, code: int, body: dict) -> None:
        data = json.dumps(body).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802
        if self.path == "/health":
            self._json(200, {"ok": True, "token_set": bool(TOKEN)})
            return
        self._json(404, {"error": "not found"})

    def do_POST(self) -> None:  # noqa: N802
        if self.path != "/fetch":
            self._json(404, {"error": "expected /fetch"})
            return
        length = int(self.headers.get("Content-Length", "0") or 0)
        if length == 0:
            self._json(400, {"error": "empty body"})
            return
        try:
            req = json.loads(self.rfile.read(length).decode("utf-8"))
        except Exception as e:
            self._json(400, {"error": f"bad json: {e}"})
            return
        url = (req.get("url") or "").strip()
        callback = (req.get("callback") or "").strip()
        if not url:
            self._json(400, {"error": "missing url"})
            return
        if not callback:
            self._json(400, {"error": "missing callback"})
            return

        # Block in the handler — clipxd-web's SPA already shows an "importing…" UI, so
        # waiting up to ~5 min for yt-dlp is fine, and we can return {id} synchronously
        # which makes error reporting much cleaner than a fire-and-forget callback model.
        try:
            result = self._download_and_post_back(url, callback)
        except Exception as e:
            # Last-resort net: an unhandled exception here previously killed the request
            # thread with no HTTP response at all (the box saw a hung connection, surfaced
            # to the user as "forwarder offline" even though the process was alive). Always
            # send *something* back.
            log(f"unhandled forwarder error for {url}: {e}")
            self._json(502, {"error": f"forwarder crashed: {e}"})
            return
        if result is None:
            self._json(502, {"error": "yt-dlp or callback failed; see forwarder logs"})
            return
        self._json(200, {"id": result})

    def _download_and_post_back(self, url: str, callback: str) -> str | None:
        log(f"yt-dlp: {url}")
        tmpdir = tempfile.mkdtemp(prefix="clipxd-yt-")
        out_tpl = pathlib.Path(tmpdir) / "%(id)s.%(ext)s"
        try:
            yt_dlp_bin = resolve_yt_dlp()
        except FileNotFoundError as e:
            log(str(e))
            shutil.rmtree(tmpdir, ignore_errors=True)
            return None
        cmd = [
            yt_dlp_bin,
            "--no-playlist",
            "--no-progress",
            "--no-part",
            "-S", "res:1080,ext:mp4:m4a",
            "--merge-output-format", "mp4",
            "-o", str(out_tpl),
            "--print", "after_move:filepath",
            url,
        ]
        try:
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
        except subprocess.TimeoutExpired:
            log(f"yt-dlp timed out for {url}")
            shutil.rmtree(tmpdir, ignore_errors=True)
            return None
        except OSError as e:
            # Covers FileNotFoundError and friends if the resolved path stops working
            # between resolve_yt_dlp() and here (e.g. removed mid-run) — never let a
            # subprocess-launch failure crash the handler thread silently.
            log(f"yt-dlp failed to launch for {url}: {e}")
            shutil.rmtree(tmpdir, ignore_errors=True)
            return None
        if proc.returncode != 0:
            tail = (proc.stderr or "").strip().splitlines()[-3:]
            log(f"yt-dlp failed for {url}: {'; '.join(tail)}")
            shutil.rmtree(tmpdir, ignore_errors=True)
            return None
        # The final --print writes the post-move path. With it, that's the last non-empty line.
        out_path = next((line for line in reversed(proc.stdout.splitlines()) if line.strip()), None)
        if not out_path or not pathlib.Path(out_path).exists():
            log(f"yt-dlp produced no file for {url}")
            shutil.rmtree(tmpdir, ignore_errors=True)
            return None
        try:
            import urllib.request
            filename = pathlib.Path(out_path).name
            with open(out_path, "rb") as f:
                data = f.read()
            log(f"uploading {filename} ({len(data)//1024} KiB) → {callback}")
            req = urllib.request.Request(
                callback,
                data=data,
                method="POST",
                headers={"Content-Type": "video/mp4", "X-Clipxd-Filename": filename},
            )
            with urllib.request.urlopen(req, timeout=120) as r:
                body = r.read().decode("utf-8", errors="replace")
                log(f"callback {r.status}: {body[:200]}")
                # Try to parse the returned {id} so we can hand it back to clipxd-web in one round-trip.
                try:
                    j = json.loads(body)
                    if isinstance(j, dict) and "id" in j:
                        return j["id"]
                except Exception:
                    pass
        except Exception as e:
            log(f"callback POST failed for {url}: {e}")
        finally:
            shutil.rmtree(tmpdir, ignore_errors=True)
        return None


def main() -> int:
    if not TOKEN:
        log("CLIPXD_FORWARDER_TOKEN is not set; the forwarder will reject all requests.")
    log(f"listening on {BIND}:{PORT}")
    server = ThreadingHTTPServer((BIND, PORT), Handler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        log("shutting down")
    return 0


if __name__ == "__main__":
    sys.exit(main())