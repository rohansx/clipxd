#!/bin/sh
# Container entrypoint for clipxd-web.
#
# Does the two things a Dockerfile cannot: make the mounted volume writable (a fresh Docker
# volume arrives owned by root no matter what the image declares), and fail loudly on a
# misconfiguration rather than booting into a state that looks fine and loses data.
set -eu

DATA_DIR="${CLIPXD_DATA_DIR:-/data}"
CLIPS_DIR="$DATA_DIR/clips"
PORT="${CLIPXD_PORT:-8787}"

mkdir -p "$CLIPS_DIR"

# Refuse to start rather than serve from a directory whose contents cannot be written. Silently
# read-only storage is how a recorder ends up accepting uploads it can never commit.
if ! touch "$CLIPS_DIR/.write-test" 2>/dev/null; then
    echo "FATAL: $CLIPS_DIR is not writable — check the volume mount" >&2
    exit 1
fi
rm -f "$CLIPS_DIR/.write-test"

# Auth is on in every hosted deployment, and its secret has no safe default: a generated-per-boot
# secret would invalidate every session on restart, and a hardcoded one is worse.
if [ "${CLIPXD_AUTH:-}" = "1" ] && [ -z "${CLIPXD_JWT_SECRET:-}" ]; then
    echo "FATAL: CLIPXD_AUTH=1 requires CLIPXD_JWT_SECRET (>= 16 chars)" >&2
    exit 1
fi

# Say out loud what this container will and won't do, so a surprising index later is traceable to
# a boot decision rather than guessed at from behaviour.
echo "clipxd-web starting"
echo "  data dir   : $DATA_DIR"
echo "  storage    : ${CLIPXD_STORAGE:-local disk only (no object storage configured)}"
echo "  server OCR : ${CLIPXD_OCR_MODE:-server}"
echo "  whisper    : ${WHISPER_MODEL:-<unset — transcription disabled>}"

exec /usr/local/bin/clipxd-web "$CLIPS_DIR" --port "$PORT"
