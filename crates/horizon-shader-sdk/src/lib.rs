//! Horizon shader pack stages in Rust (docs/shader-packs.md, "Horizon
//! shaders v1"). The same contract as the WGSL prelude: the bindings of
//! group 0, the [`Frame`] uniform and the helpers, for a stage compiled by
//! rust-gpu: declare one with [`stage!`]. SPIR-V for Vulkan can't hold
//! textures or samplers in a struct, so the helpers take the ones they read.
//!
//! Options are SPIR-V specialization constants (rust-gpu has `u32` ones
//! only): give each one an `id` in `pack.toml`. Horizon passes every value as
//! the bits of an `f32` (`true` is 1.0, integers 0.0, 1.0, ...) and `stage!`
//! hands them over as `f32`.
#![no_std]
pub use spirv_std::glam::{self, *};
pub use spirv_std::image::Image2d as Texture;
pub use spirv_std::{num_traits, spirv, Image, Sampler};
#[cfg(target_arch = "spirv")]
use spirv_std::num_traits::Float;

/// A depth texture: the world's (1 at the near plane, 0 for the sky) or the shadow map.
pub type DepthTexture = Image!(2D, type = f32, sampled, depth);
/// The G-buffer.
pub type UintTexture = Image!(2D, type = u32, sampled);

/// The prelude's `Frame` (shader_pack_common.wgsl), field for field.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct Frame {
    pub view: Mat4,
    pub projection: Mat4,
    pub inverse_view: Mat4,
    pub inverse_projection: Mat4,
    pub camera_position: Vec4,
    pub resolution: Vec4,
    pub time: Vec4,
    pub sun: Vec4,
    pub moon: Vec4,
    pub sky_color: Vec4,
    pub fog_color: Vec4,
    pub fog: Vec4,
    pub weather: Vec4,
    pub camera_state: Vec4,
    pub view_params: Vec4,
    pub dimension: Vec4,
    pub previous_view_projection: Mat4,
    pub shadow_view_projection: Mat4,
    pub camera_delta: Vec4,
    pub shadow: Vec4,
    pub shadow_light: Vec4,
    pub world: Vec4,
}

pub const MATERIAL_NONE: u32 = 0;
pub const MATERIAL_TERRAIN: u32 = 1;
pub const MATERIAL_LEAVES: u32 = 2;
pub const MATERIAL_PLANT: u32 = 3;
pub const MATERIAL_WATER: u32 = 4;
pub const MATERIAL_LAVA: u32 = 5;
pub const MATERIAL_GLASS: u32 = 6;
pub const MATERIAL_EMISSIVE: u32 = 7;
pub const MATERIAL_ENTITY: u32 = 8;

/// What the world pass knew about a pixel's frontmost surface.
#[derive(Copy, Clone)]
pub struct GBuffer {
    /// World-space normal; zero for the sky.
    pub normal: Vec3,
    pub block_light: f32,
    pub sky_light: f32,
    pub material: u32,
}

/// Where a stage's pixel is.
#[derive(Copy, Clone)]
pub struct Pixel {
    /// 0..1, top left at 0.
    pub uv: Vec2,
    /// The fragment's position in pixels.
    pub position: Vec4,
}

impl Frame {
    /// Blocks from the eye along the view axis.
    #[inline(always)]
    pub fn linear_depth(&self, depth: f32) -> f32 {
        let near = self.view_params.x;
        let far = self.view_params.y;
        near * far / ((far - near) * depth + near)
    }
    #[inline(always)]
    pub fn view_position(&self, uv: Vec2, depth: f32) -> Vec3 {
        let v = self.inverse_projection * vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
        v.truncate() / v.w
    }
    /// World position relative to the eye.
    #[inline(always)]
    pub fn world_offset(&self, uv: Vec2, depth: f32) -> Vec3 {
        (self.inverse_view * self.view_position(uv, depth).extend(1.0)).truncate()
    }
    /// Screen position (uv, depth) of a camera-relative world point.
    #[inline(always)]
    pub fn project(&self, world: Vec3) -> Vec3 {
        let clip = self.projection * self.view * world.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        vec3(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, ndc.z)
    }
    /// Where a camera-relative world point was on screen last frame.
    #[inline(always)]
    pub fn previous_uv(&self, world: Vec3) -> Vec3 {
        let clip =
            self.previous_view_projection * (world + self.camera_delta.truncate()).extend(1.0);
        let ndc = clip.truncate() / clip.w;
        vec3(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, ndc.z)
    }
}

