//! Daybreak's full-screen stages, in Rust (rust-gpu). The world hooks that
//! light the terrain are in shaders/world.wgsl; these run after the world:
//! ambient occlusion (deferred), then reflections, fog and light shafts,
//! bloom and the grade (composite). Group 1 holds the pack's targets in
//! pack.toml order (`bloom_wide` 0, `bloom` 1); options go by their `id`.
#![no_std]
#[cfg(target_arch = "spirv")]
use horizon_shader_sdk::num_traits::Float;
use horizon_shader_sdk::*;

stage! {
    /// Eight points on a disc around the pixel, about a block across in the
    /// world: how many lie in front of the surface above it.
    fn ambient_occlusion(px, frame, scene_color, scene_depth, previous, linear, nearest,
        gbuffer, opaque_depth, shadow_map, shadow_sampler, repeat;
        textures []; options [enabled = 0]) {
        occlusion(px, frame, scene_color, scene_depth, nearest, gbuffer, enabled)
    }
}
stage! {
    fn atmosphere(px, frame, scene_color, scene_depth, previous, linear, nearest,
        gbuffer, opaque_depth, shadow_map, shadow_sampler, repeat;
        textures []; options [fog_density = 1, light_shafts = 2, exposure = 3]) {
        let water = reflect_water(px, frame, scene_color, scene_depth, previous, linear, nearest, gbuffer, opaque_depth);
        air(px, frame, scene_depth, nearest, water, fog_density, light_shafts, exposure)
    }
}
stage! {
    /// Bloom, at half size: what is brighter than most of the picture,
    /// blurred across...
    fn bloom_bright(px, frame, scene_color, scene_depth, previous, linear, nearest,
        gbuffer, opaque_depth, shadow_map, shadow_sampler, repeat;
        textures []; options []) {
        blur(previous, linear, px.uv, vec2(frame.resolution.z * 1.5, 0.0), true)
    }
}
stage! {
    /// ... then down.
    fn bloom_blur(px, frame, scene_color, scene_depth, previous, linear, nearest,
        gbuffer, opaque_depth, shadow_map, shadow_sampler, repeat;
        textures [bloom_wide = 0]; options []) {
        blur(bloom_wide, linear, px.uv, vec2(0.0, frame.resolution.w * 1.5), false)
    }
}
stage! {
    /// Bloom over the picture, saturation, a soft vignette.
    fn grade(px, frame, scene_color, scene_depth, previous, linear, nearest,
        gbuffer, opaque_depth, shadow_map, shadow_sampler, repeat;
        textures [bloom = 1]; options [bloom_strength = 4, saturation = 5]) {
        let glow = sample(bloom, linear, px.uv).truncate();
        let mut c = sample(previous, linear, px.uv).truncate() + glow * bloom_strength;
        c = Vec3::splat(luminance(c)).lerp(c, saturation);
        let d = ((px.uv - 0.5) * vec2(frame.resolution.x * frame.resolution.w, 1.0)).length();
        c *= 1.0 - 0.25 * smoothstep(0.5, 1.0, d);
        c.clamp(Vec3::ZERO, Vec3::ONE).extend(1.0)
    }
}

