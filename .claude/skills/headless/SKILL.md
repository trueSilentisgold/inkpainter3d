---
name: headless
description: Render, run or screenshot inkpainter3d without a display or GPU (software Vulkan, the snapshot example and its options, Xvfb, headless Weston, xdotool). Read before checking a renderer or UI change in a container.
---

# Seeing the app without a display

The container has no GPU or screen; software Vulkan (lavapipe) works. Xvfb and ImageMagick's
`import` are preinstalled.

```sh
apt-get install -y --no-install-recommends mesa-vulkan-drivers libvulkan1 \
  libxkbcommon-x11-0 libxcursor1 libxrandr2 libxi6 libx11-xcb1 xdotool
cargo run --release -p ink_render --example snapshot -- model.fbx out.png display=materialid
```

The snapshot options are listed at the top of `crates/ink_render/examples/snapshot.rs`; the
model may also be `sphere` or `cube`. `display=` takes a display mode label without spaces,
case-insensitive (`basecolor`, `ambientocclusion`, …). It also paints test strokes through
the real paint pipeline (`paint`, `erase`, `uv`, `live=`, `undo`, `nocull`, `nodepth`,
`set=`, `only=`), builds layer stacks to check the compositor (`fill=`, `image=`,
`normalmap=`, `heightmap=`, `triplanar`, `mask`, `levels`, `blend=`, `opacity=`, `folder`,
`passthrough`, `height`, `rotate=`, `nblend=`, `nopacity=`), renders the 2D view (`2d`),
saves and opens projects (`save=file.ink3d`; the model may be a `.ink3d`), exports a
template (`export=unity_hdrp_metallic`, `format=`, `depth=`, `padding=`, `dilation=`) and
prints pixels for scripted checks (`probe=x,y` of the render, `xprobe=Map,x,y` of an
exported map). A small OBJ with two parallel quads in
different materials is the quickest occlusion test.

To check that the app starts on Wayland, install `weston`, run
`weston --backend=headless --socket=wl-test` with `XDG_RUNTIME_DIR` set to a private
directory and start the app with `WAYLAND_DISPLAY=wl-test DISPLAY=`. Headless Weston has no
tablet, so pens can't be tested here.

To drive the real UI, start `Xvfb :99`, run the app with `DISPLAY=:99` and the model as its
first argument (the file dialogs do nothing here: no xdg portal or zenity), and screenshot
with `import -window root`. With xdotool: call `xdotool windowfocus <id>` first or modifier
keys never arrive, and sleep between pointer moves (llvmpipe frames are slow, egui misses
drags that start and end in one frame). Don't `pkill -f` a pattern that also matches your
own shell command; use `pgrep -x inkpainter3d | xargs -r kill`.
