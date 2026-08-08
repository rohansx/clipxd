#!/usr/bin/env bash
#
# End-to-end check for `clipxd beautify`'s encode path — the part the unit tests in
# beautify.rs cannot reach, because it is an ffmpeg invocation rather than arithmetic.
#
# It measures CONTENT sync, not container metadata. The first version of this script compared
# `ffprobe` stream durations, which cannot detect desync at all in a pipeline that ends in
# `-shortest`: it makes the two durations agree by construction. A 12-region render that was
# 297 ms out of sync measured 4 ms apart and passed.
#
# So every fixture carries a 1-frame white flash and a 50 ms beep at each integer second. The
# flashes come back out of per-frame luma (`signalstats`), the beeps out of 10 ms RMS windows
# (`astats`), and the assertion is that they still land on top of each other in the OUTPUT.
# A soundtrack pulled early, stretched, or shifted by a rounding error moves the beeps off
# their flashes, whatever the container says.
#
# Region boundaries in the speed fixtures sit ON the markers deliberately: output frame 0 of a
# span always samples its first source frame, so a 1-frame flash survives any decimation and
# what is left to measure is *accumulated* drift rather than the half-frame quantisation that
# is inherent to showing a picture on frame boundaries.
#
# Cases, and the shipped bug each one exists for:
#
#   1  a plain render carries audio, in sync, at full length
#   2  source audio SHORTER than source video (3.0s of a 6.0s clip)  -> `-shortest` truncated
#      the VIDEO to 3.0s / 90 of 180 frames
#   3  a 0.05s audio track (what a suspended AudioContext records)   -> rendered TWO FRAMES
#   4  a trim whose kept span starts after the audio ends            -> muxed output had NO
#      VIDEO STREAM AT ALL, exit 0, shipped as a successful render
#   5  audio with a start offset                                     -> `asetpts=PTS-STARTPTS`
#      discarded it and pulled the whole soundtrack ~1s early
#   6  twelve speed regions at distinct rates                        -> audio led video by
#      0, 63, 117, 170, 233, 297 ms — monotonically, because emit_order quantised each span to
#      whole frames while atempo got the raw requested rate
#   7  a 1.5x ramp is 1.5x (`rate.round().max(1.0)` made it 2x), and the audio follows
#   8  a trim cuts both streams by the same amount
#   9  a source with no audio still renders — a filtergraph pointed at a missing [1:a] aborts
#      the whole export, which is worse than the silence it fixes
#  10  gif takes no audio path, webm gets opus, mp4 gets aac
#  11  two zoom regions differing only in focus produce different video — cx/cy were parsed
#      nowhere and the override loop hardcoded 0.5/0.5
#  12  --bg mint renders mint, not the aurora `_ =>` fallback
#  13  concurrent renders keep their own scratch dirs and clean up after themselves
#  14  a SIGKILLed render's scratch dir is reclaimed by a later render once it is stale — Drop
#      does not run on SIGKILL / OOM-kill, and a leak nothing reclaims is unbounded
#  15  three simultaneous renders of ONE clip through clipxd-web get their own scratch files —
#      they were keyed on the clip id alone. Each asks for a different OUTPUT LENGTH, because two
#      requests expecting the same duration cannot detect being handed each other's video.
#      Skipped unless target/debug/clipxd-web is built.
#  16  a render that 404s reclaims the project file it already wrote — per-request scratch paths
#      left one orphan per request, forever, where the old per-clip path was self-overwriting
#  17  a slow-motion ramp that would emit more frames than the render limit is refused up front,
#      instead of filling the disk one full-resolution PNG at a time and dying mid-render
#  18  a failing ffmpeg says what ffmpeg said — the audio filtergraph is machine-built from user
#      JSON and "ffmpeg failed" was the entire production signal for a malformed one
#
# TMPDIR is redirected into this run's own work dir, so the scratch-dir counts see only this
# script's renders. The old version globbed the shared /tmp and counted other processes' dirs.
#
# Usage:  cargo build -p clipxd-cli -p clipxd-web && tools/render-audio-check.sh
set -u