#[inline(always)]
fn occlusion(
    px: Pixel,
    frame: &Frame,
    scene_color: &Texture,
    scene_depth: &DepthTexture,
    nearest: &Sampler,
    gbuffer: &UintTexture,
    enabled: f32,
) -> Vec4 {
    let color = sample(scene_color, nearest, px.uv);
    let d0 = depth(scene_depth, nearest, px.uv);
    if enabled == 0.0 || is_sky(d0) {
        return color;
    }
    let g = gbuffer_at(gbuffer, frame, px.uv);
    if g.material == MATERIAL_NONE {
        return color;
    }
    let p = frame.world_offset(px.uv, d0);
    let distance = p.length();
    if distance > 48.0 {
        return color;
    }
    let radius = 0.6;
    let pixels = radius * frame.resolution.y / (distance * 2.0 * (frame.view_params.w * 0.5).tan());
    let step = Vec2::splat(pixels) * vec2(frame.resolution.z, frame.resolution.w);
    let noise = hash(vec2(px.position.x, px.position.y));
    let mut occluded = 0.0;
    let mut i = 0;
    while i < 8 {
        let a = (i as f32 + noise) * 0.785398;
        let r = ((i % 4) as f32 + 1.0) * 0.25;
        let uv = px.uv + vec2(a.cos(), a.sin()) * step * r;
        let d = depth(scene_depth, nearest, uv);
        if !is_sky(d) {
            let q = frame.world_offset(uv, d) - p;
            let len = q.length();
            let towards = q.dot(g.normal) / len.max(1e-4);
            occluded += saturate(towards - 0.15) * saturate(1.0 - len / (radius * 2.0));
        }
        i += 1;
    }
    let ao = 1.0 - 0.55 * saturate(occluded / 8.0 * 2.0) * (1.0 - smoothstep(32.0, 48.0, distance));
    (color.truncate() * ao).extend(color.w)
}

#[inline(always)]
fn daylight(f: &Frame) -> f32 {
    smoothstep(-0.06, 0.18, f.sun.y)
}
#[inline(always)]
fn tonemap(c: Vec3, exposure: f32) -> Vec3 {
    // A filmic curve (Narkowicz's fit of ACES), as world.wgsl's.
    let x = c * exposure * 0.8;
    ((x * (x * 2.51 + 0.03)) / (x * (x * 2.43 + 0.59) + 0.14)).clamp(Vec3::ZERO, Vec3::ONE)
}
/// The sky near the horizon in direction `d` (world.wgsl's sky, simplified),
/// tonemapped to the screen's values.
#[inline(always)]
fn horizon_color(f: &Frame, d: Vec3, exposure: f32) -> Vec3 {
    let day = daylight(f);
    let sun = f.sun.truncate();
    let sunset = (1.0 - smoothstep(0.0, 0.35, f.sun.y.abs())) * smoothstep(-0.25, 0.1, f.sun.y);
    let mut c = vec3(0.02, 0.025, 0.04).lerp(vec3(0.42, 0.58, 0.86), day);
    c = c.lerp(vec3(1.0, 0.5, 0.24), sunset * 0.8);
    c += vec3(1.0, 0.62, 0.32) * d.dot(sun).max(0.0).powf(8.0) * (0.25 + 0.9 * sunset);
    c = c.lerp(Vec3::splat(luminance(c) * 0.7), f.weather.x * 0.8);
    linear_to_srgb(tonemap(c, exposure))
}

/// The picture with water reflecting what's on screen: the reflected ray is
/// marched through the opaque depth (screen-space reflections).
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn reflect_water(
    px: Pixel,
    f: &Frame,
    scene_color: &Texture,
    scene_depth: &DepthTexture,
    previous: &Texture,
    linear: &Sampler,
    nearest: &Sampler,
    gbuffer: &UintTexture,
    opaque_depth: &DepthTexture,
) -> Vec3 {
    let c = sample(previous, linear, px.uv).truncate();
    let d0 = depth(scene_depth, nearest, px.uv);
    let g = gbuffer_at(gbuffer, f, px.uv);
    if f.dimension.x != 0.0 || g.material != MATERIAL_WATER || is_sky(d0) {
        return c;
    }
    let origin = f.world_offset(px.uv, d0);
    let view = origin.normalize();
    let r = view - g.normal * 2.0 * view.dot(g.normal);
    let fresnel = 0.02 + 0.98 * (1.0 - (-view).dot(g.normal).max(0.0)).powf(5.0);
    let mut t = 0.5;
    let mut i = 0;
    while i < 24 {
        let p = f.project(origin + r * t);
        if p.x < 0.0 || p.y < 0.0 || p.x > 1.0 || p.y > 1.0 || p.z <= 0.0 {
            break;
        }
        let uv = vec2(p.x, p.y);
        let d = depth(opaque_depth, nearest, uv);
        if !is_sky(d) && d > p.z && f.linear_depth(p.z) - f.linear_depth(d) < t * 0.3 + 0.5 {
            let edge = smoothstep(0.0, 0.08, p.x.min(1.0 - p.x).min(p.y.min(1.0 - p.y)));
            let hit = sample(scene_color, linear, uv).truncate();
            return c.lerp(hit, edge * fresnel * 0.8);
        }
        t *= 1.25;
        i += 1;
    }
    c
}

