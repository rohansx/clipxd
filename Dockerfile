# clipxd container images.
#
# TWO final targets are built from this one file:
#   --target web   → the Rust service (clipxd-web) + the tools it shells out to
#   --target edge  → Caddy + the built SPA, i.e. everything Caddy served off disk on the VM
#
# They are split because the SPA is static files that Caddy reads directly; sharing them between
# containers would otherwise need a volume whose contents drift from the image that built them.
# Two images from one build keeps them versioned together.
#
# BUILD CONTEXT IS THE PARENT DIRECTORY, not this repo:
#
#   docker build -f clipxd/Dockerfile --target web  -t clipxd-web  .
#
# because the workspace path-depends on ../veyo (see the workspace Cargo.toml — veyo-core and
# veyo-enrich are path deps during active enrich development). A build context of just this repo
# cannot see them and fails to resolve the workspace. CI checks out both repos side by side; see
# .github/workflows/images.yml.

# ---------------------------------------------------------------- SPA -------------------------
FROM node:22-bookworm-slim AS spa
WORKDIR /app
# package.json first so `npm ci` is cached until dependencies actually change.
COPY clipxd/app/package.json clipxd/app/package-lock.json ./
RUN npm ci
COPY clipxd/app/ ./
RUN npm run build

# ---------------------------------------------------------------- whisper ---------------------
# Transcription runs locally (whisper.cpp); everything else that could be called a model — the
# captioner, the title/chapter/Ask passes — is a remote API. Built from source because no distro
# ships whisper-cli.
FROM debian:trixie-slim AS whisper
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential cmake git ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
RUN git clone --depth 1 https://github.com/ggml-org/whisper.cpp .
# BUILD_SHARED_LIBS=OFF is the point: by default whisper.cpp produces libwhisper.so / libggml.so
# beside the binary and resolves them by RPATH into the build tree. That works on a VM where the
# build directory sticks around — it is how the old host ran — but a discarded builder stage takes
# those libraries with it, and the binary then dies at exec with "libwhisper.so.1: cannot open
# shared object file". Static gives one self-contained file to copy.
RUN cmake -B build -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF \
        -DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_EXAMPLES=ON \
    && cmake --build build --config Release -j"$(nproc)" --target whisper-cli
# The base.en model is baked in rather than fetched at boot: a container that needs the network
# before it can transcribe is a container that silently produces empty transcripts on a bad day.
# --http1.1 because HuggingFace's HTTP/2 stream dies part-way through a 142 MB file often enough
# to fail a build ("curl: (92) stream was not closed cleanly"); retries cover the rest. The size
# check is the important part: a truncated download would otherwise bake a corrupt model into the
# image and surface months later as silently empty transcripts.
RUN mkdir -p /models && \
    curl -fL --http1.1 --retry 5 --retry-delay 3 --retry-all-errors \
        -o /models/ggml-base.en.bin \
        https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin && \
    test "$(stat -c %s /models/ggml-base.en.bin)" -gt 100000000

# ---------------------------------------------------------------- rust ------------------------
# TRIXIE, NOT BOOKWORM — this is load-bearing. The `ort` crate links a prebuilt ONNX Runtime
# (for oar-ocr) that was built against glibc 2.38+, so on bookworm (glibc 2.36) the link fails on
# `undefined symbol: __isoc23_strtoll` / `strtoull` / `strtol`. Those `__isoc23_*` names are the
# giveaway: they are glibc's C23-conformant strtol family, introduced in 2.38.
#
# It linked fine on the VM only because that box is Ubuntu 24.04 (glibc 2.39). Containerising is
# what surfaced a dependency the host had been satisfying by accident.
#
# build-essential rather than a pinned libstdc++-N-dev: the ONNX objects also need libstdc++ at
# link time, and letting the distro pick its own g++ keeps this working across a base bump.
FROM rust:1-trixie AS rust
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev build-essential \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
# Both repos, laid out exactly as the path deps expect (clipxd/ next to veyo/).
COPY veyo/ /build/veyo/
COPY clipxd/ /build/clipxd/
WORKDIR /build/clipxd
RUN cargo build --release -p clipxd-web -p clipxd-cli

# ---------------------------------------------------------------- web -------------------------
# Same base family as the builder: a binary linked against glibc 2.41 will not start on 2.36.
FROM debian:trixie-slim AS web
# ffmpeg/ffprobe are shelled out to for every probe, frame extraction, remux and audio slice.
# tesseract's English data is the OCR *fallback*; the primary (oar-ocr) links ONNX Runtime
# statically into the binary and needs nothing here.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ffmpeg ca-certificates tesseract-ocr tesseract-ocr-eng \
    && rm -rf /var/lib/apt/lists/*

COPY --from=rust /build/clipxd/target/release/clipxd-web /usr/local/bin/clipxd-web
COPY --from=rust /build/clipxd/target/release/clipxd /usr/local/bin/clipxd
COPY --from=whisper /src/build/bin/whisper-cli /usr/local/bin/whisper-cli
COPY --from=whisper /models/ggml-base.en.bin /opt/clipxd/models/ggml-base.en.bin

# Build-time smoke test: every binary this image ships must actually be able to start. The
# whisper shared-library breakage above got as far as a built image and was only caught by
# running it — this turns that into a build failure instead.
RUN for b in clipxd-web clipxd whisper-cli; do \
        ldd "/usr/local/bin/$b" 2>/dev/null | grep -q "not found" \
            && { echo "FATAL: $b has unresolved shared libraries"; ldd "/usr/local/bin/$b" | grep "not found"; exit 1; }; \
        true; \
    done && ffprobe -version >/dev/null

ENV WHISPER_MODEL=/opt/clipxd/models/ggml-base.en.bin \
    TESSDATA_PREFIX=/usr/share/tesseract-ocr/5/tessdata \
    # HOME is where oar-ocr caches its ONNX models (~133 MB, fetched on first use). Pointing it
    # at the data volume means one download for the life of the deployment rather than one per
    # container start.
    HOME=/data \
    CLIPXD_PORT=8787

# Clips, the SQLite auth DB, and the OCR model cache all live here. Must be a volume: it is the
# only state the container has, and it is not reconstructible from the image.
VOLUME ["/data"]
WORKDIR /data
EXPOSE 8787

# Runs as root deliberately: the entrypoint has to chown the mounted volume on first boot (a
# fresh Docker volume comes up owned by root regardless of the image's user), and dropping
# privileges afterwards is the entrypoint's job, not the Dockerfile's.
COPY clipxd/deploy/dokploy/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]

# ---------------------------------------------------------------- edge ------------------------
# Caddy with the SPA baked in. TLS is Traefik's job in front of this, so the Caddyfile here binds
# :80 and keeps only what it was always really doing: routing, cache headers, and the CSP.
FROM caddy:2-alpine AS edge
COPY --from=spa /app/dist/ /srv/
COPY clipxd/deploy/dokploy/Caddyfile /etc/caddy/Caddyfile
EXPOSE 80