ROOT=$(git rev-parse --show-toplevel 2>/dev/null)
CLIPXD=${CLIPXD:-$ROOT/target/debug/clipxd}
CLIPXD_WEB=${CLIPXD_WEB:-$ROOT/target/debug/clipxd-web}
[ -x "$CLIPXD" ] || { echo "no clipxd binary at $CLIPXD (cargo build -p clipxd-cli)"; exit 2; }

WORK=$(mktemp -d) || exit 2
trap 'rm -rf "$WORK"' EXIT
# isolate the renderer's scratch dirs from every other process on this box
export TMPDIR="$WORK/tmp"
mkdir -p "$TMPDIR"
cd "$WORK" || exit 2

fail=0
ok()  { echo "  PASS  $*"; }
bad() { echo "  FAIL  $*"; fail=1; }

adur()  { ffprobe -v error -select_streams a:0 -show_entries stream=duration -of csv=p=0 "$1"; }
vdur()  { ffprobe -v error -select_streams v:0 -show_entries stream=duration -of csv=p=0 "$1"; }
vframes() { ffprobe -v error -select_streams v:0 -count_frames -show_entries stream=nb_read_frames -of csv=p=0 "$1"; }
types() { ffprobe -v error -show_entries stream=codec_type -of csv=p=0 "$1" | tr '\n' ' '; }
acodec(){ ffprobe -v error -select_streams a -show_entries stream=codec_name -of csv=p=0 "$1"; }
near()  { awk -v a="$1" -v b="$2" -v t="$3" 'BEGIN{d=a-b; if(d<0)d=-d; exit !(d<=t)}'; }
scratch_dirs() { find "$TMPDIR" -maxdepth 1 -name 'clipxd-beautify-*' | wc -l; }

# --- content probes: where the picture flashes, where the sound beeps -----------------------
flashes() {
  ffprobe -v error -f lavfi -i "movie=$1,signalstats" \
    -show_entries frame=pts_time:frame_tags=lavfi.signalstats.YAVG -of csv=p=0 2>/dev/null |
    awk -F, '{t=$1; y=$2+0; if (y>100 && !on) {printf "%.3f ", t; on=1} else if (y<=100) on=0}'
}
beeps() {
  ffprobe -v error -f lavfi \
    -i "amovie=$1,aformat=sample_rates=48000:channel_layouts=mono,asetnsamples=n=480:p=0,astats=metadata=1:reset=1" \
    -show_entries frame=pts_time:frame_tags=lavfi.astats.Overall.RMS_level -of csv=p=0 2>/dev/null |
    awk -F, '{t=$1; r=$2+0; if (r>-40 && !on) {printf "%.3f ", t; on=1} else if (r<=-40) on=0}'
}

# in_sync <file> <tolerance_s> [expected_beeps]
# Beeps must line up with flashes marker for marker. `expected_beeps` differs from the flash
# count only where the source audio genuinely ran out (cases 2 and 3), and is asserted so a
# render that silently dropped its sound cannot pass by having nothing left to compare.
in_sync() {
  awk -v tol="$2" -v want="${3:--1}" -v F="$(flashes "$1")" -v B="$(beeps "$1")" 'BEGIN {
    nf = split(F, fa, " "); nb = split(B, ba, " ");
    if (want < 0) want = nf;
    if (nf == 0) { print "no flashes found in the video at all"; exit 1 }
    if (nb != want) { printf "%d flash(es) but %d beep(s), wanted %d\n", nf, nb, want; exit 1 }
    worst = 0;
    for (i = 1; i <= nb; i++) { d = fa[i] - ba[i]; if (d < 0) d = -d; if (d > worst) worst = d }
    printf "%d marker(s), worst %.0fms apart\n", nb, worst * 1000;
    exit !(worst <= tol)
  }'
}

