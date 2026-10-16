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

# check NAME MODEL "OPTIONS" "X,Y:CONDITION"... "Map,X,Y:CONDITION"...
# Each CONDITION is an awk expression over r, g, b and a, the 8-bit values of a pixel of the
# render (X,Y) or of the exported map whose name ends with Map (Map,X,Y; after the others).
check() {
    local name=$1 model=$2 options=$3
    shift 3
    local probes=() conditions=() exported=() exported_conditions=() spec
    for spec in "$@"; do
        if [[ $spec =~ ^[A-Za-z] ]]; then
            exported+=("xprobe=${spec%%:*}")
            exported_conditions+=("${spec#*:}")
        else
            probes+=("probe=${spec%%:*}")
            conditions+=("${spec#*:}")
        fi
    done
    probes+=("${exported[@]}")
    conditions+=("${exported_conditions[@]}")
    local output
    # shellcheck disable=SC2086 # options are several words
    if ! output=$("$snapshot" "$model" "$out/$name.png" $options "${probes[@]}" 2>&1); then
        echo "FAIL $name: the snapshot failed"
        echo "$output" | grep -v '^ \+[0-9]*: \|^ \+at ' | tail -n 15
        failures=$((failures + 1))
        return
    fi
    local lines failed=0 i=0 x y r g b a
    mapfile -t lines < <(grep '^probe ' <<<"$output")
    if [ "${#lines[@]}" -ne "${#conditions[@]}" ]; then
        echo "FAIL $name: expected ${#conditions[@]} probes, got ${#lines[@]}"
        failures=$((failures + 1))
        return
    fi
    for i in "${!lines[@]}"; do
        read -r _ x y r g b a <<<"${lines[$i]}"
        a=${a:-255}
        if ! awk -v r="$r" -v g="$g" -v b="$b" -v a="$a" "BEGIN { exit !(${conditions[$i]}) }"; then
            echo "FAIL $name: pixel $x,$y is $r $g $b $a, expected ${conditions[$i]}"
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
check paint-planar sphere "paint align=planar display=basecolor" "640,400:$red" "640,250:$grey"
check paint-camera sphere "paint align=camera display=basecolor" "640,400:$red" "640,250:$grey"
check paint-follow sphere "paint follow jitter=0.2 display=basecolor" "640,400:$red" "640,250:$grey"
check polyfill-mesh sphere "polyfill=mesh display=basecolor" "640,400:b > 200 && r < 60" "500,330:b > 200 && r < 60"
check polyfill-triangle sphere "polyfill=triangle display=basecolor" "500,330:$grey"
check stencil sphere "paint stencil=half size=20 display=basecolor" "500,400:$red" "700,400:$grey"
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

# Projects: a painted model comes back from its project file.
check project-save sphere "paint save=$out/painted.ink3d display=basecolor" "640,400:$red"
check project-open "$out/painted.ink3d" "display=basecolor" "640,400:$red" "640,250:$grey"

# Export of a quad whose UV island covers the middle of the texture (128 to 384 of 512).
island="res=512 yaw=0 fill=0.2,0.5,0.9"
fill_blue='r == 51 && g == 127 && b == 229 && a == 255'
check export-infinite "$data/uv_island.obj" "$island export=document_channels_with_alpha" \
    "BaseColor,256,256:$fill_blue" "BaseColor,10,10:$fill_blue"
check export-background "$data/uv_island.obj" \
    "$island export=document_channels_with_alpha padding=color dilation=16" \
    "BaseColor,120,256:$fill_blue" "BaseColor,10,10:r == 188 && g == 188 && a == 255" \
    "Roughness,10,10:r == 77"
check export-transparent "$data/uv_island.obj" \
    "$island export=document_channels_with_alpha padding=transparent dilation=16" \
    "BaseColor,120,256:$fill_blue" "BaseColor,10,10:a == 0" "Roughness,10,10:a == 0" \
    "Roughness,256,256:r == 76 && a == 255"
# Normal maps: OpenGL for Blender, Unity, Godot and glTF, DirectX for Unreal.
ridge="res=512 yaw=0 fill=1,1,1 normalmap=$data/ridge_normal_opengl.png opengl"
check export-opengl "$data/uv_island.obj" "$ridge export=pbr_metallic_roughness" \
    "Normal,256,200:g > 75 && g < 95 && b > 240"
check export-directx "$data/uv_island.obj" "$ridge export=unreal_engine_packed" \
    "Normal,256,200:g > 160 && g < 180 && b > 240"
# Packing: Unity's mask map holds metallic, AO, detail mask and smoothness.
check export-unity-hdrp "$data/uv_island.obj" \
    "$island rough=0.2 metal=1 export=unity_hdrp_metallic" \
    "MaskMap,256,256:r == 255 && g == 255 && b == 255 && a == 204"
# Float files hold linear colors; 16-bit files are written too.
check export-exr "$data/uv_island.obj" "$island export=pbr_metallic_roughness format=exr depth=16f" \
    "BaseColor,256,256:r < 15 && b > 190"
check export-png16 "$data/uv_island.obj" "$island export=pbr_metallic_roughness depth=16" \
    "BaseColor,256,256:r == 51 && b == 229"

# Baking: mesh maps from the model and from a high-poly. Specs are "map,x,y:CONDITION".
check_bake() {
    local name=$1 model=$2 options=$3
    shift 3
    local probes=() conditions=() spec output
    for spec in "$@"; do
        probes+=("bprobe=${spec%%:*}")
        conditions+=("${spec#*:}")
    done
    # shellcheck disable=SC2086
    if ! output=$("$snapshot" "$model" "$out/$name.png" $options "${probes[@]}" 2>&1); then
        echo "FAIL $name: the snapshot failed"
        echo "$output" | tail -n 15
        failures=$((failures + 1))
        return
    fi
    local failed=0 i line x y r g b map
    for i in "${!probes[@]}"; do
        # The snapshot prints the probes map by map, not in the order they were asked for.
        IFS=, read -r map x y <<<"${probes[$i]#bprobe=}"
        line=$(grep "^bprobe $map $x $y " <<<"$output" | head -n 1)
        if [ -z "$line" ]; then
            echo "FAIL $name: no probe $map $x $y"
            failed=1
            continue
        fi
        read -r _ _ _ _ r g b <<<"$line"
        if ! awk -v r="$r" -v g="$g" -v b="$b" "BEGIN { exit !(${conditions[$i]}) }"; then
            echo "FAIL $name: $map texel $x,$y is $r $g $b, expected ${conditions[$i]}"
            failed=1
        fi
    done
    if [ "$failed" -eq 0 ]; then
        echo "ok   $name"
    else
        failures=$((failures + 1))
    fi
}

# A floor meeting a wall: occlusion falls towards the crease (the bottom of the islands), and
# the crease is concave while the border of an island is flat.
check_bake bake-corner "$data/bake_corner.obj" "bake=ambient_occlusion,curvature,position,world_normal res=128 rays=64"     "ambient_occlusion,32,5:r > 240" "ambient_occlusion,32,126:r < 190"     "curvature,32,64:r > 118 && r < 138" "curvature,32,127:r < 100"     "position,32,64:b > 0" "world_normal,32,64:g > 250 && r > 120 && r < 135"
# The edge of a cube is convex.
check_bake bake-cube cube "bake=curvature res=512" "curvature,0,100:r > 240" "curvature,100,100:r > 118 && r < 138"
# A flat quad against a bumpy copy: the bump's slopes tilt the normal, its top is high.
check_bake bake-high "$data/bake_low.obj" "bake=normal,height,id res=128 bakehigh=$data/bake_high.obj front=0.15 rear=0.05"     "normal,5,5:r > 120 && r < 135 && b > 250" "normal,64,100:g < 110" "normal,64,28:g > 150"     "height,64,64:r > 180" "height,5,5:r < 90" "id,64,64:r > 200"

# Generators read the baked maps: edge wear on a cube is red at its edges and not on its faces.
check generator-edge cube "bake=curvature,position res=512 generator=edge_wear display=basecolor" \
    "704,400:$red" "500,400:$grey" "600,300:$grey"
# Without baked maps a generator still runs: noise falls back to the UV space and varies.
check generator-noise sphere "generator=noise gparam=scale:4 gparam=contrast:6 display=basecolor" \
    "500,400:r > 100" "640,300:r > 100"

if [ "$failures" -gt 0 ]; then
    echo "$failures check(s) failed; the renders are in $out"
    exit 1
fi
echo "All GPU checks passed."
