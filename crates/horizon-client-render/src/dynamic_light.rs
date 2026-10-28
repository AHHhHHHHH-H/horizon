//! Horizon Extras' dynamic lights (docs/extras.md): a torch, lantern or other light source held by
//! a player or mob, dropped as an item, or an entity on fire lights the blocks and entities
//! around it. Not in vanilla; off by default.
//!
//! Client-only and at render time: the light engine, chunk meshes and saves never see it. Each
//! frame the game picks the [`MAX`] sources nearest the camera; terrain.wgsl and entity.wgsl
//! raise a vertex's block light to `level - distance` (one level per block, as block light falls
//! off) when that is brighter. Off, the list is empty and the shaders skip the loop.
use crate::blocks::BlockFacts;
use glam::DVec3;
use horizon_world::registry::BlockRegistry;
use std::collections::HashMap;

/// Lights the shaders read: `Globals.dynamic_lights` in gfx.rs, terrain.wgsl and entity.wgsl.
pub const MAX: usize = 16;
/// Position/radius and RGB/intensity, as the shader reads them.
pub type Colored = [[f32; 4]; 2];

/// Select nearest sandbox lights without retaining an unbounded candidate list.
/// Inputs are world position, RGB, radius, intensity. Validation is repeated at
/// this renderer boundary; the sandbox additionally checks player distance.
pub fn select_colored(
    sources: impl Iterator<Item = ([f64; 3], u32, f32, f32)>,
    camera: DVec3,
) -> Vec<Colored> {
    let mut near: Vec<(f64, Colored)> = Vec::with_capacity(MAX + 1);
    for (at, rgb, radius, intensity) in sources {
        let at = DVec3::from(at);
        let d2 = at.distance_squared(camera);
        if !d2.is_finite()
            || d2 > 96.0 * 96.0
            || rgb > 0xFFFFFF
            || !radius.is_finite()
            || !(0.25..=16.0).contains(&radius)
            || !intensity.is_finite()
            || !(0.0..=1.0).contains(&intensity)
        {
            continue;
        }
        let p = (at - camera).as_vec3();
        let value = [
            [p.x, p.y, p.z, radius],
            [
                ((rgb >> 16) & 255) as f32 / 255.0,
                ((rgb >> 8) & 255) as f32 / 255.0,
                (rgb & 255) as f32 / 255.0,
                intensity,
            ],
        ];
        let i = near.partition_point(|(d, _)| *d <= d2);
        if i < MAX {
            near.insert(i, (d2, value));
            near.truncate(MAX);
        }
    }
    near.into_iter().map(|(_, l)| l).collect()
}

/// The shader's additive illumination, clamped per channel, with linear falloff.
pub fn colored_at(pos: [f32; 3], lights: &[Colored]) -> [f32; 3] {
    let mut rgb = [0.0f32; 3];
    for [at, color] in lights.iter().take(MAX) {
        let distance =
            ((pos[0] - at[0]).powi(2) + (pos[1] - at[1]).powi(2) + (pos[2] - at[2]).powi(2)).sqrt();
        let weight = (1.0 - distance / at[3]).max(0.0) * color[3];
        for i in 0..3 {
            rgb[i] = (rgb[i] + color[i] * weight).min(1.0);
        }
    }
    rgb
}
/// What an entity on fire gives off, as a lit campfire or fire block does.
pub const BURNING: u8 = 15;
/// Sources farther than this from the camera light nothing it can see well.
const REACH: f64 = 96.0;

/// How brightly each item glows when held or dropped.
#[derive(Debug, Default)]
pub struct Luminance(HashMap<String, u8>);

impl Luminance {
    /// A block item glows like its block's default state (`getLightEmission`): torches 14,
    /// lanterns and glowstone 15, a lit redstone torch 7; a furnace's item, unlit, not at all.
    pub fn new(facts: &BlockFacts, registry: &BlockRegistry) -> Self {
        let mut map = HashMap::new();
        for (id, state) in facts.states.iter().enumerate() {
            if state.emission == 0 || *state.item != *state.name {
                continue;
            }
            if registry.default_state(&state.name).map(|d| d.0 as usize) == Some(id) {
                map.insert(state.item.to_string(), state.emission);
            }
        }
        // A bucket of lava glows as lava does.
        map.insert("minecraft:lava_bucket".into(), 15);
        Self(map)
    }

    pub fn of(&self, item: &str) -> u8 {
        self.0.get(item).copied().unwrap_or(0)
    }