# mkvid <out> <dur> <first_flash_second>: black, one white frame at each integer second
mkvid() {
  ffmpeg -v error -y -f lavfi -i "color=c=black:s=320x240:r=30:d=$2" \
    -vf "drawbox=x=0:y=0:w=iw:h=ih:color=white:t=fill:enable='gte(n\,$3*30)*lt(mod(n\,30)\,1)'" \
    -c:v libx264 -pix_fmt yuv420p "$1"
}
# mkvid_mid <out> <dur>: a 3-frame flash HALFWAY through each second. Three frames, not one,
# because a marker that is not on a span boundary can be decimated away by a speed ramp.
mkvid_mid() {
  ffmpeg -v error -y -f lavfi -i "color=c=black:s=320x240:r=30:d=$2" \
    -vf "drawbox=x=0:y=0:w=iw:h=ih:color=white:t=fill:enable='gte(n\,15)*lt(mod(n-15\,30)\,3)'" \
    -c:v libx264 -pix_fmt yuv420p "$1"
}
# mkaud <out> <dur> <shift>: a 50ms 880Hz beep whenever t+shift crosses an integer second
mkaud() {
  ffmpeg -v error -y -f lavfi \
    -i "aevalsrc=0.8*sin(2*PI*880*t)*between(mod(t+$3\,1)\,0\,0.05):s=48000:d=$2" -c:a pcm_s16le "$1"
}

echo "== fixtures (30fps; flash + beep on every integer second) =="
mkvid v6.mp4 6 0 || exit 2
mkaud a6.wav 6 0 || exit 2
mkaud a3.wav 3 0 || exit 2
mkaud a2.wav 2 0 || exit 2
mkvid v12.mp4 12 0 || exit 2
mkaud a12.wav 12 0 || exit 2
mkvid_mid vmid.mp4 12 || exit 2
mkaud amid.wav 12 0.5 || exit 2
mux() { ffmpeg -v error -y -i "$1" -i "$2" -c:v copy -c:a aac "$3"; }   # NO -shortest: keep both
mux v6.mp4 a6.wav  sync6.mp4  || exit 2   # 6.0s video, 6.0s audio
mux v6.mp4 a3.wav  short3.mp4 || exit 2   # 6.0s video, 3.0s audio
mux v6.mp4 a2.wav  short2.mp4 || exit 2   # 6.0s video, 2.0s audio
mux v12.mp4 a12.wav sync12.mp4 || exit 2
mux vmid.mp4 amid.wav mid12.mp4 || exit 2
ffmpeg -v error -y -f lavfi -i "aevalsrc=0.8*sin(2*PI*880*t):s=48000:d=0.05" -c:a pcm_s16le a005.wav || exit 2
mux v6.mp4 a005.wav tiny.mp4 || exit 2    # what a suspended AudioContext records
ffmpeg -v error -y -f lavfi -i testsrc2=size=320x240:rate=30:duration=2 \
  -c:v libx264 -pix_fmt yuv420p textured.mp4 || exit 2    # spatial detail, for the focus test
cp v6.mp4 silent.mp4

# audio that starts 0.978s into the container, beeps still on the flashes
mkvid voff.mp4 6 1 || exit 2
mkaud aoff.wav 5.022 0.978 || exit 2
ffmpeg -v error -y -i voff.mp4 -itsoffset 0.978 -i aoff.wav -map 0:v -map 1:a -c:v copy -c:a aac offset.mp4 || exit 2

echo '{"zoom_regions":[],"edit_regions":[{"kind":"speed","start":0,"end":6,"rate":1.5}]}' > speed15.json
echo '{"zoom_regions":[],"edit_regions":[{"kind":"trim","start":2,"end":4,"rate":1}]}'    > trim.json
echo '{"zoom_regions":[],"edit_regions":[{"kind":"trim","start":0,"end":2,"rate":1}]}'    > trim02.json
echo '{"zoom_regions":[{"start":0,"end":2,"scale":2,"cx":0.1,"cy":0.1}],"edit_regions":[]}' > focusTL.json
echo '{"zoom_regions":[{"start":0,"end":2,"scale":2,"cx":0.9,"cy":0.9}],"edit_regions":[]}' > focusBR.json
# twelve 1s regions at DISTINCT rates — equal adjacent rates merge into one span in `spans()`,
# and a single span cannot show accumulated drift
{ printf '{"zoom_regions":[],"edit_regions":['
  for k in 0 1 2 3 4 5 6 7 8 9 10 11; do
    [ "$k" -gt 0 ] && printf ','
    awk -v k="$k" 'BEGIN{printf "{\"kind\":\"speed\",\"start\":%d,\"end\":%d,\"rate\":%.2f}", k, k+1, 1.70+0.01*k}'
  done
  printf ']}'
} > speed12.json
echo "  sync6: $(types sync6.mp4)| short3: v=$(vdur short3.mp4)s a=$(adur short3.mp4)s | tiny: a=$(adur tiny.mp4)s"

