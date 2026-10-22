// Horizon shaders v1: the terrain and sky entry points a `[gbuffer]` pack's
// world pass uses instead of terrain.wgsl's, and the shadow pass. They run
// Horizon's own vertex and texture code, then the pack's hooks.
struct PackTerrainOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    // Vanilla's lit vertex colour (terrain_vertex's).
    @location(2) color: vec4<f32>,
    @location(3) spherical: f32,
    @location(4) cylindrical: f32,
    @location(5) tint: vec4<f32>,
    @location(6) world: vec3<f32>,
    // Block, sky light 0..1.
    @location(7) light: vec2<f32>,
    @location(8) @interpolate(flat) material: u32,
}
fn pack_terrain_vertex(position: vec3<f32>, uv: vec2<f32>, layer: u32, color: vec4<f32>,
    light: vec2<u32>, origin: vec3<f32>) -> PackTerrainOut {
    let vanilla = terrain_vertex(position, uv, layer, color, light, origin);
    let pos = position + origin;
    var v: SurfaceVertex;
    v.world = pos;
    v.uv = uv;
    v.material = material_of(layer);
    v.block_light = saturate(max(f32(light.y), dynamic_block_light(pos)) / 240.0);
    v.sky_light = saturate(f32(light.x) / 240.0);
    let world = surface_vertex(v);
    var out: PackTerrainOut;
    out.clip = globals.view_proj * vec4(world, 1.0);
    out.uv = uv;
    out.layer = layer;
    out.color = vanilla.color;
    out.spherical = vanilla.spherical;
    out.cylindrical = vanilla.cylindrical;
    out.tint = color;
    out.world = world;
    out.light = vec2(v.block_light, v.sky_light);
    out.material = v.material;
    return out;
}
@vertex fn vs_terrain_pack(
    @location(0) position: vec3<f32>, @location(1) uv: vec2<u32>,
    @location(2) layer: u32, @location(3) color: vec4<f32>, @location(4) light: vec2<u32>,
    @location(5) origin: vec4<f32>) -> PackTerrainOut {
    return pack_terrain_vertex(position, terrain_uv(uv, origin.w), layer, color, light, origin.xyz);
}
@vertex fn vs_terrain_flat_pack(
    @builtin(vertex_index) vertex: u32,
    @location(0) position: vec3<f32>, @location(1) uv: vec2<u32>,
    @location(2) layer: u32, @location(3) color: vec4<f32>, @location(4) light: vec2<u32>
) -> PackTerrainOut {
    let origin = slot_origins[block_slots[vertex >> 8u]];
    return pack_terrain_vertex(position, terrain_uv(uv, origin.w), layer, color, light, origin.xyz);
}
fn pack_surface(in: PackTerrainOut, texel: vec4<f32>, color: vec4<f32>, translucent: bool) -> Surface {
    var s: Surface;
    s.albedo = vec4(texel.rgb * in.tint.rgb, texel.a * in.tint.a);
    s.lit = color;
    s.vanilla = apply_fog(color, in.spherical, in.cylindrical);
    s.world = in.world;
    s.normal = face_normal(in.world);
    s.block_light = in.light.x;
    s.sky_light = in.light.y;
    s.material = in.material;
    s.screen = in.clip;
    s.translucent = translucent;
    return s;
}
@fragment fn fs_solid_pack(in: PackTerrainOut) -> PackOut {
    let texel = sprite(in.uv, in.layer & 0x7fffu);
    let color = texel * in.color;
    var s = pack_surface(in, texel, vec4(color.rgb, 1.0), false);
    if color.a < 0.5 && (in.layer & 0x8000u) == 0u { discard; }
    s.albedo.a = 1.0;
    s.vanilla.a = 1.0;
    var out: PackOut;
    out.color = vec4(surface_color(s).rgb, 1.0);
    out.gbuffer = gbuffer_value(surface_normal(s), s.block_light, s.sky_light, s.material);
    return out;
}
@fragment fn fs_translucent_pack(in: PackTerrainOut) -> PackOut {
    let texel = sprite(in.uv, in.layer & 0x7fffu);
    let color = texel * in.color;
    let s = pack_surface(in, texel, color, true);
    if color.a < 0.01 { discard; }
    var out: PackOut;
    out.color = surface_color(s);
    out.gbuffer = gbuffer_value(surface_normal(s), s.block_light, s.sky_light, s.material);
    return out;
}
@fragment fn fs_sky_pack(in: SkyOut) -> @location(0) vec4<f32> {
    var p: SkyPixel;
    p.direction = view_ray(in.ndc);
    p.vanilla = sky_pixel(in.ndc);
    p.screen = in.clip;
    return vec4(sky_color(p).rgb, 1.0);
}

// The shadow pass: solid and cutout terrain from the light, through the
// pack's vertex hook (so waving plants cast waving shadows) and the
// distortion of shadow_coords.
struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
}
@vertex fn vs_shadow(
    @location(0) position: vec3<f32>, @location(1) uv: vec2<u32>,
    @location(2) layer: u32, @location(3) color: vec4<f32>, @location(4) light: vec2<u32>,
    @location(5) origin: vec4<f32>) -> ShadowOut {
    let pos = position + origin.xyz;
    var v: SurfaceVertex;
    v.world = pos;
    v.uv = terrain_uv(uv, origin.w);
    v.material = material_of(layer);
    v.block_light = saturate(f32(light.y) / 240.0);
    v.sky_light = saturate(f32(light.x) / 240.0);
    let clip = frame.shadow_view_projection * vec4(surface_vertex(v), 1.0);
    var out: ShadowOut;
    out.clip = vec4(shadow_distort(clip.xy), clip.z, 1.0);
    out.uv = v.uv;
    out.layer = layer;
    return out;
}
@fragment fn fs_shadow(in: ShadowOut) {
    let texel = sprite(in.uv, in.layer & 0x7fffu);
    if texel.a < 0.5 && (in.layer & 0x8000u) == 0u { discard; }
}
