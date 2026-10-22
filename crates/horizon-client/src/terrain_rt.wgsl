// EXPERIMENTAL ray-traced sun shadows on solid terrain (gfx/raytrace.rs, docs/raytracing.md),
// appended to terrain.wgsl behind `enable wgpu_ray_query;`. Off unless HORIZON_RT_SHADOWS=1.
struct ShadowParams {
    // xyz: towards the sun, world space; w: shadow strength (0 at night and when off).
    sun: vec4<f32>,
    // x: ray length in blocks.
    misc: vec4<f32>,
}
@group(2) @binding(0) var shadow_tlas: acceleration_structure;
@group(2) @binding(1) var<uniform> shadow: ShadowParams;
// The sky light a shadowed pixel keeps: its lightmap is read with the sky level scaled by this.
override SHADOW_SKY: f32 = 0.55;

struct TerrainRtOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) color: vec4<f32>,
    @location(3) spherical: f32,
    @location(4) cylindrical: f32,
    // The colour with the sky light dimmed, for pixels the sun doesn't reach.
    @location(5) shadow_color: vec4<f32>,
    // Camera-relative position: the TLAS places sections the same way.
    @location(6) pos: vec3<f32>,
}
@vertex fn vs_terrain_rt(
    @location(0) position: vec3<f32>, @location(1) uv: vec2<u32>,
    @location(2) layer: u32, @location(3) color: vec4<f32>, @location(4) light: vec2<u32>,
    @location(5) origin: vec4<f32>) -> TerrainRtOut {
    let t = terrain_vertex(position, terrain_uv(uv, origin.w), layer, color, light, origin.xyz);
    var out: TerrainRtOut;
    out.clip = t.clip;
    out.uv = t.uv;
    out.layer = t.layer;
    out.color = t.color;
    out.spherical = t.spherical;
    out.cylindrical = t.cylindrical;
    out.shadow_color = color * sample_lightmap(f32(light.x) / 16.0 * SHADOW_SKY, f32(light.y) / 16.0);
    out.pos = position + origin.xyz;
    return out;
}
// 1 where the sun reaches `pos` (on a face of normal `n`), 0 where terrain blocks it.
fn sun_light(pos: vec3<f32>, n: vec3<f32>) -> f32 {
    let sun = shadow.sun.xyz;
    let facing = dot(n, sun);
    // Faces turned from the sun are in their own shadow.
    if facing < -0.01 { return 0.0; }
    // Off the face, a little more far away where float precision is coarser.
    let origin = pos + n * (0.002 + length(pos) * 0.0001);
    var rq: ray_query;
    rayQueryInitialize(&rq, shadow_tlas,
        RayDesc(RAY_FLAG_TERMINATE_ON_FIRST_HIT, 0xffu, 0.001, shadow.misc.x, origin, sun));
    rayQueryProceed(&rq);
    let hit = rayQueryGetCommittedIntersection(&rq);
    return select(1.0, 0.0, hit.kind != RAY_QUERY_INTERSECTION_NONE);
}
@fragment fn fs_solid_rt(in: TerrainRtOut) -> @location(0) vec4<f32> {
    // Derivatives first, in uniform control flow: the face normal, turned to the camera.
    var n = normalize(cross(dpdx(in.pos), dpdy(in.pos)));
    if dot(n, in.pos) > 0.0 { n = -n; }
    let texel = sprite(in.uv, in.layer & 0x7fffu);
    if texel.a * in.color.a < 0.5 && (in.layer & 0x8000u) == 0u { discard; }
    var lit = in.color;
    if shadow.sun.w > 0.0 {
        let light = 1.0 - shadow.sun.w * (1.0 - sun_light(in.pos, n));
        lit = mix(in.shadow_color, in.color, light);
    }
    let color = texel * lit;
    return apply_fog(vec4(color.rgb, 1.0), in.spherical, in.cylindrical);
}
