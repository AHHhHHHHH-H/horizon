// Horizon shaders v1: the entity entry points a `[gbuffer]` pack's opaque
// world pass uses instead of entity.wgsl's: Horizon's entity code, then the
// pack's hooks.
struct PackEntityOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) spherical: f32,
    @location(3) cylindrical: f32,
    @location(5) overlay_color: vec4<f32>,
    @location(4) @interpolate(flat) overlay: u32,
    @location(6) @interpolate(flat) wrap: vec2<f32>,
    @location(7) proj: vec4<f32>,
    @location(8) back_color: vec4<f32>,
    @location(9) light_color: vec4<f32>,
    @location(10) world: vec3<f32>,
    // Block, sky light 0..1.
    @location(11) light: vec2<f32>,
    // xyz: the vertex normal, w: 0 when it has none.
    @location(12) normal: vec4<f32>,
}
@vertex fn vs_entity_pack(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>,
    @location(2) overlay: u32, @location(3) color: vec4<f32>, @location(4) light: vec4<u32>, @location(5) normal: vec4<f32>) -> PackEntityOut {
    let o = entity_vertex(position, uv, overlay, color, light, normal);
    var out: PackEntityOut;
    out.clip = o.clip;
    out.uv = o.uv;
    out.color = o.color;
    out.spherical = o.spherical;
    out.cylindrical = o.cylindrical;
    out.overlay_color = o.overlay_color;
    out.overlay = o.overlay;
    out.wrap = o.wrap;
    out.proj = o.proj;
    out.back_color = o.back_color;
    out.light_color = o.light_color;
    out.world = position;
    out.light = saturate(vec2(max(f32(light.y), dynamic_block_light(position)), f32(light.x)) / 240.0);
    out.normal = vec4(normal.xyz, abs(normal.w));
    return out;
}
@fragment fn fs_entity_pack(in: PackEntityOut, @builtin(front_facing) front: bool) -> PackOut {
    var o: Out;
    o.clip = in.clip;
    o.uv = in.uv;
    o.color = in.color;
    o.spherical = in.spherical;
    o.cylindrical = in.cylindrical;
    o.overlay_color = in.overlay_color;
    o.overlay = in.overlay;
    o.wrap = in.wrap;
    o.proj = in.proj;
    o.back_color = in.back_color;
    o.light_color = in.light_color;
    var s: Surface;
    s.world = in.world;
    let derived = face_normal(in.world);
    let vertex_normal = select(-in.normal.xyz, in.normal.xyz, front);
    s.normal = select(derived, normalize(vertex_normal), in.normal.w > 0.0 && dot(in.normal.xyz, in.normal.xyz) > 0.0);
    s.albedo = textureSample(atlas, atlas_sampler, in.uv) * select(in.back_color, in.color, front);
    let vanilla = entity_color(o, front);
    s.lit = vanilla;
    s.vanilla = vanilla;
    s.block_light = in.light.x;
    s.sky_light = in.light.y;
    s.material = MATERIAL_ENTITY;
    s.screen = in.clip;
    s.translucent = false;
    var out: PackOut;
    out.color = vec4(surface_color(s).rgb, 1.0);
    out.gbuffer = gbuffer_value(surface_normal(s), s.block_light, s.sky_light, s.material);
    return out;
}