echo "== 1. a plain render carries audio, and the beeps stay on the flashes =="
$CLIPXD beautify sync6.mp4 --padding 0 --out after.mp4 >render.log 2>&1
[ "$(types after.mp4)" = "video audio " ] && ok "streams: $(types after.mp4)" || bad "streams: $(types after.mp4)"
near "$(vdur after.mp4)" 6.0 0.15 && ok "duration preserved: $(vdur after.mp4)s" || bad "duration $(vdur after.mp4)s != 6.0s"
msg=$(in_sync after.mp4 0.05) && ok "in sync — $msg" || bad "out of sync — $msg"
grep -q "emit 180/180 frames" render.log && ok "emit 180/180 frames" \
  || bad "frame selection changed: $(grep -o 'emit [0-9/]* frames' render.log)"

echo "== 2. a 3.0s audio track on a 6.0s clip must not truncate the VIDEO =="
$CLIPXD beautify short3.mp4 --padding 0 --out s3.mp4 >/dev/null 2>&1
[ "$(vframes s3.mp4)" = 180 ] && ok "180 frames kept (-shortest gave 90)" || bad "$(vframes s3.mp4) frames, wanted 180"
near "$(vdur s3.mp4)" 6.0 0.15 && ok "video $(vdur s3.mp4)s" || bad "video $(vdur s3.mp4)s != 6.0s"
near "$(adur s3.mp4)" 6.0 0.15 && ok "audio padded to $(adur s3.mp4)s" || bad "audio $(adur s3.mp4)s != 6.0s"
# beeps only exist for the first 3 seconds; the ones that exist must still be on their flashes
msg=$(in_sync s3.mp4 0.05 3) && ok "surviving beeps in sync — $msg" || bad "out of sync — $msg"

echo "== 3. a 0.05s audio track (suspended AudioContext) must not render two frames =="
$CLIPXD beautify tiny.mp4 --padding 0 --out t.mp4 >/dev/null 2>&1
[ "$(vframes t.mp4)" = 180 ] && ok "180 frames kept (-shortest gave 2)" || bad "$(vframes t.mp4) frames, wanted 180"
near "$(vdur t.mp4)" 6.0 0.15 && ok "video $(vdur t.mp4)s" || bad "video $(vdur t.mp4)s != 6.0s"

echo "== 4. a trim whose kept span starts after the audio ends still has a VIDEO stream =="
$CLIPXD beautify short2.mp4 --padding 0 --project trim02.json --out cut2.mp4 >/dev/null 2>&1
case " $(types cut2.mp4)" in *video*) ok "streams: $(types cut2.mp4)";; *) bad "no video stream: $(types cut2.mp4)";; esac
near "$(vdur cut2.mp4)" 4.0 0.15 && ok "video $(vdur cut2.mp4)s ~= 4.0s" || bad "video $(vdur cut2.mp4)s != 4.0s"
near "$(adur cut2.mp4)" 4.0 0.15 && ok "audio silence-padded to $(adur cut2.mp4)s" || bad "audio $(adur cut2.mp4)s != 4.0s"

echo "== 5. an audio stream that starts late is not pulled to zero =="
$CLIPXD beautify offset.mp4 --padding 0 --out off.mp4 >/dev/null 2>&1
msg=$(in_sync off.mp4 0.05) && ok "leading gap preserved — $msg" || bad "soundtrack shifted — $msg"

echo "== 6. twelve speed regions at distinct rates do not accumulate drift =="
# 6a: markers ON the region boundaries. Output frame 0 of a span always samples that span's
# first source frame, so this measures accumulation with nothing else mixed in.
$CLIPXD beautify sync12.mp4 --padding 0 --project speed12.json --out multi.mp4 >multi.log 2>&1
msg=$(in_sync multi.mp4 0.05) && ok "boundary markers: $msg" || bad "boundary markers drifted — $msg"
# 6b: markers HALFWAY through each region — where the drift was first seen (0, 63, 117, 170,
# 233, 297 ms). The tolerance is looser because a picture can only change on a frame boundary,
# so up to half a frame (17 ms) of offset here is inherent and does not accumulate.
$CLIPXD beautify mid12.mp4 --padding 0 --project speed12.json --out mid.mp4 >/dev/null 2>&1
msg=$(in_sync mid.mp4 0.06) && ok "mid-region markers: $msg" || bad "mid-region markers drifted — $msg"

