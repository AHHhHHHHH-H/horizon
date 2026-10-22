// Horizon shader pack prelude for full-screen stages (docs/shader-packs.md):
// after shader_pack_common.wgsl, before the stage's source. Bindings, the
// full-screen vertex shader and helpers that hide the renderer's reversed
// depth. The pack's own targets and textures follow in group 1.
@group(0) @binding(0) var scene_color: texture_2d<f32>;
@group(0) @binding(1) var scene_depth: texture_depth_2d;
@group(0) @binding(2) var previous: texture_2d<f32>;
@group(0) @binding(3) var linear_sampler: sampler;
@group(0) @binding(4) var nearest_sampler: sampler;
@group(0) @binding(5) var<uniform> frame: Frame;
// api 2. The G-buffer (`[gbuffer]` packs; zero otherwise): read with gbuffer_at.
@group(0) @binding(6) var gbuffer: texture_2d<u32>;
// The depth before translucent terrain, water and clouds (= scene_depth in
// packs without `[gbuffer]`).
@group(0) @binding(7) var opaque_depth: texture_depth_2d;
// The `[shadow]` map (1 nearest the light) and its comparison sampler: use
// shadow_lit.
@group(0) @binding(8) var shadow_map: texture_depth_2d;
@group(0) @binding(9) var shadow_sampler: sampler_comparison;
// Linear, repeating: for noise textures.
@group(0) @binding(10) var repeat_sampler: sampler;

struct StageIn {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
@vertex fn horizon_stage_vertex(@builtin(vertex_index) i: u32) -> StageIn {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: StageIn;
    out.position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2(p.x, 1.0 - p.y);
    return out;
}

// The world's depth at `uv`: 1 at the near plane, 0 for the sky.
fn depth_at(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(scene_depth, nearest_sampler, uv, 0);
}
fn is_sky(depth: f32) -> bool {
    return depth <= 0.0;
}
// Blocks from the eye along the view axis.
fn linear_depth(depth: f32) -> f32 {
    let near = frame.view_params.x;
    let far = frame.view_params.y;
    return near * far / ((far - near) * depth + near);
}
fn view_position(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let v = frame.inverse_projection * vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    return v.xyz / v.w;
}
// World position relative to the eye (add frame.camera_position.xyz for the
// absolute one).
fn world_offset(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    return (frame.inverse_view * vec4(view_position(uv, depth), 1.0)).xyz;
}

// The pixel's G-buffer: what the world pass knew about the frontmost surface.
struct GBuffer {
    // World-space normal; zero for the sky.
    normal: vec3<f32>,
    // Block and sky light, 0..1 (vanilla's lightmap coordinates).
    block_light: f32,
    sky_light: f32,
    // MATERIAL_*.
    material: u32,
}
fn gbuffer_at(uv: vec2<f32>) -> GBuffer {
    let size = frame.world.xy;
    let g = textureLoad(gbuffer, vec2<i32>(clamp(uv * size, vec2(0.0), size - 1.0)), 0);
    var out: GBuffer;
    out.material = g.w;
    out.normal = select(vec3(0.0), decode_normal(vec2<f32>(g.xy) / 65535.0), g.w != 0u);
    out.block_light = f32(g.z >> 8u) / 255.0;
    out.sky_light = f32(g.z & 255u) / 255.0;
    return out;
}
fn opaque_depth_at(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(opaque_depth, nearest_sampler, uv, 0);
}
// Where a camera-relative world position was on screen last frame: uv and
// depth (for temporal effects).
fn previous_uv(world: vec3<f32>) -> vec3<f32> {
    let clip = frame.previous_view_projection * vec4(world + frame.camera_delta.xyz, 1.0);
    let ndc = clip.xyz / clip.w;
    return vec3(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, ndc.z);
}
