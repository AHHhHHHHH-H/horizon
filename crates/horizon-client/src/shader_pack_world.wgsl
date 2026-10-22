// Horizon shaders v1, world hooks (docs/shader-packs.md, `[gbuffer]`): after
// Horizon's own world shader (terrain.wgsl or entity.wgsl) and
// shader_pack_common.wgsl, before the pack's hooks. The pack's resources are
// group 2; groups 0 and 1 are Horizon's.
@group(2) @binding(0) var<uniform> frame: Frame;
@group(2) @binding(1) var shadow_map: texture_depth_2d;
@group(2) @binding(2) var shadow_sampler: sampler_comparison;
// One byte per sprite layer: its MATERIAL_*.
@group(2) @binding(3) var<storage, read> materials: array<u32>;
@group(2) @binding(4) var repeat_sampler: sampler;

fn material_of(layer: u32) -> u32 {
    let i = layer & 0x7fffu;
    if (i >> 2u) >= arrayLength(&materials) { return MATERIAL_TERRAIN; }
    return (materials[i >> 2u] >> ((i & 3u) * 8u)) & 255u;
}

// What a vertex hook (`surface_vertex`) gets.
struct SurfaceVertex {
    // Camera-relative position (add frame.camera_position.xyz for the world one).
    world: vec3<f32>,
    // The vertex's texture coordinates inside its sprite (0..1; 0 at the top).
    uv: vec2<f32>,
    material: u32,
    block_light: f32,
    sky_light: f32,
}
// What a fragment hook (`surface_color`, `surface_normal`) gets.
struct Surface {
    // The texture times the vertex colour (biome tint, vanilla's face shade
    // and ambient occlusion); alpha as the texture's.
    albedo: vec4<f32>,
    // Vanilla's colour: albedo times the lightmap, before fog.
    lit: vec4<f32>,
    // Vanilla's colour with its fog: what the frame shows without a pack.
    vanilla: vec4<f32>,
    // Camera-relative position.
    world: vec3<f32>,
    // World-space face normal, towards the viewer.
    normal: vec3<f32>,
    block_light: f32,
    sky_light: f32,
    material: u32,
    // @builtin(position) of the fragment.
    screen: vec4<f32>,
    // Drawn in the translucent pass (water, glass, ice...).
    translucent: bool,
}
// What a sky hook (`sky_color`) gets, for every sky pixel.
struct SkyPixel {
    // World-space direction of the view ray.
    direction: vec3<f32>,
    // Vanilla's sky colour there, with its fog and sunrise.
    vanilla: vec4<f32>,
    screen: vec4<f32>,
}
struct PackOut {
    @location(0) color: vec4<f32>,
    @location(1) gbuffer: vec4<u32>,
}
fn gbuffer_value(normal: vec3<f32>, block_light: f32, sky_light: f32, material: u32) -> vec4<u32> {
    let n = vec2<u32>(round(encode_normal(normal) * 65535.0));
    let light = (u32(round(saturate(block_light) * 255.0)) << 8u) | u32(round(saturate(sky_light) * 255.0));
    return vec4(n, light, material);
}
// The face normal from the position's screen derivatives: exact for flat faces.
fn face_normal(world: vec3<f32>) -> vec3<f32> {
    return normalize(cross(dpdy(world), dpdx(world)));
}