echo "== 7. a 1.5x ramp is exactly 1.5x (6.0s -> 4.0s), audio follows =="
$CLIPXD beautify sync6.mp4 --padding 0 --project speed15.json --out fast.mp4 >/dev/null 2>&1
near "$(vdur fast.mp4)" 4.0 0.15 && ok "video $(vdur fast.mp4)s ~= 4.0s (the old 2x snap gave 3.0s)" \
  || bad "video $(vdur fast.mp4)s != 4.0s"
msg=$(in_sync fast.mp4 0.05) && ok "audio follows — $msg" || bad "out of sync — $msg"

echo "== 8. a trim cuts both streams by the same 2.0s =="
$CLIPXD beautify sync6.mp4 --padding 0 --project trim.json --out cut.mp4 >/dev/null 2>&1
near "$(vdur cut.mp4)" 4.0 0.15 && ok "video $(vdur cut.mp4)s ~= 4.0s" || bad "video $(vdur cut.mp4)s != 4.0s"
msg=$(in_sync cut.mp4 0.05) && ok "audio follows — $msg" || bad "out of sync — $msg"

echo "== 9. a source with no audio still renders (must not abort) =="
$CLIPXD beautify silent.mp4 --padding 0 --out nosound.mp4 >/dev/null 2>&1
rc=$?
[ $rc -eq 0 ] && ok "exit 0 on a silent source" || bad "exit $rc on a silent source"
[ "$(types nosound.mp4)" = "video " ] && ok "no phantom audio stream" || bad "streams: $(types nosound.mp4)"

echo "== 10. gif skips audio entirely, webm gets opus, mp4 gets aac =="
$CLIPXD beautify sync6.mp4 --format gif  --out out.gif  >/dev/null 2>&1
$CLIPXD beautify sync6.mp4 --format webm --out out.webm >/dev/null 2>&1
[ "$(types out.gif)" = "video " ] && ok "gif: $(types out.gif)" || bad "gif: $(types out.gif)"
[ "$(acodec out.webm)" = "opus" ] && ok "webm audio: opus" || bad "webm audio: $(acodec out.webm)"
[ "$(acodec after.mp4)" = "aac" ]  && ok "mp4 audio: aac"   || bad "mp4 audio: $(acodec after.mp4)"

echo "== 11. a zoom region's focus point reaches the renderer =="
$CLIPXD beautify textured.mp4 --padding 0 --project focusTL.json --out ztl.mp4 >/dev/null 2>&1
$CLIPXD beautify textured.mp4 --padding 0 --project focusBR.json --out zbr.mp4 >/dev/null 2>&1
cmp -s ztl.mp4 zbr.mp4 && bad "two different focus points rendered byte-identical video" \
  || ok "focus 0.1/0.1 and 0.9/0.9 render differently"

echo "== 12. --bg mint renders mint, not the aurora fallback =="
$CLIPXD beautify sync6.mp4 --bg mint --out mint.mp4 >/dev/null 2>&1
$CLIPXD beautify sync6.mp4 --bg aurora --out aur.mp4 >/dev/null 2>&1
for f in mint aur; do
  ffmpeg -v error -y -i $f.mp4 -vf "crop=8:8:0:0" -frames:v 1 -f rawvideo -pix_fmt rgb24 $f.raw
done
cmp -s mint.raw aur.raw && bad "the mint corner is identical to aurora's" || ok "mint background differs from aurora"

echo "== 13. concurrent renders keep their own scratch dirs, and clean up =="
$CLIPXD beautify sync6.mp4 --out par-a.mp4 >/dev/null 2>&1 & p1=$!
$CLIPXD beautify sync6.mp4 --bg mint --out par-b.mp4 >/dev/null 2>&1 & p2=$!
wait $p1; r1=$?
wait $p2; r2=$?
[ $r1 -eq 0 ] && [ $r2 -eq 0 ] && ok "both concurrent renders exited 0" || bad "exit codes $r1 / $r2"
near "$(vdur par-a.mp4)" 6.0 0.15 && near "$(vdur par-b.mp4)" 6.0 0.15 \
  && ok "both outputs are full length" || bad "a: $(vdur par-a.mp4)s  b: $(vdur par-b.mp4)s"
