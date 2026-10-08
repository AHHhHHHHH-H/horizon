//! Mob rendering: the per-frame `EntityRenderState` snapshot extracted from a
//! simulated mob, and the vanilla `LivingEntityRenderer` + model `setupAnim`
//! ports that turn it into camera-relative vertices.
use super::model::{ModelLibrary, Part, render_model_tinted};
use crate::mesher::Vertex;
use horizon_world::registry::BlockStateId;
use std::sync::Arc;

/// Atlas lookup: texture path without namespace (`entity/pig/pig_temperate`)
/// to its `[u0, v0, u1, v1]` rectangle in the shared atlas.
pub type SpriteLookup<'a> = &'a dyn Fn(&str) -> Option<[f32; 4]>;

/// Frame snapshot of one mob, mirroring vanilla's `*RenderState` classes.
/// Values are interpolated for the frame's partial tick. Fields that do not
/// apply to a mob keep their defaults.
#[derive(Clone, Debug, Default)]
pub struct RenderState {
    /// Entity type id (`minecraft:pig`).
    pub kind: Arc<str>,
    pub entity_id: u32,
    /// Interpolated feet position (world space).
    pub position: [f64; 3],
    /// `yBodyRot` in degrees.
    pub body_rot: f32,
    /// Head yaw relative to the body (`yRot` of the render state), degrees.
    pub head_yaw: f32,
    /// Head pitch (`xRot`), degrees.
    pub head_pitch: f32,
    /// `tickCount + partialTick`.
    pub age_in_ticks: f32,
    pub walk_pos: f32,
    pub walk_speed: f32,
    /// `getScale()` (attribute scale; slimes use `size` instead).
    pub scale: f32,
    pub baby: bool,
    /// `getAgeScale()` (0.5 for babies of types without dedicated baby models).
    pub age_scale: f32,
    /// `deathTime + partialTick` when dying, else 0.
    pub death_time: f32,
    /// `hurtTime > 0 || deathTime > 0`.
    pub hurt_overlay: bool,
    pub on_ground: bool,
    pub in_water: bool,
    pub bb_width: f32,
    pub bb_height: f32,
    /// Resolved texture path for data-driven variants (adult or baby asset as
    /// appropriate), e.g. `entity/pig/pig_cold`. `None` = renderer default.
    pub texture: Option<Arc<str>>,
    /// Variant model selector (`cold`, `warm`) for pig/cow/chicken variants.
    pub model_variant: Option<Arc<str>>,
    /// DyeColor id (0 white .. 15 black): sheep wool, wolf/cat collar.
    pub color: Option<u8>,
    /// Small per-type variant index (rabbit type, horse colour, fox type,
    /// llama colour, tropical fish packed variant, axolotl, parrot...).
    pub variant_index: i32,
    /// Secondary variant (horse markings, tropical fish colours...).
    pub variant_extra: i32,
    /// `Mob.isAggressive()` (zombie arms, skeleton bow pose, piglin...).
    pub aggressive: bool,
    /// Swing progress `getAttackAnim(partial)` in 0..1.
    pub attack_time: f32,
    /// Main-hand item id (`minecraft:bow`), for held-item poses/rendering.
    pub main_hand: Option<Arc<str>>,
    pub sitting: bool,
    pub tame: bool,
    pub angry: bool,
    pub crouching: bool,
    pub sleeping: bool,
    pub in_love: bool,
    /// Creeper `getSwelling(partial)`.
    pub swelling: f32,
    pub powered: bool,
    /// Sheep.
    pub sheared: bool,
    pub head_eat_position: f32,
    pub head_eat_angle: f32,
    /// Chicken/parrot wing flap (`flap`, `flapSpeed`).
    pub flap: f32,
    pub flap_speed: f32,
    /// Wolf: tail angle, head roll (begging/interested), shake, wet shade.
    pub tail_angle: f32,
    pub head_roll: f32,
    pub shake_anim: f32,
    pub wet_shade: f32,
    /// Enderman.
    pub carried_block: Option<BlockStateId>,
    pub creepy: bool,
    /// Slime / magma cube squish and size.
    pub squish: f32,
    pub size: i32,
    /// Squid.
    pub tentacle_angle: f32,
    pub x_body_rot: f32,
    pub z_body_rot: f32,
    /// Ghast charging, blaze charged.
    pub charging: bool,
    /// Rabbit jump progress.
    pub jump_completion: f32,
    /// Horse eat/stand/feeding animations.
    pub eat_anim: f32,
    pub stand_anim: f32,
    pub feeding_anim: f32,
    /// Cat lie-down / relax amounts.
    pub lie_down: f32,
    pub relax: f32,
    /// Zombie/skeleton conversion shaking.
    pub shaking: bool,
    /// Bee roll amount and flags.
    pub roll_amount: f32,
    pub has_nectar: bool,
    pub has_stinger: bool,
}

