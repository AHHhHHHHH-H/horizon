// Daybreak's world hooks: Horizon calls these from its own terrain, entity
// and sky shaders (docs/shader-packs.md, `[gbuffer]`). Lighting is done here,
// per fragment (forward), in linear light, then tonemapped: the stages after
// the world only add fog, light shafts, reflections and bloom.
override sun_strength: f32 = 1.0;
override shadow_softness: f32 = 1.0;
override waving: bool = true;
override water_waves: f32 = 1.0;
override exposure: f32 = 1.0;

const TAU: f32 = 6.2831853;

// 0 at night, 1 by day, eased through sunrise and sunset.
fn daylight() -> f32 {
    return smoothstep(-0.06, 0.18, frame.sun.y);
}
// The colour of direct light: warm and low at the ends of the day, the
// moon's cool light at night.
fn light_color() -> vec3<f32> {
    let high = smoothstep(0.0, 0.45, frame.sun.y);
    let sun = mix(vec3(1.0, 0.48, 0.22), vec3(1.0, 0.9, 0.78), high) * 2.1 * sun_strength;
    let moon = vec3(0.32, 0.42, 0.7) * 0.4;
    let rain = 1.0 - 0.85 * frame.weather.x;
    return select(moon, sun, frame.shadow_light.w > 0.5) * rain;
}
// The sky's light on surfaces facing up: blue by day, a dim blue at night.
fn ambient_color() -> vec3<f32> {
    let day = daylight();
    let rain = mix(1.0, 0.55, frame.weather.x);
    return (mix(vec3(0.012, 0.016, 0.03), vec3(0.36, 0.5, 0.78), day) * rain) * 1.1;
}
// The painted sky along `d`: a gradient from the horizon to the zenith that
// follows the sun, a glow around it and its disc.
fn sky(d: vec3<f32>) -> vec3<f32> {
    let day = daylight();
    let sun = frame.sun.xyz;
    let up = clamp(d.y, -1.0, 1.0);
    let sunset = (1.0 - smoothstep(0.0, 0.35, abs(sun.y))) * smoothstep(-0.25, 0.1, sun.y);
    let zenith = mix(vec3(0.004, 0.006, 0.014), vec3(0.16, 0.32, 0.72), day);
    var horizon = mix(vec3(0.02, 0.025, 0.04), vec3(0.42, 0.58, 0.86), day);
    horizon = mix(horizon, vec3(1.0, 0.5, 0.24), sunset * 0.8);
    let h = pow(1.0 - max(up, 0.0), 3.0);
    var c = mix(zenith, horizon, h);
    // Below the horizon: darker, towards the ground.
    c = mix(c, horizon * 0.35, smoothstep(0.0, -0.3, up));
    let facing = max(dot(d, sun), 0.0);
    c += vec3(1.0, 0.62, 0.32) * pow(facing, 8.0) * (0.25 + 0.9 * sunset) * smoothstep(-0.2, 0.05, sun.y);
    c += vec3(1.0, 0.9, 0.75) * smoothstep(0.9993, 0.9997, facing) * 18.0 * day;
    let rain = frame.weather.x;
    c = mix(c, vec3(luminance(c)) * 0.7, rain * 0.8);
    return c;
}
fn tonemap(c: vec3<f32>) -> vec3<f32> {
    // A filmic curve (Narkowicz's fit of ACES), in linear light.
    let x = c * exposure * 0.8;
    return saturate((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14));
}

// Waves on water, as a height field of a few travelling sines: the
// gradient gives the surface normal.
fn wave_height(p: vec2<f32>, t: f32) -> f32 {
    var h = 0.0;
    h += sin(dot(p, vec2(0.8, 0.6)) * 1.1 + t * 1.3) * 0.5;
    h += sin(dot(p, vec2(-0.5, 0.86)) * 1.9 + t * 1.9) * 0.25;
    h += sin(dot(p, vec2(0.95, -0.3)) * 3.7 + t * 2.7) * 0.12;
    h += sin(dot(p, vec2(-0.2, -0.98)) * 6.1 + t * 3.3) * 0.06;
    return h * 0.05 * water_waves;
}
fn wave_normal(p: vec2<f32>, t: f32) -> vec3<f32> {
    let e = 0.05;
    let dx = wave_height(p + vec2(e, 0.0), t) - wave_height(p - vec2(e, 0.0), t);
    let dz = wave_height(p + vec2(0.0, e), t) - wave_height(p - vec2(0.0, e), t);
    return normalize(vec3(-dx / (2.0 * e), 1.0, -dz / (2.0 * e)));
}