[ "$(scratch_dirs)" -eq 0 ] && ok "scratch dirs cleaned up" || bad "$(scratch_dirs) scratch dir(s) left behind"

echo "== 14. a killed render's scratch dir is reclaimed once it is stale =="
$CLIPXD beautify sync12.mp4 --out killme.mp4 >/dev/null 2>&1 & victim=$!
sleep 3
kill -9 $victim 2>/dev/null; pkill -9 -P $victim 2>/dev/null; wait $victim 2>/dev/null
[ "$(scratch_dirs)" -eq 1 ] && ok "SIGKILL left a scratch dir behind (Drop cannot run)" \
  || bad "expected 1 orphaned scratch dir, found $(scratch_dirs)"
$CLIPXD beautify sync6.mp4 --out sweep1.mp4 >/dev/null 2>&1
[ "$(scratch_dirs)" -eq 1 ] && ok "a fresh render does NOT touch it (it could be concurrent)" \
  || bad "the sweep ate a dir that could still be an in-flight render"
find "$TMPDIR" -maxdepth 1 -name 'clipxd-beautify-*' -exec touch -d '7 hours ago' {} +
$CLIPXD beautify sync6.mp4 --out sweep2.mp4 >/dev/null 2>&1
[ "$(scratch_dirs)" -eq 0 ] && ok "once stale, the next render reclaims it" \
  || bad "$(scratch_dirs) stale scratch dir(s) survived — the leak is unbounded again"

echo "== 15. three simultaneous renders of ONE clip get their own scratch files =="
if [ ! -x "$CLIPXD_WEB" ]; then
  echo "  SKIP  no clipxd-web at $CLIPXD_WEB (cargo build -p clipxd-web)"
else
  port=$((18000 + RANDOM % 2000))
  mkdir -p clips/clp_conc && cp textured.mp4 clips/clp_conc/video.mp4
  # Run both binaries out of one throwaway dir. `clipxd_bin()` resolves the renderer relative to
  # clipxd-web's own path, preferring ../release/clipxd — so run in place and a stale release
  # build from weeks ago is what actually gets tested, which is how this case first "passed"
  # while measuring the wrong binary entirely.
  mkdir -p bin && cp "$CLIPXD" "$CLIPXD_WEB" bin/
  bin/clipxd-web clips --port "$port" >web.log 2>&1 & web=$!
  for _ in $(seq 40); do curl -sf "http://127.0.0.1:$port/clip/clp_conc/index.json" >/dev/null 2>&1 && break; sleep 0.25; done
  # Three DIFFERENT projects for the same clip, each asking for a DIFFERENT OUTPUT LENGTH.
  # Requests 1 and 2 used to be two 1s trims that both expected 1.0s, so the exact failure this
  # case exists for — one request handed another's video — was invisible between them: a swap
  # produced the expected duration either way. 2.0 / 1.0 / 1.5 makes any pairing detectable.
  echo '{"zoom_regions":[],"edit_regions":[]}'                                              > w0.json
  echo '{"zoom_regions":[],"edit_regions":[{"kind":"trim","start":0,"end":1,"rate":1}]}'     > w1.json
  echo '{"zoom_regions":[],"edit_regions":[{"kind":"trim","start":0.5,"end":1,"rate":1}]}'   > w2.json
  reqs=()
  for i in 0 1 2; do
    curl -s -o "w$i.mp4" -w '%{http_code}' -X POST -H 'content-type: application/json' \
      --data-binary "@w$i.json" "http://127.0.0.1:$port/clip/clp_conc/render?format=mp4&mockup=false" > "w$i.code" &
    reqs+=($!)
  done
  wait "${reqs[@]}"   # not bare `wait`: the server is a background job of this shell too
  # 16: five renders of a clip with NO VIDEO. Each writes its project file before the 404, and
  # nothing sweeps `clipxd-render-*` — pre-RAII this left five orphans, up to 512 MB each.
  mkdir -p clips/clp_novideo && echo '{}' > clips/clp_novideo/index.json
  for _ in 1 2 3 4 5; do
    curl -s -o /dev/null -X POST -H 'content-type: application/json' \
      --data '{"zoom_regions":[],"edit_regions":[]}' "http://127.0.0.1:$port/clip/clp_novideo/render?format=mp4"
  done
  { kill -9 $web; wait $web; } 2>/dev/null
  # each request asked for a different length, so a swap between any two shows up in the OUTPUT
  for i in 0 1 2; do
    want=2.0; [ "$i" = 1 ] && want=1.0; [ "$i" = 2 ] && want=1.5
    if [ "$(cat "w$i.code")" = 200 ] && near "$(vdur "w$i.mp4")" $want 0.15; then
      ok "request $i: HTTP 200, ${want}s — its own project, its own file"
    else
      bad "request $i: HTTP $(cat "w$i.code"), video=$(vdur "w$i.mp4")s, wanted ${want}s"
    fi
  done
  left=$(find "$TMPDIR" -maxdepth 1 -name 'clipxd-render-*' | wc -l)
  [ "$left" -eq 0 ] && ok "no render scratch files left behind (404s and 200s alike)" \
    || bad "$left orphaned render scratch file(s): $(find "$TMPDIR" -maxdepth 1 -name 'clipxd-render-*' | head -3)"
