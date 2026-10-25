#!/usr/bin/env bash
# Smoke test for a release binary: batch-render a small animated scene without a window.
# usage: tools/ci_smoke.sh <path-to-rust_maya binary> <output dir>
set -euo pipefail
BIN="$1"
OUT="$2"
mkdir -p "$OUT"
SCENE="$OUT/smoke.rma"
echo '{"nodes":[],"next_id":1,"layers":[],"anim_start":1,"anim_end":24,"play_start":1,"play_end":24,"fps":24,"time":1}' > "$SCENE"
MEL='polyCube -w 14 -h 0.2 -d 8 -n floor; move 0 -1.1 0 floor;
polySphere -r 0.7 -n red; materialPreset -p plastic -rgb 0.9 0.15 0.1 red;
setKeyframe -at tx -t 1 -v -6 red; setKeyframe -at tx -t 24 -v 6 red;
polyTorus -r 1 -sr 0.25 -n ring; materialPreset -p gold ring; move 2.5 0.2 1.5 ring;
setKeyframe -at ry -t 1 -v 0 ring; setKeyframe -at ry -t 24 -v 720 ring;
camera -n shotCam -fl 35 -p 0 3 10 -aim 0 0 0; areaLight -w 3 -h 3 -i 6 -p 0 6 3 -aim 0 0 0'
MEL="${MEL//$'\n'/ }"

"$BIN" --version
"$BIN" --help > "$OUT/help.txt"
grep -q -- "--render" "$OUT/help.txt"

# A still with motion blur and every pass, then a short EXR sequence.
"$BIN" --render "$SCENE" -mel "$MEL" -f 12 -x 320 -y 180 -spp 16 -env 0.4 -mb on -shutter 1 -passes -o "$OUT/still.png"
"$BIN" --render "$SCENE" -mel "$MEL" -s 1 -e 3 -x 160 -y 90 -spp 4 -o "$OUT/seq/frame.exr"

for f in still_beauty.png still_albedo.png still_depth.png still_diffuse_direct.png still_diffuse_indirect.png \
         still_specular.png still_emission.png seq/frame.0001.exr seq/frame.0002.exr seq/frame.0003.exr; do
  [ -s "$OUT/$f" ] || { echo "missing output: $f"; exit 1; }
done

# Bad arguments exit with 2, a missing scene with 1.
set +e
"$BIN" --render "$SCENE" -no-such-flag >/dev/null 2>&1; c1=$?
"$BIN" --render "$OUT/missing.rma" >/dev/null 2>&1; c2=$?
set -e
[ "$c1" = 2 ] || { echo "bad flag exited $c1, expected 2"; exit 1; }
[ "$c2" = 1 ] || { echo "missing scene exited $c2, expected 1"; exit 1; }
echo "smoke test passed"