/// Height and distance fog in the sky's colour, fog under water, light
/// shafts towards the sun.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn air(
    px: Pixel,
    f: &Frame,
    scene_depth: &DepthTexture,
    nearest: &Sampler,
    color: Vec3,
    fog_density: f32,
    light_shafts: f32,
    exposure: f32,
) -> Vec4 {
    let mut c = color;
    let d0 = depth(scene_depth, nearest, px.uv);
    let sky = is_sky(d0);
    let world = f.world_offset(px.uv, d0.max(1e-7));
    let view = world.normalize();
    let overworld = f.dimension.x == 0.0;
    let underwater = f.camera_state.x == 1.0;
    if underwater {
        let d = if sky { 64.0 } else { f.linear_depth(d0) };
        let amount = 1.0 - (-d * 0.06 * fog_density).exp();
        c = c.lerp(vec3(0.03, 0.16, 0.24) * (0.3 + 0.7 * daylight(f)), amount);
    } else if overworld && !sky {
        let d = world.length();
        let density = (0.0015 + 0.012 * f.weather.x) * fog_density;
        let height = (world.y + f.camera_position.y - 63.0).max(0.0);
        let mut amount = 1.0 - (-d * density * (-height * 0.02).exp()).exp();
        amount = amount.max(smoothstep(f.view_params.z * 0.75, f.view_params.z, d));
        c = c.lerp(horizon_color(f, view, exposure), saturate(amount));
    }
    if light_shafts != 0.0 && overworld && !underwater && f.shadow_light.w > 0.5 {
        let sun_dir = f.sun.truncate();
        let forward = (f.inverse_view * vec4(0.0, 0.0, -1.0, 0.0)).truncate();
        let facing = sun_dir.dot(forward);
        if facing > 0.0 {
            let sun = f.project(sun_dir * 1000.0);
            let noise = hash(vec2(px.position.x, px.position.y));
            let mut open = 0.0;
            let mut i = 0;
            while i < 12 {
                let uv = px.uv.lerp(vec2(sun.x, sun.y), (i as f32 + noise) / 12.0);
                if is_sky(depth(scene_depth, nearest, uv.clamp(Vec2::ZERO, Vec2::ONE))) {
                    open += 1.0;
                }
                i += 1;
            }
            let glow = view.dot(sun_dir).max(0.0).powf(6.0) * facing;
            let low = 1.0 - smoothstep(0.1, 0.7, f.sun.y);
            c += vec3(1.0, 0.68, 0.38)
                * (open / 12.0 * glow * (0.15 + 0.25 * low) * (1.0 - f.weather.x));
        }
    }
    c.extend(1.0)
}

const WEIGHTS: [f32; 5] = [0.227, 0.194, 0.122, 0.054, 0.016];
/// Nine taps along `step`; `bright`: only what's brighter than most of the
/// picture.
#[inline(always)]
fn blur(source: &Texture, linear: &Sampler, uv: Vec2, step: Vec2, bright: bool) -> Vec4 {
    let mut c = Vec3::ZERO;
    let mut i: i32 = -4;
    while i <= 4 {
        let s = sample(source, linear, uv + step * i as f32).truncate();
        let keep = if bright { smoothstep(0.55, 0.95, luminance(s)) } else { 1.0 };
        c += s * keep * WEIGHTS[i.unsigned_abs() as usize];
        i += 1;
    }
    c.extend(1.0)
}
