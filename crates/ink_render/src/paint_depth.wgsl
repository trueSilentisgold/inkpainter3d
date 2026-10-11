// Depth map of a stroke's camera: the distance along the view direction to the nearest
// surface, for hiding texels behind other surfaces from the brush.

struct DepthCamera {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    forward: vec4<f32>,
}

@group(0) @binding(0) var<uniform> camera: DepthCamera;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
}

@vertex
fn vs_depth(@location(0) position: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(position, 1.0);
    out.world = position;
    return out;
}

@fragment
fn fs_depth(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(dot(in.world - camera.eye.xyz, camera.forward.xyz), 0.0, 0.0, 1.0);
}