fi

echo "== 17. a slow-motion ramp over the frame budget is refused before it fills the disk =="
# rate 0 clamps to 0.1, i.e. TEN output PNGs per source frame. 121s at 30fps = 3630 source frames
# -> 36300 output frames, just past the limit. At 1440p that is ~36 GB of scratch; the point is
# that the render stops at the arithmetic, not somewhere in the middle of writing it.
mkvid vlong.mp4 121 0 || exit 2
echo '{"zoom_regions":[],"edit_regions":[{"kind":"speed","start":0,"end":121,"rate":0}]}' > slowmo.json
$CLIPXD beautify vlong.mp4 --padding 0 --project slowmo.json --out boom.mp4 >budget.log 2>&1
rc=$?
[ $rc -ne 0 ] && ok "refused (exit $rc)" || bad "exit 0 — the render was allowed to proceed"
grep -qi "output frames" budget.log && grep -q "36000" budget.log \
  && ok "the message names the limit: $(grep -o 'over the [0-9]*-frame render limit' budget.log)" \
  || bad "the refusal does not name the limit: $(tail -1 budget.log)"
[ ! -f boom.mp4 ] && ok "no partial output produced" || bad "boom.mp4 exists despite the refusal"
[ "$(scratch_dirs)" -eq 0 ] && ok "scratch dir cleaned up on the refusal" || bad "$(scratch_dirs) scratch dir(s) left"
# and a ramp INSIDE the budget still renders — a check that refuses everything is no check
echo '{"zoom_regions":[],"edit_regions":[{"kind":"speed","start":0,"end":6,"rate":0.5}]}' > half.json
$CLIPXD beautify sync6.mp4 --padding 0 --project half.json --out slow6.mp4 >/dev/null 2>&1
near "$(vdur slow6.mp4)" 12.0 0.2 && ok "0.5x still renders: $(vdur slow6.mp4)s" \
  || bad "the budget refused a legitimate 0.5x ramp: $(vdur slow6.mp4)s"

echo "== 18. a failing ffmpeg reports what ffmpeg said, not just \"ffmpeg failed\" =="
$CLIPXD beautify sync6.mp4 --padding 0 --out /nonexistent-dir-xyz/out.mp4 >ffail.log 2>&1
rc=$?
[ $rc -ne 0 ] && ok "exit $rc" || bad "exit 0 writing into a directory that does not exist"
grep -qi "No such file or directory" ffail.log \
  && ok "ffmpeg's own words reached the caller" \
  || bad "no ffmpeg diagnostic in the error: $(tail -2 ffail.log)"
grep -q "command: " ffail.log && ok "the failing command line is in the error" \
  || bad "no command line in the error — a malformed filtergraph is still unreproducible"

echo
[ $fail -eq 0 ] && echo "ALL CHECKS PASSED" || echo "SOME CHECKS FAILED"
exit $fail