fn surface_vertex(v: SurfaceVertex) -> vec3<f32> {
    var p = v.world;
    let world = v.world + frame.camera_position.xyz;
    let t = frame.time.x;
    let wind = (0.6 + 0.4 * frame.weather.x) * v.sky_light;
    if waving && (v.material == MATERIAL_LEAVES || (v.material == MATERIAL_PLANT && v.uv.y < 0.5)) {
        let phase = dot(world, vec3(0.7, 0.3, 0.9));
        let amount = select(0.07, 0.05, v.material == MATERIAL_LEAVES) * wind;
        p.x += sin(t * 1.7 + phase) * amount;
        p.z += sin(t * 1.3 + phase * 1.3) * amount * 0.7;
    }
    if v.material == MATERIAL_WATER && v.sky_light > 0.5 {
        p.y += wave_height(world.xz, t) * 0.6;
    }
    return p;
}

fn surface_normal(s: Surface) -> vec3<f32> {
    if s.material == MATERIAL_WATER && s.normal.y > 0.5 {
        return wave_normal(s.world.xz + frame.camera_position.xz, frame.time.x);
    }
    return s.normal;
}

// Direct light reaching the surface: facing the light, not in shadow.
fn direct(s: Surface, n: vec3<f32>) -> vec3<f32> {
    let l = frame.shadow_light.xyz;
    let foliage = s.material == MATERIAL_LEAVES || s.material == MATERIAL_PLANT;
    // Plants and leaves let light through: lit from either side.
    let facing = select(max(dot(n, l), 0.0), 0.35 + 0.65 * abs(dot(n, l)), foliage);
    if facing <= 0.0 { return vec3(0.0); }
    // Soft shadows: a few taps spread by the softness option.
    var lit = 0.0;
    let spread = 0.06 * shadow_softness;
    let side = normalize(cross(l, vec3(0.0, 0.0, 1.0)));
    let up = cross(side, l);
    let offsets = array<vec2<f32>, 4>(vec2(-1.0, -0.4), vec2(0.4, -1.0), vec2(1.0, 0.4), vec2(-0.4, 1.0));
    for (var i = 0; i < 4; i++) {
        let o = (side * offsets[i].x + up * offsets[i].y) * spread;
        lit += shadow_lit(s.world + o, select(n, l, foliage));
    }
    lit *= 0.25;
    // Low light fades out (the sun crossing the horizon), and only where the
    // sky reaches: caves keep their darkness.
    let elevation = smoothstep(0.02, 0.12, l.y);
    let outdoors = smoothstep(0.55, 0.95, s.sky_light);
    return light_color() * facing * lit * elevation * outdoors;
}

fn surface_color(s: Surface) -> vec4<f32> {
    // The Nether and the End keep vanilla's lighting.
    if frame.dimension.x != 0.0 { return s.vanilla; }
    let albedo = srgb_to_linear(s.albedo.rgb);
    let n = surface_normal(s);
    let view = normalize(s.world);
    let sky_light = s.sky_light * s.sky_light;
    let ambient = ambient_color() * sky_light * (0.75 + 0.25 * n.y);
    let block = vec3(1.0, 0.6, 0.3) * pow(s.block_light, 3.0) * 2.2;
    let sunlight = direct(s, n);
    var light = ambient + sunlight + block + vec3(0.012);
    if s.material == MATERIAL_EMISSIVE || s.material == MATERIAL_LAVA {
        light = max(light, vec3(3.0));
    }
    var color = albedo * light;
    var alpha = s.albedo.a;
    if s.material == MATERIAL_WATER {
        // Water: its tint lit like a surface, then the sky's reflection by
        // Fresnel and the sun's glint. Composite adds screen-space reflections.
        let r = reflect(view, n);
        let fresnel = 0.02 + 0.98 * pow(1.0 - max(dot(-view, n), 0.0), 5.0);
        let reflected = sky(r) * sky_light;
        let glint = pow(max(dot(r, frame.shadow_light.xyz), 0.0), 400.0) * 40.0;
        color = color * 0.6 + reflected * fresnel + sunlight * glint;
        alpha = mix(0.6, 0.97, fresnel);
    }
    return vec4(linear_to_srgb(tonemap(color)), alpha);
}

fn sky_color(p: SkyPixel) -> vec4<f32> {
    let c = sky(p.direction);
    // In the Nether and the End, and underwater, the vanilla sky.
    if frame.dimension.x != 0.0 || frame.camera_state.x != 0.0 {
        return p.vanilla;
    }
    return vec4(linear_to_srgb(tonemap(c)), 1.0);
}
