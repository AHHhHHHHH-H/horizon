// Horizon shaders v1 (docs/shader-packs.md): what every pack shader sees,
// full-screen stages and world hooks alike. The Frame uniform only grows: new
// fields go at the end.
struct Frame {
    view: mat4x4<f32>,
    projection: mat4x4<f32>,
    inverse_view: mat4x4<f32>,
    inverse_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    resolution: vec4<f32>,
    time: vec4<f32>,
    sun: vec4<f32>,
    moon: vec4<f32>,
    sky_color: vec4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>,
    weather: vec4<f32>,
    camera_state: vec4<f32>,
    view_params: vec4<f32>,
    dimension: vec4<f32>,
    // api 2
    previous_view_projection: mat4x4<f32>,
    shadow_view_projection: mat4x4<f32>,
    camera_delta: vec4<f32>,
    shadow: vec4<f32>,
    shadow_light: vec4<f32>,
    // The world's size: width, height, 1/width, 1/height (the G-buffer's).
    world: vec4<f32>,
}

// Materials (the G-buffer's `material`, `Surface.material`): sprites are
// sorted into these by name; a pack's `[materials]` adds its own (16..255).
const MATERIAL_NONE: u32 = 0u;
const MATERIAL_TERRAIN: u32 = 1u;
const MATERIAL_LEAVES: u32 = 2u;
const MATERIAL_PLANT: u32 = 3u;
const MATERIAL_WATER: u32 = 4u;
const MATERIAL_LAVA: u32 = 5u;
const MATERIAL_GLASS: u32 = 6u;
const MATERIAL_EMISSIVE: u32 = 7u;
const MATERIAL_ENTITY: u32 = 8u;

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3(0.2126, 0.7152, 0.0722));
}
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}
fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(max(c, vec3(0.0)), vec3(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3(0.0031308));
}

// The shadow map's distortion: more texels near the eye. `frame.shadow.z`
// is the strength (0: a plain orthographic map).
fn shadow_distort(clip: vec2<f32>) -> vec2<f32> {
    let k = frame.shadow.z;
    return clip / mix(1.0, length(clip) + 0.1, k);
}
// Where a camera-relative world position lands in the shadow map: uv and
// depth (1 nearest the light). `bias` is in blocks along the normal.
fn shadow_coords(world: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    // One texel of the map in blocks, where `world` is (the distortion makes
    // texels smaller near the eye).
    let texel = 2.0 * frame.shadow.y / frame.shadow.x;
    let at = (frame.shadow_view_projection * vec4(world, 1.0)).xy;
    let scale = mix(1.0, length(at) + 0.1, frame.shadow.z);
    let p = world + normal * texel * scale * 1.5;
    let clip = frame.shadow_view_projection * vec4(p, 1.0);
    let xy = shadow_distort(clip.xy);
    return vec3(xy.x * 0.5 + 0.5, 0.5 - xy.y * 0.5, clip.z);
}
// 1 in light, 0 in shadow: four comparison taps around the texel (2x2 PCF,
// filtered by the comparison sampler). Outside the map: lit.
fn shadow_lit(world: vec3<f32>, normal: vec3<f32>) -> f32 {
    if frame.shadow.w == 0.0 { return 1.0; }
    let c = shadow_coords(world, normal);
    if any(c.xy <= vec2(0.0)) || any(c.xy >= vec2(1.0)) || c.z <= 0.0 || c.z >= 1.0 {
        return 1.0;
    }
    let o = 0.5 / frame.shadow.x;
    var lit = 0.0;
    lit += textureSampleCompareLevel(shadow_map, shadow_sampler, c.xy + vec2(-o, -o), c.z);
    lit += textureSampleCompareLevel(shadow_map, shadow_sampler, c.xy + vec2(o, -o), c.z);
    lit += textureSampleCompareLevel(shadow_map, shadow_sampler, c.xy + vec2(-o, o), c.z);
    lit += textureSampleCompareLevel(shadow_map, shadow_sampler, c.xy + vec2(o, o), c.z);
    return lit * 0.25;
}
// Octahedral normal encoding (the G-buffer stores two 16-bit numbers).
fn encode_normal(n: vec3<f32>) -> vec2<f32> {
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    let w = (1.0 - abs(p.yx)) * select(vec2(-1.0), vec2(1.0), p >= vec2(0.0));
    return select(w, p, n.z >= 0.0) * 0.5 + 0.5;
}
fn decode_normal(e: vec2<f32>) -> vec3<f32> {
    let f = e * 2.0 - 1.0;
    var n = vec3(f, 1.0 - abs(f.x) - abs(f.y));
    let t = max(-n.z, 0.0);
    n.x += select(t, -t, n.x >= 0.0);
    n.y += select(t, -t, n.y >= 0.0);
    return normalize(n);
}