#[inline(always)]
pub fn sample(texture: &Texture, sampler: &Sampler, uv: Vec2) -> Vec4 {
    texture.sample_by_lod(*sampler, uv, 0.0)
}
/// A depth texture at `uv` (the world's: 1 at the near plane, 0 for the sky).
#[inline(always)]
pub fn depth(texture: &DepthTexture, sampler: &Sampler, uv: Vec2) -> f32 {
    let d: Vec4 = texture.sample_by_lod(*sampler, uv, 0.0);
    d.x
}
#[inline(always)]
pub fn is_sky(depth: f32) -> bool {
    depth <= 0.0
}
#[inline(always)]
pub fn gbuffer_at(gbuffer: &UintTexture, frame: &Frame, uv: Vec2) -> GBuffer {
    let size = vec2(frame.world.x, frame.world.y);
    let p = (uv * size).clamp(Vec2::ZERO, size - 1.0);
    let g: UVec4 = gbuffer.fetch(p.as_ivec2());
    let normal = if g.w != 0 {
        decode_normal(vec2(g.x as f32, g.y as f32) / 65535.0)
    } else {
        Vec3::ZERO
    };
    GBuffer {
        normal,
        block_light: (g.z >> 8) as f32 / 255.0,
        sky_light: (g.z & 255) as f32 / 255.0,
        material: g.w,
    }
}
/// 1 in light, 0 in shadow (2x2 filtered taps), lit outside the map.
#[inline(always)]
pub fn shadow_lit(
    map: &DepthTexture,
    sampler: &Sampler,
    f: &Frame,
    world: Vec3,
    normal: Vec3,
) -> f32 {
    if f.shadow.w == 0.0 {
        return 1.0;
    }
    let texel = 2.0 * f.shadow.y / f.shadow.x;
    let at = (f.shadow_view_projection * world.extend(1.0)).truncate().truncate();
    let scale = 1.0 + (at.length() + 0.1 - 1.0) * f.shadow.z;
    let p = world + normal * texel * scale * 1.5;
    let clip = f.shadow_view_projection * p.extend(1.0);
    let xy = shadow_distort(clip.truncate().truncate(), f.shadow.z);
    let c = vec3(xy.x * 0.5 + 0.5, 0.5 - xy.y * 0.5, clip.z);
    if c.x <= 0.0 || c.y <= 0.0 || c.x >= 1.0 || c.y >= 1.0 || c.z <= 0.0 || c.z >= 1.0 {
        return 1.0;
    }
    let o = 0.5 / f.shadow.x;
    let tap = |dx: f32, dy: f32| {
        map.sample_depth_reference_by_lod(*sampler, vec2(c.x + dx, c.y + dy), c.z, 0.0)
    };
    (tap(-o, -o) + tap(o, -o) + tap(-o, o) + tap(o, o)) * 0.25
}

