#!/usr/bin/env bash
# Renders the snapshot example's standard cases on the GPU and checks a few pixels of each.
# Catches what unit tests can't: wgpu validation errors, shaders that don't compile on a
# real driver, and wrong pixels from painting, undo, masks, blending and projections.
# Runs on software Vulkan (lavapipe) in CI; anywhere with a Vulkan driver locally.
#
#   scripts/gpu-smoke.sh            # renders go to target/gpu-smoke/
set -uo pipefail
cd "$(dirname "$0")/.."

cargo build --release -p ink_render --example snapshot || exit 1
snapshot=target/release/examples/snapshot
data=crates/ink_render/examples/testdata
out=target/gpu-smoke
mkdir -p "$out"
failures=0

# check NAME MODEL "OPTIONS" "X,Y:CONDITION"...
# Each CONDITION is an awk expression over r, g and b, the pixel's 8-bit sRGB values.
check() {
    local name=$1 model=$2 options=$3
    shift 3
    local probes=() conditions=() spec
    for spec in "$@"; do
        probes+=("probe=${spec%%:*}")
        conditions+=("${spec#*:}")
    done
    local output
    # shellcheck disable=SC2086 # options are several words
    if ! output=$("$snapshot" "$model" "$out/$name.png" $options "${probes[@]}" 2>&1); then
        echo "FAIL $name: the snapshot failed"
        echo "$output" | grep -v '^ \+[0-9]*: \|^ \+at ' | tail -n 15
        failures=$((failures + 1))
        return
    fi
    local lines failed=0 i=0 x y r g b
    mapfile -t lines < <(grep '^probe ' <<<"$output")
    if [ "${#lines[@]}" -ne "${#conditions[@]}" ]; then
        echo "FAIL $name: expected ${#conditions[@]} probes, got ${#lines[@]}"
        failures=$((failures + 1))
        return
    fi
    for i in "${!lines[@]}"; do
        read -r _ x y r g b <<<"${lines[$i]}"
        if ! awk -v r="$r" -v g="$g" -v b="$b" "BEGIN { exit !(${conditions[$i]}) }"; then
            echo "FAIL $name: pixel $x,$y is $r $g $b, expected ${conditions[$i]}"
            failed=1
        fi
    done
    if [ "$failed" -eq 0 ]; then
        echo "ok   $name"
    else
        failures=$((failures + 1))
    fi
}

# Colors of the 3D view with display=basecolor: the red paint of `paint`, the grey where
# nothing is painted (Painter's default base color), the blue of fill=0.2,0.5,0.9.
red='r > 200 && g < 50 && b < 50'
grey='r > 170 && r < 205 && g > 170 && g < 205 && b > 170 && b < 205'
blue='r < 80 && g > 110 && g < 145 && b > 210'
# The F test image: its red bars, rotated 30° counterclockwise.
f_red='r > 180 && g < 60 && b < 60'

# Painting, erasing and undo (the stroke is horizontal, the eraser vertical, both through
# the middle).
check paint sphere "paint display=basecolor" "640,400:$red" "640,250:$grey"
check paint-live sphere "paint live=10 display=basecolor" "500,400:$red"
check erase sphere "paint erase display=basecolor" "640,400:$grey" "500,400:$red"
check undo sphere "paint undo display=basecolor" "640,400:$grey" "500,400:$grey"
check undo-live sphere "paint live=10 undo display=basecolor" "500,400:$grey"
check paint-4k sphere "res=4096 size=60 paint display=basecolor" "640,400:$red"
check paint-uv sphere "paint uv 2d" "640,400:b > 190 && r < 140" "400,650:r > 160 && b < 200"
check paint-height sphere "paint height display=height" "500,400:r > 240" "640,250:r < 10"

# Occlusion: the front quad hides the middle of the back one from the stroke.
check occlusion "$data/two_planes.obj" "paint set=Back only=Back yaw=0 display=basecolor" \
    "640,400:$grey" "450,400:$red"
check no-depth "$data/two_planes.obj" "paint nodepth set=Back only=Back yaw=0 display=basecolor" \
    "640,400:$red"

# Masks: the strokes paint the fill's black mask; levels invert it.
check mask sphere "fill=0.2,0.5,0.9 mask paint display=basecolor" "500,400:$blue" "640,250:$grey"
check mask-levels sphere "fill=0.2,0.5,0.9 mask levels paint display=basecolor" \
    "500,400:$grey" "640,250:$blue"

# Blending and folders.
check multiply sphere "color=1,1,1 fill=0.9,0.2,0.2 folder blend=multiply display=basecolor" \
    "640,400:r > 215 && g > 40 && g < 65 && b > 40 && b < 65"
check passthrough sphere \
    "color=1,1,1 fill=0.9,0.2,0.2 folder passthrough blend=multiply opacity=0.5 display=basecolor" \
    "640,400:r > 235 && g > 180 && g < 200"

# Normal maps at half opacity: Normal Map Detail as strong as Normal (green is the tilt
# across the ridge).
for mode in normal normalmapdetail; do
    check "normal-$mode" cube \
        "fill=1,1,1 normalmap=$data/ridge_normal_opengl.png opengl nblend=$mode nopacity=0.5 2d display=normal" \
        "640,250:g > 146 && g < 158" "640,332:g > 97 && g < 109"
done
# Turned by 90°, a normal map and the height map it was made from tilt the same way (red
# is the tilt across the ridge, now vertical).
check height-turned cube "fill=1,1,1 heightmap=$data/ridge_height.png rotate=90 2d display=normal" \
    "490,400:r < 122" "572,400:r > 142"
check normal-turned cube \
    "fill=1,1,1 normalmap=$data/ridge_normal_opengl.png opengl rotate=90 2d display=normal" \
    "490,400:r < 115" "572,400:r > 155"
check height-view cube "fill=1,1,1 heightmap=$data/ridge_height.png 2d display=height" \
    "640,291:r > 240" "640,600:r < 10"

# Projections turn images counterclockwise.
check uv-rotation cube "fill=1,1,1 image=$data/letter_f.png rotate=30 2d" \
    "420,250:$f_red" "640,600:$f_red"
check triplanar-rotation cube \
    "fill=1,1,1 image=$data/letter_f.png triplanar rotate=30 yaw=0 display=basecolor" \
    "600,280:$f_red" "520,320:$f_red"

if [ "$failures" -gt 0 ]; then
    echo "$failures check(s) failed; the renders are in $out"
    exit 1
fi
echo "All GPU checks passed."