/// Interpolated model rendering of one mob: `LivingEntityRenderer.render`.
pub fn render(state: &RenderState, models: &ModelLibrary, camera: glam::DVec3, sprites: SpriteLookup, light: u32, out: &mut Vec<Vertex>) {
    let kind = state.kind.trim_start_matches("minecraft:");
    let Some(model) = models.get(&format!("minecraft:{kind}#main")) else { return };
    let texture = state.texture.as_deref().map(str::to_owned).unwrap_or_else(|| default_texture(kind));
    let Some(uv) = sprites(&texture) else { return };
    let p = glam::DVec3::new(state.position[0], state.position[1], state.position[2]) - camera;
    let mut base = glam::Mat4::from_translation(p.as_vec3()) * glam::Mat4::from_rotation_y((180.0 - state.body_rot).to_radians());
    if state.death_time > 0.0 {
        let f = ((state.death_time - 1.0) / 20.0 * 1.6).sqrt().min(1.0);
        base *= glam::Mat4::from_rotation_z(f * 90.0_f32.to_radians());
    }
    if kind == "creeper" {
        let s = state.swelling.clamp(0.0, 1.0);
        let wobble = 1.0 + (s * 100.0).sin() * s * 0.01;
        let s4 = s * s * s * s;
        let xz = (1.0 + s4 * 0.4) * wobble;
        let y = (1.0 + s4 * 0.1) / wobble;
        base *= glam::Mat4::from_scale(glam::Vec3::new(xz, y, xz));
    }
    base *= glam::Mat4::from_scale(glam::Vec3::new(-1.0, -1.0, 1.0)) * glam::Mat4::from_translation(glam::Vec3::new(0.0, -1.501, 0.0));
    let overlay = u32::from(state.hurt_overlay);
    let limb = state.walk_pos;
    let speed = state.walk_speed;
    let swing = |phase: f32| (limb * 0.6662 + phase).cos() * 1.4 * speed;
    let pi = std::f32::consts::PI;
    let quadruped = matches!(kind, "pig" | "cow" | "sheep" | "creeper");
    let humanoid = matches!(kind, "zombie" | "skeleton");
    let pose_for = |part: &Part| -> Option<[f32; 9]> {
        let mut pose = part.pose;
        match part.name.as_str() {
            "head" => {
                pose[4] = state.head_yaw.to_radians();
                pose[3] = state.head_pitch.to_radians();
            }
            "right_hind_leg" | "left_front_leg" if quadruped => pose[3] = swing(0.0),
            "left_hind_leg" | "right_front_leg" if quadruped => pose[3] = swing(pi),
            "right_leg" if humanoid || kind == "chicken" => pose[3] = swing(0.0),
            "left_leg" if humanoid || kind == "chicken" => pose[3] = swing(pi),
            "right_arm" | "left_arm" if kind == "zombie" => pose[3] = -std::f32::consts::FRAC_PI_2 + (state.age_in_ticks * 0.067).sin() * 0.05,
            "right_arm" if humanoid => pose[3] = (limb * 0.6662 + pi).cos() * 2.0 * speed * 0.5,
            "left_arm" if humanoid => pose[3] = (limb * 0.6662).cos() * 2.0 * speed * 0.5,
            "right_wing" if kind == "chicken" => pose[5] = state.flap,
            "left_wing" if kind == "chicken" => pose[5] = -state.flap,
            _ => {}
        }
        Some(pose)
    };
    render_model_tinted(model, base, &pose_for, uv, light, overlay, 0xFFFF_FFFF, out);
}

/// Vanilla renderer default texture for a type without variant data.
pub fn default_texture(kind: &str) -> String {
    match kind {
        "pig" => "entity/pig/pig_temperate".into(),
        "cow" => "entity/cow/cow_temperate".into(),
        "chicken" => "entity/chicken/chicken_temperate".into(),
        "cave_spider" => "entity/spider/cave_spider".into(),
        _ => format!("entity/{kind}/{kind}"),
    }
}