#[inline(always)]
pub fn shadow_distort(clip: Vec2, k: f32) -> Vec2 {
    clip / (1.0 + (clip.length() + 0.1 - 1.0) * k)
}
#[inline(always)]
pub fn luminance(c: Vec3) -> f32 {
    c.dot(vec3(0.2126, 0.7152, 0.0722))
}
#[inline(always)]
fn srgb_channel_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}
#[inline(always)]
fn linear_channel_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.max(0.0).powf(1.0 / 2.4) - 0.055 }
}
#[inline(always)]
pub fn srgb_to_linear(c: Vec3) -> Vec3 {
    vec3(srgb_channel_to_linear(c.x), srgb_channel_to_linear(c.y), srgb_channel_to_linear(c.z))
}
#[inline(always)]
pub fn linear_to_srgb(c: Vec3) -> Vec3 {
    vec3(linear_channel_to_srgb(c.x), linear_channel_to_srgb(c.y), linear_channel_to_srgb(c.z))
}
#[inline(always)]
pub fn decode_normal(e: Vec2) -> Vec3 {
    let f = e * 2.0 - 1.0;
    let mut n = vec3(f.x, f.y, 1.0 - f.x.abs() - f.y.abs());
    let t = (-n.z).max(0.0);
    n.x += if n.x >= 0.0 { -t } else { t };
    n.y += if n.y >= 0.0 { -t } else { t };
    n.normalize()
}
/// A cheap hash of a pixel position, 0..1 (dithering, noise).
#[inline(always)]
pub fn hash(p: Vec2) -> f32 {
    ((p.dot(vec2(12.9898, 78.233))).sin() * 43758.547).fract()
}
#[inline(always)]
pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
#[inline(always)]
pub fn saturate(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// A stage's fragment entry point with Horizon's bindings. The caller names
/// them (the pixel, then group 0 in binding order), then the pack textures it
/// reads (group 1 bindings) and its options (specialization constant ids,
/// handed over as `f32`); the body gives the colour.
///
/// ```ignore
/// stage! {
///     fn grade(px, frame, scene_color, scene_depth, previous, linear, nearest,
///              gbuffer, opaque_depth, shadow_map, shadow_sampler, repeat;
///              textures [bloom = 1]; options [strength = 4]) {
///         sample(previous, linear, px.uv) + sample(bloom, linear, px.uv) * strength
///     }
/// }
/// ```
#[macro_export]
macro_rules! stage {
    (
        $(#[$meta:meta])*
        fn $entry:ident(
            $px:ident, $frame:ident, $scene_color:ident, $scene_depth:ident, $previous:ident,
            $linear:ident, $nearest:ident, $gbuffer:ident, $opaque_depth:ident,
            $shadow_map:ident, $shadow_sampler:ident, $repeat:ident;
            textures [$($t:ident = $tb:literal),*]; options [$($o:ident = $oid:literal),*]
        ) $body:block
    ) => {
        $(#[$meta])*
        #[allow(clippy::too_many_arguments, unused_variables)]
        #[$crate::spirv(fragment)]
        pub fn $entry(
            #[spirv(frag_coord)] position: $crate::Vec4,
            uv: $crate::Vec2,
            #[spirv(descriptor_set = 0, binding = 0)] $scene_color: &$crate::Texture,
            #[spirv(descriptor_set = 0, binding = 1)] $scene_depth: &$crate::DepthTexture,
            #[spirv(descriptor_set = 0, binding = 2)] $previous: &$crate::Texture,
            #[spirv(descriptor_set = 0, binding = 3)] $linear: &$crate::Sampler,
            #[spirv(descriptor_set = 0, binding = 4)] $nearest: &$crate::Sampler,
            #[spirv(uniform, descriptor_set = 0, binding = 5)] $frame: &$crate::Frame,
            #[spirv(descriptor_set = 0, binding = 6)] $gbuffer: &$crate::UintTexture,
            #[spirv(descriptor_set = 0, binding = 7)] $opaque_depth: &$crate::DepthTexture,
            #[spirv(descriptor_set = 0, binding = 8)] $shadow_map: &$crate::DepthTexture,
            #[spirv(descriptor_set = 0, binding = 9)] $shadow_sampler: &$crate::Sampler,
            #[spirv(descriptor_set = 0, binding = 10)] $repeat: &$crate::Sampler,
            $(#[spirv(descriptor_set = 1, binding = $tb)] $t: &$crate::Texture,)*
            $(#[spirv(spec_constant(id = $oid))] $o: u32,)*
            output: &mut $crate::Vec4,
        ) {
            let $px = $crate::Pixel { uv, position };
            $(let $o = f32::from_bits($o);)*
            *output = $body;
        }
    };
}