    #[cfg(test)]
    fn from_pairs(pairs: &[(&str, u8)]) -> Self {
        Self(pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect())
    }
}

/// One light this frame: where it is (world space) and its level, 1 to 15.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    pub at: DVec3,
    pub level: u8,
}

/// The [`MAX`] sources nearest `camera` within reach, as the shaders read them: camera-relative
/// position and level. Overwrites `out`.
pub fn select(sources: &[Source], camera: DVec3, out: &mut Vec<[f32; 4]>) {
    out.clear();
    let key = |s: &Source| s.at.distance_squared(camera);
    let mut near: Vec<&Source> = sources
        .iter()
        .filter(|s| s.level > 0 && key(s) <= REACH * REACH)
        .collect();
    near.sort_by(|a, b| key(a).total_cmp(&key(b)));
    out.extend(near.into_iter().take(MAX).map(|s| {
        let p = (s.at - camera).as_vec3();
        [p.x, p.y, p.z, f32::from(s.level.min(15))]
    }));
}

/// The block light a vertex at `pos` (camera-relative) gets from `lights`, 0 to 15: the WGSL
/// `dynamic_block_light` in Rust, for tests.
pub fn level_at(pos: [f32; 3], lights: &[[f32; 4]]) -> f32 {
    lights.iter().fold(0.0f32, |level, l| {
        let d =
            ((pos[0] - l[0]).powi(2) + (pos[1] - l[1]).powi(2) + (pos[2] - l[2]).powi(2)).sqrt();
        level.max(l[3] - d)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colored_sources_are_bounded_and_fall_off_without_changing_white_light() {
        let lights = select_colored(
            (0..100)
                .rev()
                .map(|i| ([f64::from(i), 0.0, 0.0], 0xFF0000, 4.0, 1.0)),
            DVec3::ZERO,
        );
        assert_eq!(lights.len(), MAX);
        assert_eq!(lights[0][0], [0.0, 0.0, 0.0, 4.0]);
        assert_eq!(colored_at([0.0; 3], &lights[..1]), [1.0, 0.0, 0.0]);
        assert_eq!(colored_at([2.0, 0.0, 0.0], &lights[..1]), [0.5, 0.0, 0.0]);
        assert_eq!(colored_at([4.0, 0.0, 0.0], &lights[..1]), [0.0; 3]);
        assert_eq!(colored_at([0.0; 3], &[]), [0.0; 3]);
        assert!(
            select_colored(std::iter::once(([f64::NAN; 3], 0, 4.0, 1.0)), DVec3::ZERO).is_empty()
        );
    }

    #[test]
    fn nearest_sources_win() {
        let camera = DVec3::new(100.0, 64.0, 100.0);
        let mut sources: Vec<Source> = (0..40)
            .map(|i| Source {
                at: camera + DVec3::new(f64::from(i), 0.0, 0.0),
                level: 14,
            })
            .collect();
        sources.push(Source {
            at: camera + DVec3::new(0.5, 0.0, 0.0),
            level: 0,
        });
        sources.push(Source {
            at: camera + DVec3::new(0.0, 0.0, 200.0),
            level: 15,
        });
        let mut out = Vec::new();
        select(&sources, camera, &mut out);
        assert_eq!(out.len(), MAX);
        assert_eq!(out[0], [0.0, 0.0, 0.0, 14.0]);
        assert_eq!(out[MAX - 1][0], (MAX - 1) as f32);
        select(&[], camera, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn light_falls_off_a_level_per_block() {
        let torch = [[0.0, 0.0, 0.0, 14.0]];
        assert_eq!(level_at([0.0, 0.0, 0.0], &torch), 14.0);
        assert_eq!(level_at([3.0, 4.0, 0.0], &torch), 9.0);
        assert_eq!(level_at([20.0, 0.0, 0.0], &torch), 0.0);
        let two = [[0.0, 0.0, 0.0, 14.0], [10.0, 0.0, 0.0, 15.0]];
        assert_eq!(level_at([8.0, 0.0, 0.0], &two), 13.0);
        assert_eq!(level_at([0.0; 3], &[]), 0.0);
    }

    #[test]
    fn item_luminance() {
        let l = Luminance::from_pairs(&[("minecraft:torch", 14)]);
        assert_eq!(l.of("minecraft:torch"), 14);
        assert_eq!(l.of("minecraft:stone"), 0);
    }
}
