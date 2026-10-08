//! Mobs: vanilla `Mob`/`PathfinderMob` simulation for the native client.
//!
//! * [`path`] / [`nav`]: `WalkNodeEvaluator` + `PathFinder` A* and
//!   `GroundPathNavigation` (path following, corner cutting, stuck checks).
//! * [`control`]: `MoveControl`, `LookControl`, `JumpControl`,
//!   `BodyRotationControl`.
//! * [`goals`]: `GoalSelector` with priorities/flags and the vanilla goals.
//! * [`body`]: `LivingEntity.aiStep`/`travel` physics (air, water, lava).
//! * [`species`]: per-type goal registration, attributes and behaviour.
//! * [`spawn`]: `NaturalSpawner` (caps, spawn rules, packs, despawning).
//! * [`interact`]: item-on-mob interactions (breeding, shearing, milking,
//!   taming, sitting), see [`Mob::interact`].
//! * [`model`] / [`render`]: baked vanilla layers and renderer ports.
//!
//! # Game integration
//!
//! Per tick: build a [`MobWorld`], call [`tick_all`], then apply the returned
//! [`MobEvent`]s and [`Death`]s (sounds, damage to the player, projectiles,
//! explosions, block changes, item drops, loot/experience). Natural spawning
//! is [`spawn::NaturalSpawner`]; chunk generation packs are
//! [`spawn::spawn_for_chunk_generation`]. Player attacks call [`Mob::hurt_by`];
//! right clicks on a mob call [`Mob::interact`].
use crate::{
    blocks::{BlockFacts, Fluid},
    mesher::Vertex,
    mobs::render::SpriteLookup,
    world::World,
};
use horizon_core::{
    geometry::{Aabb, Vec3},
    random::{LegacyRandom, RandomSource},
};
use horizon_world::{block_collision::BlockCollisionRegistry, registry::BlockStateId};
use serde_json::Value;
use std::{collections::HashMap, fs, path::Path, sync::Arc};

pub mod body;
pub mod control;
pub mod goals;
pub mod interact;
pub mod level;
pub mod model;
pub mod nav;
pub mod path;
pub mod render;
pub mod spawn;
pub mod species;

pub use interact::InteractionOutcome;
pub use level::MobRules;
pub use model::{ModelLibrary, Part, Polygon, load_model, render_model};
pub use render::RenderState;
pub use species::Species;

/// Small helpers over `RandomSource` with vanilla's bound semantics.
pub trait Rand {
    fn int(&mut self, bound: i32) -> i32;
    fn float(&mut self) -> f32;
    fn double(&mut self) -> f64;
    fn boolean(&mut self) -> bool;
    fn gaussian(&mut self) -> f64;
    /// `Mth.nextInt(random, min, max)` (inclusive).
    fn int_between(&mut self, min: i32, max: i32) -> i32 {
        if min >= max { min } else { self.int(max - min + 1) + min }
    }
}
impl Rand for LegacyRandom {
    fn int(&mut self, bound: i32) -> i32 {
        self.next_int_bound(bound.max(1)).unwrap_or(0)
    }
    fn float(&mut self) -> f32 {
        self.next_float()
    }
    fn double(&mut self) -> f64 {
        self.next_double()
    }
    fn boolean(&mut self) -> bool {
        self.next_boolean()
    }
    fn gaussian(&mut self) -> f64 {
        self.next_gaussian()
    }
}

/// `Mth.wrapDegrees`.
pub fn wrap_degrees(a: f32) -> f32 {
    let mut a = a % 360.0;
    if a >= 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}
fn wrap(a: f32) -> f32 {
    wrap_degrees(a)
}

/// `MobCategory`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Monster,
    Creature,
    Ambient,
    Axolotls,
    UndergroundWaterCreature,
    WaterCreature,
    WaterAmbient,
    Misc,
}
impl Category {
    pub const SPAWNING: [Category; 7] = [
        Category::Monster,
        Category::Creature,
        Category::Ambient,
        Category::Axolotls,
        Category::UndergroundWaterCreature,
        Category::WaterCreature,
        Category::WaterAmbient,
    ];
    pub fn from_name(s: &str) -> Self {
        match s {
            "monster" => Self::Monster,
            "creature" => Self::Creature,
            "ambient" => Self::Ambient,
            "axolotls" => Self::Axolotls,
            "underground_water_creature" => Self::UndergroundWaterCreature,
            "water_creature" => Self::WaterCreature,
            "water_ambient" => Self::WaterAmbient,
            _ => Self::Misc,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Monster => "monster",
            Self::Creature => "creature",
            Self::Ambient => "ambient",
            Self::Axolotls => "axolotls",
            Self::UndergroundWaterCreature => "underground_water_creature",
            Self::WaterCreature => "water_creature",
            Self::WaterAmbient => "water_ambient",
            Self::Misc => "misc",
        }
    }
    /// `getMaxInstancesPerChunk`.
    pub fn max(self) -> i32 {
        match self {
            Self::Monster => 70,
            Self::Creature => 10,
            Self::Ambient => 15,
            Self::Axolotls | Self::UndergroundWaterCreature | Self::WaterCreature => 5,
            Self::WaterAmbient => 20,
            Self::Misc => -1,
        }
    }
    pub fn is_friendly(self) -> bool {
        self != Self::Monster
    }
    pub fn is_persistent(self) -> bool {
        matches!(self, Self::Creature | Self::Misc)
    }
    pub fn despawn_distance(self) -> i32 {
        if self == Self::WaterAmbient { 64 } else { 128 }
    }
    pub fn no_despawn_distance(self) -> i32 {
        32
    }
}

/// Default attribute values (`entity-types.json`, from `DefaultAttributes`).
#[derive(Clone, Debug)]
pub struct Attributes {
    pub max_health: f64,
    pub movement_speed: f64,
    pub flying_speed: f64,
    pub attack_damage: f64,
    pub attack_knockback: f64,
    pub follow_range: f64,
    pub knockback_resistance: f64,
    pub armor: f64,
    pub armor_toughness: f64,
    pub step_height: f64,
    pub gravity: f64,
    pub jump_strength: f64,
    pub safe_fall_distance: f64,
    pub fall_damage_multiplier: f64,
    pub tempt_range: f64,
    pub water_movement_efficiency: f64,
    pub movement_efficiency: f64,
    pub scale: f64,
}
impl Attributes {
    fn from_json(v: &Value) -> Self {
        let a = |k: &str, d: f64| v[format!("minecraft:{k}")].as_f64().unwrap_or(d);
        Self {
            max_health: a("max_health", 20.0),
            movement_speed: a("movement_speed", 0.7),
            flying_speed: a("flying_speed", 0.4),
            attack_damage: a("attack_damage", 2.0),
            attack_knockback: a("attack_knockback", 0.0),
            follow_range: a("follow_range", 16.0),
            knockback_resistance: a("knockback_resistance", 0.0),
            armor: a("armor", 0.0),
            armor_toughness: a("armor_toughness", 0.0),
            step_height: a("step_height", 0.6),
            gravity: a("gravity", 0.08),
            jump_strength: a("jump_strength", 0.42),
            safe_fall_distance: a("safe_fall_distance", 3.0),
            fall_damage_multiplier: a("fall_damage_multiplier", 1.0),
            tempt_range: a("tempt_range", 10.0),
            water_movement_efficiency: a("water_movement_efficiency", 0.0),
            movement_efficiency: a("movement_efficiency", 0.0),
            scale: a("scale", 1.0),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MobType {
    pub id: String,
    pub species: Species,
    pub width: f64,
    pub height: f64,
    pub eye_height: f64,
    pub category: Category,
    pub fire_immune: bool,
    pub attributes: Attributes,
    /// `getDefaultDimensions` for babies: (width, height, eye height).
    pub baby_dimensions: Option<(f64, f64, f64)>,
    // Legacy fields kept for callers predating the species module.
    pub max_health: f32,
    pub speed: f64,
    pub attack_damage: f32,
    pub follow_range: f64,
    pub step_height: f64,
    pub models: Arc<ModelLibrary>,
    /// Renderer default texture (without namespace).
    pub texture: String,
    /// Ambient, hurt and death sound events (without namespace).
    pub sounds: [&'static str; 3],
}
pub struct MobTypes {
    pub types: HashMap<String, MobType>,
}
impl MobTypes {
    /// Load every supported type from `entity-types.json` and the baked models.
    pub fn load(root: &Path) -> crate::Result<Self> {
        let types: Value = serde_json::from_slice(&fs::read(root.join("entity-types.json"))?)?;
        let library = Arc::new(ModelLibrary::load(root)?);
        let mut out = HashMap::new();
        for species in Species::ALL {
            let id = format!("minecraft:{}", species.name());
            let t = &types[&id];
            if t.is_null() {
                continue;
            }
            let attributes = Attributes::from_json(&t["attributes"]);
            let sounds = species.default_sounds();
            out.insert(
                id.clone(),
                MobType {
                    species,
                    width: t["width"].as_f64().unwrap_or(0.6),
                    height: t["height"].as_f64().unwrap_or(1.8),
                    eye_height: t["eye_height"].as_f64().unwrap_or(1.6),
                    category: Category::from_name(t["category"].as_str().unwrap_or("misc")),
                    fire_immune: t["fire_immune"].as_bool().unwrap_or(false),
                    baby_dimensions: species.baby_dimensions(t["width"].as_f64().unwrap_or(0.6), t["height"].as_f64().unwrap_or(1.8)),
                    max_health: attributes.max_health as f32,
                    speed: attributes.movement_speed,
                    attack_damage: attributes.attack_damage as f32,
                    follow_range: attributes.follow_range,
                    step_height: attributes.step_height,
                    attributes,
                    models: library.clone(),
                    texture: render::default_texture(species.name()),
                    sounds,
                    id,
                },
            );
        }
        Ok(Self { types: out })
    }
}

/// Who an entity reference points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntityRef {
    Player,
    Mob(u32),
}
/// The local player as mobs see it.
#[derive(Clone, Debug, Default)]
pub struct PlayerView {
    pub position: [f64; 3],
    pub eye_height: f64,
    pub width: f64,
    pub height: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub alive: bool,
    /// Creative or spectator: never targeted.
    pub creative: bool,
    pub main_hand: Option<Arc<str>>,
    pub off_hand: Option<Arc<str>>,
    /// Wearing a carved pumpkin (endermen ignore the gaze).
    pub pumpkin_head: bool,
}
/// Snapshot of any living entity for targeting and searches.
#[derive(Clone, Debug)]
pub struct EntityView {
    pub r: EntityRef,
    pub species: Option<Species>,
    pub category: Category,
    pub position: [f64; 3],
    pub width: f64,
    pub height: f64,
    pub eye_height: f64,
    pub alive: bool,
    pub baby: bool,
    pub age: i32,
    pub in_love: bool,
    pub panicking: bool,
    pub tame: bool,
    pub owned_by_player: bool,
    pub sitting: bool,
    pub health: f32,
    pub target: Option<EntityRef>,
    pub creative: bool,
    pub in_water: bool,
    pub on_ground: bool,
    pub variant: u32,
    pub hostile: bool,
}
impl EntityView {
    pub fn eye(&self) -> [f64; 3] {
        [self.position[0], self.position[1] + self.eye_height, self.position[2]]
    }
    pub fn bounds(&self) -> Aabb {
        let w = self.width / 2.0;
        let p = self.position;
        Aabb::new(Vec3::new(p[0] - w, p[1], p[2] - w), Vec3::new(p[0] + w, p[1] + self.height, p[2] + w))
    }
    pub fn dist2(&self, p: [f64; 3]) -> f64 {
        (self.position[0] - p[0]).powi(2) + (self.position[1] - p[1]).powi(2) + (self.position[2] - p[2]).powi(2)
    }
    pub fn is(&self, s: Species) -> bool {
        self.species == Some(s)
    }
    pub fn is_player(&self) -> bool {
        self.r == EntityRef::Player
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Difficulty {
    Peaceful,
    Easy,
    #[default]
    Normal,
    Hard,
}
impl Difficulty {
    pub fn id(self) -> i32 {
        self as i32
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Dimension {
    #[default]
    Overworld,
    Nether,
    End,
}

/// Everything a mob tick reads from the game.
pub struct MobWorld<'a> {
    pub world: &'a World,
    pub facts: &'a BlockFacts,
    pub collision: &'a BlockCollisionRegistry<'a>,
    pub rules: &'a MobRules,
    pub biomes: &'a crate::biome::Biomes,
    pub player: Option<PlayerView>,
    /// `Level.isBrightOutside()` (overworld day, not raining hard).
    pub daylight: bool,
    /// `Level.getSkyDarken()`.
    pub sky_darken: i32,
    pub game_time: i64,
    pub day_time: i64,
    pub raining: bool,
    pub thundering: bool,
    pub difficulty: Difficulty,
    pub dimension: Dimension,
    pub mob_griefing: bool,
    /// Last entity that hurt the player and when (`getLastHurtByMob`), for
    /// tamed pets (`OwnerHurtByTargetGoal`).
    pub owner_hurt_by: Option<EntityRef>,
    pub owner_hurt_by_time: i32,
    /// Last entity the player attacked and when (`getLastHurtMob`).
    pub owner_last_hurt: Option<EntityRef>,
    pub owner_last_hurt_time: i32,
}
impl MobWorld<'_> {
    pub fn level(&self) -> level::GameLevel<'_> {
        level::GameLevel { world: self.world, facts: self.facts, collision: self.collision, rules: self.rules }
    }
}

/// Damage types the mob logic distinguishes (`DamageTypes` + tags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageKind {
    PlayerAttack,
    MobAttack,
    Arrow,
    Fireball,
    Explosion,
    Fall,
    OnFire,
    InFire,
    Lava,
    Drown,
    Cactus,
    Magic,
    Generic,
    OutOfWorld,
    Thorns,
}
impl DamageKind {
    /// `#panic_causes` (`#panic_environmental_causes` + entity damage).
    pub fn panic_causes(self) -> bool {
        !matches!(self, DamageKind::Fall | DamageKind::Drown | DamageKind::OutOfWorld | DamageKind::Magic | DamageKind::Generic)
    }
    pub fn is_fire(self) -> bool {
        matches!(self, DamageKind::OnFire | DamageKind::InFire | DamageKind::Lava | DamageKind::Fireball)
    }
    pub fn is_projectile(self) -> bool {
        matches!(self, DamageKind::Arrow | DamageKind::Fireball)
    }
    pub fn bypasses_cooldown(self) -> bool {
        false
    }
    pub fn no_knockback(self) -> bool {
        matches!(self, DamageKind::Fall | DamageKind::OnFire | DamageKind::InFire | DamageKind::Lava | DamageKind::Drown | DamageKind::Cactus | DamageKind::OutOfWorld | DamageKind::Magic | DamageKind::Generic)
    }
}
/// A `DamageSource`: kind, responsible entity and knockback origin.
#[derive(Clone, Copy, Debug)]
pub struct Damage {
    pub amount: f32,
    pub kind: DamageKind,
    pub attacker: Option<EntityRef>,
    /// Source position for knockback (`getSourcePosition`) or projectile origin.
    pub from: Option<[f64; 3]>,
}
impl Damage {
    pub fn player(amount: f32, from: [f64; 3]) -> Self {
        Self { amount, kind: DamageKind::PlayerAttack, attacker: Some(EntityRef::Player), from: Some(from) }
    }
    pub fn arrow(amount: f32, from: [f64; 3], shooter: Option<EntityRef>) -> Self {
        Self { amount, kind: DamageKind::Arrow, attacker: shooter, from: Some(from) }
    }
    pub fn explosion(amount: f32, center: [f64; 3]) -> Self {
        Self { amount, kind: DamageKind::Explosion, attacker: None, from: Some(center) }
    }
    pub fn plain(amount: f32, kind: DamageKind) -> Self {
        Self { amount, kind, attacker: None, from: None }
    }
}

/// Events a mob tick asks the game to act on.
#[derive(Clone, Debug)]
pub enum MobEvent {
    /// Sound event id without namespace, position, volume, pitch.
    Sound(String, [f64; 3], f32, f32),
    /// Melee damage to the player: amount, attacker position, attacker kind.
    AttackPlayer(f32, [f64; 3]),
    /// Arrow from `from` aimed at `target` (skeleton family).
    Shoot([f64; 3], [f64; 3]),
    /// Arrow with explicit initial velocity (and shooter for credit).
    Arrow { from: [f64; 3], velocity: [f64; 3], damage: f64, shooter: u32 },
    /// Small/large fireball: position, direction, explosion power (0 = small).
    Fireball { from: [f64; 3], direction: [f64; 3], power: i32, shooter: u32 },
    /// Witch potion: thrown at target position.
    Potion { from: [f64; 3], velocity: [f64; 3], effect: &'static str },
    Explode([f64; 3], f32),
    /// Set a block (mob griefing: sheep eating grass, endermen).
    SetBlock([i32; 3], BlockStateId),
    /// Destroy a block with its break effects (no drops).
    DestroyBlock([i32; 3]),
    /// Block break particles and sound only (`levelEvent 2001`).
    BreakEffect([i32; 3], BlockStateId),
    /// Drop an item stack at a position with an initial velocity.
    DropItem { item: Arc<str>, count: u32, at: [f64; 3], velocity: [f64; 3] },
    /// Spawn experience orbs.
    Experience([f64; 3], u32),
    /// Apply a status effect to the player (`effect id`, duration ticks, amplifier).
    PlayerEffect(&'static str, i32, i32),
    /// Set the player on fire for ticks.
    IgnitePlayer(i32),
    /// Particles (`minecraft:` id), position, count.
    Particles(&'static str, [f64; 3], u32),
}

/// A mob that finished dying this tick.
#[derive(Clone, Debug)]
pub struct Death {
    pub id: u32,
    pub kind: String,
    pub position: [f64; 3],
    pub killed_by_player: bool,
    /// Loot tables to roll (`minecraft:entities/...` without the prefix),
    /// empty for babies.
    pub loot_tables: Vec<String>,
    /// Experience to drop (`getExperienceReward`).
    pub experience: u32,
    /// Extra fixed drops (equipment, carried enderman block...).
    pub extra_drops: Vec<(Arc<str>, u32)>,
}

pub mod flags {
    /// Moves with `FlyingMoveControl`-style 3D steering.
    pub const FLYING_MOVE: u32 = 1;
    /// Look control keeps pitch between ticks.
    pub const KEEP_PITCH: u32 = 2;
    /// Not affected by fluids (move control jumping ends in fluids otherwise).
    pub const IGNORES_FLUIDS: u32 = 4;
    /// No gravity.
    pub const NO_GRAVITY: u32 = 8;
    /// Water animal: breathes water, swims.
    pub const WATER_ANIMAL: u32 = 16;
    /// Climbs walls (spiders).
    pub const CLIMBER: u32 = 32;
    /// Burns in daylight (`#burn_in_daylight`).
    pub const BURNS_IN_DAYLIGHT: u32 = 64;
    /// Sensitive to water (endermen, blazes, snow golems, striders).
    pub const WATER_SENSITIVE: u32 = 128;
    /// Can be bred (`Animal`).
    pub const ANIMAL: u32 = 256;
    /// Is a `Monster` (`Enemy`).
    pub const MONSTER: u32 = 512;
    /// Undead (smite, healing inverted).
    pub const UNDEAD: u32 = 1024;
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MobFlags(pub u32);
impl MobFlags {
    pub const FLYING_MOVE: u32 = flags::FLYING_MOVE;
    pub const KEEP_PITCH: u32 = flags::KEEP_PITCH;
    pub const IGNORES_FLUIDS: u32 = flags::IGNORES_FLUIDS;
    pub fn contains(self, f: u32) -> bool {
        self.0 & f != 0
    }
    pub fn set(&mut self, f: u32, on: bool) {
        if on {
            self.0 |= f;
        } else {
            self.0 &= !f;
        }
    }
}

/// Live attribute values (base values plus the modifiers vanilla applies).
#[derive(Clone, Debug)]
pub struct LiveAttributes {
    pub max_health: f64,
    pub movement_speed: f64,
    pub flying_speed: f64,
    pub attack_damage: f64,
    pub attack_knockback: f64,
    pub follow_range: f64,
    pub knockback_resistance: f64,
    pub armor: f64,
    pub step_height: f64,
    pub gravity: f64,
    pub jump_strength: f64,
    pub safe_fall_distance: f64,
    pub tempt_range: f64,
    pub water_movement_efficiency: f64,
}
impl From<&Attributes> for LiveAttributes {
    fn from(a: &Attributes) -> Self {
        Self {
            max_health: a.max_health,
            movement_speed: a.movement_speed,
            flying_speed: a.flying_speed,
            attack_damage: a.attack_damage,
            attack_knockback: a.attack_knockback,
            follow_range: a.follow_range,
            knockback_resistance: a.knockback_resistance,
            armor: a.armor,
            step_height: a.step_height,
            gravity: a.gravity,
            jump_strength: a.jump_strength,
            safe_fall_distance: a.safe_fall_distance,
            tempt_range: a.tempt_range,
            water_movement_efficiency: a.water_movement_efficiency,
        }
    }
}

/// One simulated mob.
pub struct Mob {
    pub id: u32,
    pub kind: Arc<MobType>,
    /// Per-entity random (`Entity.random`).
    pub random: LegacyRandom,
    pub attributes: LiveAttributes,
    pub flags: MobFlags,
    // Entity
    pub position: Vec3,
    pub previous: Vec3,
    pub velocity: Vec3,
    /// `yRot` (degrees).
    pub yaw: f32,
    pub previous_yaw: f32,
    /// `xRot` (degrees).
    pub pitch: f32,
    pub previous_pitch: f32,
    /// `yHeadRot`.
    pub head_yaw: f32,
    pub previous_head_yaw: f32,
    /// `yBodyRot`.
    pub body_yaw: f32,
    pub previous_body_yaw: f32,
    /// Legacy alias of `pitch` for older callers.
    pub head_pitch: f32,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub vertical_collision: bool,
    pub fall_distance: f64,
    pub in_water: bool,
    pub in_lava: bool,
    pub water_height: f64,
    pub lava_height: f64,
    pub eye_in_water: bool,
    pub fire_ticks: i32,
    pub air_supply: i32,
    /// `tickCount`.
    pub age: u32,
    pub removed: bool,
    // LivingEntity
    pub health: f32,
    pub hurt_time: i32,
    pub invulnerable: i32,
    pub last_hurt: f32,
    pub death_time: i32,
    pub dead: bool,
    pub last_hurt_by: Option<EntityRef>,
    pub last_hurt_by_timestamp: i32,
    pub last_hurt_by_memory: i32,
    pub last_hurt_by_player_time: i32,
    pub last_damage: Option<(DamageKind, i64)>,
    pub last_hurt_mob: Option<EntityRef>,
    pub speed: f32,
    pub xxa: f32,
    pub yya: f32,
    pub zza: f32,
    pub jumping: bool,
    pub no_jump_delay: i32,
    pub swinging: bool,
    pub swing_time: i32,
    pub attack_anim: f32,
    pub previous_attack_anim: f32,
    pub limb_position: f32,
    pub limb_speed: f32,
    pub previous_limb_speed: f32,
    pub use_item_ticks: i32,
    pub using_item: bool,
    pub main_hand: Option<Arc<str>>,
    pub head_item: Option<Arc<str>>,
    // Mob
    pub goals: goals::GoalSelector,
    pub targets: goals::GoalSelector,
    pub nav: nav::Navigation,
    pub move_control: control::MoveControl,
    pub look_control: control::LookControl,
    pub jump_control: bool,
    pub body_control: control::BodyRotationControl,
    pub target: Option<EntityRef>,
    pub aggressive: bool,
    pub persistent: bool,
    pub no_action_time: i32,
    pub ambient_sound_time: i32,
    pub malus: path::Malus,
    pub home: Option<([i32; 3], i32)>,
    pub can_pick_up_loot: bool,
    // AgeableMob / Animal
    pub age_ticks: i32,
    pub forced_age: i32,
    pub forced_age_timer: i32,
    pub in_love: i32,
    pub panicking: bool,
    /// Species-specific state.
    pub data: species::SpeciesData,
    /// Legacy fields (kept for callers that still read them).
    pub idle: u32,
    pub swell: i32,
    pub previous_swell: i32,
}
impl std::fmt::Debug for Mob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mob").field("id", &self.id).field("kind", &self.kind.id).field("position", &self.position).field("health", &self.health).finish()
    }
}

/// Per-tick context handed to goals and behaviours.
pub struct Cx<'a, 'w> {
    pub w: &'a MobWorld<'w>,
    pub level: level::GameLevel<'a>,
    pub entities: &'a [EntityView],
    pub events: &'a mut Vec<MobEvent>,
    /// Requests that touch other mobs, applied after this mob's tick.
    pub requests: &'a mut Vec<Request>,
    pub self_index: usize,
}
/// Cross-mob effects requested during a tick.
#[derive(Debug)]
pub enum Request {
    Hurt { target: EntityRef, damage: Damage },
    SetTarget { mob: u32, target: Option<EntityRef> },
    /// Breeding finished: reset the partner's love and age.
    Bred { partner: u32 },
    Spawn(Box<Mob>),
    /// Knock an entity back (attack knockback attribute).
    Knockback { target: EntityRef, strength: f64, dx: f64, dz: f64 },
}
impl<'a, 'w> Cx<'a, 'w> {
    pub fn view(&self, r: EntityRef) -> Option<&EntityView> {
        match r {
            EntityRef::Player => self.entities.first().filter(|e| e.r == EntityRef::Player),
            EntityRef::Mob(id) => self.entities.iter().find(|e| e.r == r && e.r == EntityRef::Mob(id)),
        }
    }
    pub fn player(&self) -> Option<&EntityView> {
        self.view(EntityRef::Player)
    }
    pub fn sound(&mut self, name: impl Into<String>, at: [f64; 3], volume: f32, pitch: f32) {
        self.events.push(MobEvent::Sound(name.into(), at, volume, pitch));
    }
}

impl Mob {
    pub fn new(id: u32, kind: Arc<MobType>, position: Vec3, yaw: f32) -> Self {
        let seed = (i64::from(id) << 20) ^ (position.x.to_bits() as i64).rotate_left(13) ^ (position.z.to_bits() as i64) ^ 0x5DEE_CE66;
        Self::with_random(id, kind, position, yaw, LegacyRandom::new(seed))
    }
    pub fn with_random(id: u32, kind: Arc<MobType>, position: Vec3, yaw: f32, random: LegacyRandom) -> Self {
        let attributes = LiveAttributes::from(&kind.attributes);
        let mut mob = Self {
            id,
            random,
            health: attributes.max_health as f32,
            nav: nav::Navigation::new(kind.attributes.follow_range),
            attributes,
            flags: MobFlags::default(),
            position,
            previous: position,
            velocity: Vec3::ZERO,
            yaw,
            previous_yaw: yaw,
            pitch: 0.0,
            previous_pitch: 0.0,
            head_yaw: yaw,
            previous_head_yaw: yaw,
            body_yaw: yaw,
            previous_body_yaw: yaw,
            head_pitch: 0.0,
            on_ground: false,
            horizontal_collision: false,
            vertical_collision: false,
            fall_distance: 0.0,
            in_water: false,
            in_lava: false,
            water_height: 0.0,
            lava_height: 0.0,
            eye_in_water: false,
            fire_ticks: 0,
            air_supply: 300,
            age: 0,
            removed: false,
            hurt_time: 0,
            invulnerable: 0,
            last_hurt: 0.0,
            death_time: 0,
            dead: false,
            last_hurt_by: None,
            last_hurt_by_timestamp: 0,
            last_hurt_by_memory: 0,
            last_hurt_by_player_time: 0,
            last_damage: None,
            last_hurt_mob: None,
            speed: 0.0,
            xxa: 0.0,
            yya: 0.0,
            zza: 0.0,
            jumping: false,
            no_jump_delay: 0,
            swinging: false,
            swing_time: 0,
            attack_anim: 0.0,
            previous_attack_anim: 0.0,
            limb_position: 0.0,
            limb_speed: 0.0,
            previous_limb_speed: 0.0,
            use_item_ticks: 0,
            using_item: false,
            main_hand: None,
            head_item: None,
            goals: goals::GoalSelector::default(),
            targets: goals::GoalSelector::default(),
            move_control: control::MoveControl::default(),
            look_control: control::LookControl::default(),
            jump_control: false,
            body_control: control::BodyRotationControl::default(),
            target: None,
            aggressive: false,
            persistent: false,
            no_action_time: 0,
            ambient_sound_time: 0,
            malus: path::Malus::default(),
            home: None,
            can_pick_up_loot: false,
            age_ticks: 0,
            forced_age: 0,
            forced_age_timer: 0,
            in_love: 0,
            panicking: false,
            data: species::SpeciesData::default(),
            idle: 0,
            swell: 0,
            previous_swell: 0,
            kind,
        };
        mob.ambient_sound_time = -mob.ambient_sound_interval();
        species::register(&mut mob);
        mob
    }
    pub fn species(&self) -> Species {
        self.kind.species
    }
    pub fn is_baby(&self) -> bool {
        if self.species().is_ageable() { self.age_ticks < 0 } else { self.data.baby }
    }
    /// (width, height) after baby and size scaling (`getDimensions`).
    pub fn dimensions(&self) -> (f64, f64) {
        let (mut w, mut h) = (self.kind.width, self.kind.height);
        if self.is_baby() {
            if let Some((bw, bh, _)) = self.kind.baby_dimensions {
                (w, h) = (bw, bh);
            }
        }
        let s = species::size_scale(self);
        (w * s, h * s)
    }
    pub fn width(&self) -> f32 {
        self.dimensions().0 as f32
    }
    pub fn height(&self) -> f32 {
        self.dimensions().1 as f32
    }
    pub fn eye_height(&self) -> f64 {
        if self.is_baby()
            && let Some((_, _, e)) = self.kind.baby_dimensions
        {
            return e;
        }
        self.kind.eye_height * species::size_scale(self)
    }
    pub fn eye_y(&self) -> f64 {
        self.position.y + self.eye_height()
    }
    pub fn eye(&self) -> [f64; 3] {
        [self.position.x, self.eye_y(), self.position.z]
    }
    pub fn pos(&self) -> [f64; 3] {
        [self.position.x, self.position.y, self.position.z]
    }
    pub fn block_pos(&self) -> [i32; 3] {
        [self.position.x.floor() as i32, self.position.y.floor() as i32, self.position.z.floor() as i32]
    }
    pub fn bounds(&self) -> Aabb {
        let (w, h) = self.dimensions();
        let w = w / 2.0;
        Aabb::new(
            Vec3::new(self.position.x - w, self.position.y, self.position.z - w),
            Vec3::new(self.position.x + w, self.position.y + h, self.position.z + w),
        )
    }
    pub fn alive(&self) -> bool {
        self.health > 0.0 && !self.removed
    }
    pub fn hostile(&self) -> bool {
        self.kind.category == Category::Monster
    }
    pub fn in_liquid(&self) -> bool {
        self.in_water || self.in_lava
    }
    pub fn max_up_step(&self) -> f32 {
        self.attributes.step_height as f32
    }
    pub fn max_head_y_rot(&self) -> i32 {
        species::max_head_y_rot(self)
    }
    pub fn max_head_x_rot(&self) -> i32 {
        species::max_head_x_rot(self)
    }
    pub fn head_rot_speed(&self) -> i32 {
        species::head_rot_speed(self)
    }
    pub fn ambient_sound_interval(&self) -> i32 {
        species::ambient_sound_interval(self)
    }
    pub fn distance_to_sqr(&self, p: [f64; 3]) -> f64 {
        (p[0] - self.position.x).powi(2) + (p[1] - self.position.y).powi(2) + (p[2] - self.position.z).powi(2)
    }
    fn distance_to(&self, p: [f64; 3]) -> f64 {
        self.distance_to_sqr(p).sqrt()
    }
    /// `getMaxFallDistance`.
    pub fn max_fall_distance(&self) -> i32 {
        species::max_fall_distance(self)
    }
    /// `PathMob` snapshot for the pathfinder.
    pub fn path_mob(&self) -> path::PathMob {
        path::PathMob {
            position: self.pos(),
            width: self.width(),
            height: self.height(),
            on_ground: self.on_ground,
            in_floatable_fluid: self.in_liquid(),
            max_up_step: self.max_up_step(),
            max_fall_distance: self.max_fall_distance(),
            malus: self.malus,
            can_float: self.nav.can_float,
            can_open_doors: self.nav.can_open_doors,
            can_pass_doors: self.nav.can_pass_doors,
            can_walk_over_fences: self.nav.can_walk_over_fences,
        }
    }
    pub fn nav_mob(&self, game_time: i64) -> nav::NavMob {
        nav::NavMob {
            path: self.path_mob(),
            in_liquid: self.in_liquid(),
            speed: self.speed,
            follow_range: self.attributes.follow_range as f32,
            game_time,
        }
    }
    /// `getWalkTargetValue`.
    pub fn walk_target_value(&self, cx: &Cx, pos: [i32; 3]) -> f32 {
        species::walk_target_value(self, cx, pos)
    }
    /// `getTarget()` resolved against the current snapshot (creative players
    /// and dead entities are not valid targets).
    pub fn target_view<'c>(&self, cx: &'c Cx) -> Option<&'c EntityView> {
        let v = cx.view(self.target?)?;
        (v.alive && !v.creative).then_some(v)
    }
    pub fn set_target(&mut self, t: Option<EntityRef>, cx: &Cx) {
        self.target = t.filter(|r| cx.view(*r).is_some_and(|v| !v.creative && species::can_attack(self, v)));
    }
    pub fn swing(&mut self) {
        if !self.swinging || self.swing_time >= species::swing_duration(self) / 2 || self.swing_time < 0 {
            self.swing_time = -1;
            self.swinging = true;
        }
    }
    /// Sound volume and pitch (`getSoundVolume`, `getVoicePitch`).
    pub fn make_sound(&mut self, name: &str, cx: &mut Cx) {
        if name.is_empty() {
            return;
        }
        let volume = species::sound_volume(self);
        let pitch = self.voice_pitch();
        let at = self.pos();
        cx.sound(name, at, volume, pitch);
    }
    pub fn voice_pitch(&mut self) -> f32 {
        let base = (self.random.float() - self.random.float()) * 0.2;
        if self.is_baby() { base + 1.5 } else { base + 1.0 }
    }
    /// Hurt sound event (without namespace).
    pub fn hurt_sound(&self) -> String {
        species::sound(self, species::SoundKind::Hurt)
    }
    pub fn death_sound(&self) -> String {
        species::sound(self, species::SoundKind::Death)
    }
    pub fn ambient_sound(&self) -> String {
        species::sound(self, species::SoundKind::Ambient)
    }

    /// Legacy entry point: damage from a position (treated as a player attack
    /// when a position is given, generic otherwise).
    pub fn hurt(&mut self, amount: f32, from: Option<[f64; 3]>, _random: &mut LegacyRandom) -> bool {
        let damage = match from {
            Some(f) => Damage::player(amount, f),
            None => Damage::plain(amount, DamageKind::Generic),
        };
        self.hurt_by(damage, 0)
    }
    /// `LivingEntity.hurtServer`: invulnerability frames, armor, knockback,
    /// revenge memory and panic. `game_time` stamps the last damage source.
    /// Sounds are emitted by the caller through [`Mob::hurt_sound`] /
    /// [`Mob::death_sound`] (or by [`tick_all`] for mob-on-mob damage).
    pub fn hurt_by(&mut self, damage: Damage, game_time: i64) -> bool {
        if !self.alive() || self.dead {
            return false;
        }
        if species::is_invulnerable_to(self, &damage) {
            return false;
        }
        self.no_action_time = 0;
        let mut amount = damage.amount.max(0.0);
        if !amount.is_finite() {
            amount = f32::MAX;
        }
        let mut full = true;
        if self.invulnerable as f32 > 10.0 && !damage.kind.bypasses_cooldown() {
            if amount <= self.last_hurt {
                return false;
            }
            let delta = amount - self.last_hurt;
            self.actually_hurt(&damage, delta);
            self.last_hurt = amount;
            full = false;
        } else {
            self.last_hurt = amount;
            self.invulnerable = 20;
            self.actually_hurt(&damage, amount);
            self.hurt_time = 10;
        }
        if let Some(a) = damage.attacker {
            self.last_hurt_by = Some(a);
            self.last_hurt_by_timestamp = self.age as i32;
            self.last_hurt_by_memory = 100;
            if a == EntityRef::Player {
                self.last_hurt_by_player_time = 100;
            }
        }
        if full && !damage.kind.no_knockback() {
            let (dx, dz) = match damage.from {
                Some(f) => (f[0] - self.position.x, f[2] - self.position.z),
                None => (0.0, 0.0),
            };
            self.knockback(0.4, dx, dz);
        }
        self.last_damage = Some((damage.kind, game_time));
        if self.health <= 0.0 {
            self.die(&damage);
        } else {
            self.ambient_sound_time = -self.ambient_sound_interval();
        }
        species::after_hurt(self, &damage);
        true
    }
    /// `actuallyHurt`: armor (`CombatRules.getDamageAfterAbsorb`) then health.
    fn actually_hurt(&mut self, damage: &Damage, amount: f32) {
        let mut amount = amount;
        if !matches!(damage.kind, DamageKind::Fall | DamageKind::Drown | DamageKind::OnFire | DamageKind::InFire | DamageKind::OutOfWorld | DamageKind::Magic) {
            let armor = self.attributes.armor as f32;
            let toughness = self.kind.attributes.armor_toughness as f32;
            let f = 2.0 + toughness / 4.0;
            let r = (armor - amount / f).clamp(armor * 0.2, 20.0);
            amount *= 1.0 - r / 25.0;
        }
        if self.species().is_animal() {
            self.in_love = 0;
        }
        self.health = (self.health - amount).max(0.0);
    }
    /// `LivingEntity.knockback`.
    pub fn knockback(&mut self, power: f64, mut xd: f64, mut zd: f64) {
        let power = power * (1.0 - self.attributes.knockback_resistance);
        if power <= 0.0 {
            return;
        }
        while xd * xd + zd * zd < 1.0e-5 {
            xd = (self.random.double() - self.random.double()) * 0.01;
            zd = (self.random.double() - self.random.double()) * 0.01;
        }
        let len = (xd * xd + zd * zd).sqrt();
        let (dx, dz) = (xd / len * power, zd / len * power);
        self.velocity = Vec3::new(
            self.velocity.x / 2.0 - dx,
            if self.on_ground { (self.velocity.y / 2.0 + power).min(0.4) } else { self.velocity.y },
            self.velocity.z / 2.0 - dz,
        );
    }
    fn die(&mut self, _damage: &Damage) {
        if self.dead {
            return;
        }
        self.dead = true;
        self.nav.stop();
    }
    /// `LivingEntity.getExperienceReward` for the death drop.
    fn experience_reward(&mut self) -> u32 {
        if self.is_baby() || self.last_hurt_by_player_time <= 0 {
            return 0;
        }
        species::experience(self)
    }

    /// Full entity tick (`Entity.tick` → `LivingEntity.tick` → `aiStep`).
    /// Returns false when the mob should be removed.
    pub fn tick(&mut self, cx: &mut Cx) -> bool {
        self.previous = self.position;
        self.previous_yaw = self.yaw;
        self.previous_pitch = self.pitch;
        self.previous_head_yaw = self.head_yaw;
        self.previous_body_yaw = self.body_yaw;
        self.previous_attack_anim = self.attack_anim;
        self.age += 1;
        species::pre_tick(self, cx);
        if self.removed {
            return false;
        }
        self.base_tick(cx);
        if self.dead || self.health <= 0.0 {
            self.dead = true;
            self.death_time += 1;
            // Keep falling while dying.
            self.xxa = 0.0;
            self.zza = 0.0;
            self.jumping = false;
            self.travel_tick(cx);
            self.swing_tick();
            return self.death_time < 20;
        }
        self.ai_step(cx);
        if self.removed {
            return false;
        }
        // LivingEntity.tick: head turn after aiStep.
        self.tick_body_rotation();
        self.yaw = wrap_rot_range(self.yaw, self.previous_yaw);
        self.head_pitch = self.pitch;
        self.update_walk_animation();
        self.swing_tick();
        species::post_tick(self, cx);
        self.idle = self.no_action_time.max(0) as u32;
        self.position.y > f64::from(cx.level.min_y()) - 64.0
    }
    /// `Entity.baseTick` + `LivingEntity.baseTick` + `Mob.baseTick`.
    fn base_tick(&mut self, cx: &mut Cx) {
        self.update_fluid_state(cx);
        if self.hurt_time > 0 {
            self.hurt_time -= 1;
        }
        if self.invulnerable > 0 {
            self.invulnerable -= 1;
        }
        if self.last_hurt_by_memory > 0 {
            self.last_hurt_by_memory -= 1;
            if self.last_hurt_by_memory == 0 {
                self.last_hurt_by = None;
            }
        }
        if self.last_hurt_by_player_time > 0 {
            self.last_hurt_by_player_time -= 1;
        }
        self.tick_fire(cx);
        self.tick_air(cx);
        // Mob.baseTick: ambient sound.
        if self.alive() && !self.dead && self.random.int(1000) < self.ambient_sound_time {
            self.ambient_sound_time = -self.ambient_sound_interval();
            let sound = self.ambient_sound();
            self.make_sound(&sound, cx);
        }
        self.ambient_sound_time += 1;
    }
    fn tick_fire(&mut self, cx: &mut Cx) {
        if self.in_lava {
            if !self.kind.fire_immune {
                self.fire_ticks = self.fire_ticks.max(300);
                let at = self.pos();
                if self.hurt_by(Damage::plain(4.0, DamageKind::Lava), cx.w.game_time) {
                    let s = if self.dead { self.death_sound() } else { self.hurt_sound() };
                    cx.sound(s, at, species::sound_volume(self), 1.0);
                }
            }
        }
        if self.fire_ticks > 0 {
            if self.kind.fire_immune {
                self.fire_ticks = (self.fire_ticks - 4).max(0);
            } else {
                if self.fire_ticks % 20 == 0 && !self.in_lava {
                    let at = self.pos();
                    if self.hurt_by(Damage::plain(1.0, DamageKind::OnFire), cx.w.game_time) {
                        let s = if self.dead { self.death_sound() } else { self.hurt_sound() };
                        cx.sound(s, at, species::sound_volume(self), 1.0);
                    }
                }
                self.fire_ticks -= 1;
            }
            if self.in_water {
                self.fire_ticks = 0;
            }
        }
    }
    fn tick_air(&mut self, cx: &mut Cx) {
        let water_breather = self.flags.contains(flags::WATER_ANIMAL);
        if water_breather {
            // WaterAnimal.handleAirSupply: suffocate out of water.
            if self.alive() && !self.in_water && !cx.w.raining {
                self.air_supply -= 1;
                if self.air_supply == -20 {
                    self.air_supply = 0;
                    self.hurt_by(Damage::plain(2.0, DamageKind::Drown), cx.w.game_time);
                }
            } else {
                self.air_supply = 300;
            }
            return;
        }
        if self.eye_in_water && !species::can_breathe_underwater(self) {
            self.air_supply -= 1;
            if self.air_supply == -20 {
                self.air_supply = 0;
                self.hurt_by(Damage::plain(2.0, DamageKind::Drown), cx.w.game_time);
            }
        } else if self.air_supply < 300 {
            self.air_supply = (self.air_supply + 4).min(300);
        }
    }
    /// `LivingEntity.aiStep` (with `Mob.aiStep` sun burning).
    fn ai_step(&mut self, cx: &mut Cx) {
        if self.no_jump_delay > 0 {
            self.no_jump_delay -= 1;
        }
        let v = self.velocity;
        let (mut dx, mut dy, mut dz) = (v.x, v.y, v.z);
        if dx.abs() < 0.003 {
            dx = 0.0;
        }
        if dz.abs() < 0.003 {
            dz = 0.0;
        }
        if dy.abs() < 0.003 {
            dy = 0.0;
        }
        self.velocity = Vec3::new(dx, dy, dz);
        // applyInput
        self.xxa *= 0.98;
        self.zza *= 0.98;
        self.server_ai_step(cx);
        self.jump_tick();
        self.travel_tick(cx);
        species::ai_step(self, cx);
        if self.flags.contains(flags::BURNS_IN_DAYLIGHT) {
            self.burn_undead(cx);
        }
        if self.flags.contains(flags::WATER_SENSITIVE) && (self.in_water || (cx.w.raining && self.in_rain(cx))) {
            let at = self.pos();
            if self.hurt_by(Damage::plain(1.0, DamageKind::Drown), cx.w.game_time) {
                let s = if self.dead { self.death_sound() } else { self.hurt_sound() };
                cx.sound(s, at, species::sound_volume(self), 1.0);
            }
        }
    }
    pub fn in_rain(&self, cx: &Cx) -> bool {
        let [x, y, z] = self.block_pos();
        cx.w.raining && cx.level.can_see_sky_top(x, y, z) && cx.level.can_see_sky_top(x, (self.position.y + f64::from(self.height())).floor() as i32, z)
    }
    /// `Mob.burnUndead` / `isSunBurnTick`.
    fn burn_undead(&mut self, cx: &mut Cx) {
        if !self.alive() || !cx.w.daylight || cx.w.dimension != Dimension::Overworld {
            return;
        }
        let br = self.light_dependent_magic_value(cx);
        let eye = [self.position.x.floor() as i32, self.eye_y().floor() as i32, self.position.z.floor() as i32];
        let wet = self.in_water || self.in_rain(cx);
        if br > 0.5 && self.random.float() * 30.0 < (br - 0.4) * 2.0 && !wet && cx.level.can_see_sky(eye[0], eye[1], eye[2]) {
            if self.head_item.is_some() {
                return;
            }
            self.fire_ticks = self.fire_ticks.max(160);
        }
    }
    /// `getLightLevelDependentMagicValue`: brightness at the eye-height block.
    pub fn light_dependent_magic_value(&self, cx: &Cx) -> f32 {
        let x = self.position.x.floor() as i32;
        let z = self.position.z.floor() as i32;
        let y = self.eye_y().floor() as i32;
        let raw = cx.level.raw_brightness(x, y, z, cx.w.sky_darken);
        level::brightness(cx.w.dimension, raw)
    }
    /// `Mob.serverAiStep`.
    fn server_ai_step(&mut self, cx: &mut Cx) {
        self.no_action_time += 1;
        let id_tick = self.age as i64 + i64::from(self.id);
        let full = !(id_tick % 2 != 0 && self.age > 1);
        let mut targets = std::mem::take(&mut self.targets);
        let mut goals = std::mem::take(&mut self.goals);
        if full {
            targets.tick(self, cx);
            goals.tick(self, cx);
        } else {
            targets.tick_running(self, cx, false);
            goals.tick_running(self, cx, false);
        }
        // Goals added during the tick (e.g. reassessed weapons) are merged.
        targets.absorb(std::mem::take(&mut self.targets));
        goals.absorb(std::mem::take(&mut self.goals));
        self.targets = targets;
        self.goals = goals;
        let level = cx.level;
        let m = self.nav_mob(cx.w.game_time);
        if let Some((at, speed)) = self.nav.tick(&level, &m) {
            self.move_control.set_wanted_position(at, speed);
        }
        species::custom_server_ai_step(self, cx);
        self.tick_move_control(&level);
        self.tick_look_control();
        self.tick_jump_control();
        if self.age % 5 == 0 {
            // updateControlFlags: no passengers/vehicles here, all enabled.
        }
    }
    /// `LivingEntity.aiStep` jump section.
    fn jump_tick(&mut self) {
        if self.jumping && !self.flags.contains(flags::IGNORES_FLUIDS) {
            let fluid_height = if self.in_lava { self.lava_height } else { self.water_height };
            let in_water_with_height = self.in_water && fluid_height > 0.0;
            let threshold = self.fluid_jump_threshold();
            if !in_water_with_height || (self.on_ground && fluid_height <= threshold) {
                if !self.in_lava || (self.on_ground && self.lava_height <= threshold) {
                    if (self.on_ground || (in_water_with_height && fluid_height <= threshold)) && self.no_jump_delay == 0 {
                        self.jump_from_ground();
                        self.no_jump_delay = 10;
                    }
                } else {
                    self.jump_in_liquid();
                }
            } else {
                self.jump_in_liquid();
            }
        } else {
            self.no_jump_delay = 0;
        }
    }
    pub fn fluid_jump_threshold(&self) -> f64 {
        if self.eye_height() < 0.4 { 0.0 } else { 0.4 }
    }
    /// `Mob.jumpInLiquid`: floating navigators swim up 0.04, others 0.3.
    fn jump_in_liquid(&mut self) {
        if self.nav.can_float {
            self.velocity.y += 0.04;
        } else {
            self.velocity.y += 0.3;
        }
    }
    pub fn jump_from_ground(&mut self) {
        let power = species::jump_power(self);
        if power <= 1.0e-5 {
            return;
        }
        self.velocity.y = f64::from(power).max(self.velocity.y);
    }
    fn swing_tick(&mut self) {
        let duration = species::swing_duration(self);
        if self.swinging {
            self.swing_time += 1;
            if self.swing_time >= duration {
                self.swing_time = 0;
                self.swinging = false;
            }
        } else {
            self.swing_time = 0;
        }
        self.attack_anim = self.swing_time as f32 / duration as f32;
    }
    /// `WalkAnimationState.update` (`updateWalkAnimation`).
    fn update_walk_animation(&mut self) {
        let dx = self.position.x - self.previous.x;
        let dy = if species::walk_animation_uses_y(self) { self.position.y - self.previous.y } else { 0.0 };
        let dz = self.position.z - self.previous.z;
        let distance = (dx * dx + dy * dy + dz * dz).sqrt() as f32;
        let target = (distance * 4.0).min(1.0);
        self.previous_limb_speed = self.limb_speed;
        self.limb_speed += (target - self.limb_speed) * 0.4;
        let scale = if self.is_baby() { 3.0 } else { 1.0 };
        self.limb_position += self.limb_speed * scale;
    }
    /// `Mob.checkDespawn` relative to the player.
    pub fn check_despawn(&mut self, cx: &Cx) -> bool {
        if cx.w.difficulty == Difficulty::Peaceful && self.hostile() {
            return false;
        }
        let persistent = self.persistent || species::requires_persistence(self);
        let Some(p) = cx.player() else { return true };
        let d2 = self.distance_to_sqr(p.position);
        let category = self.kind.category;
        let despawn = f64::from(category.despawn_distance());
        if !persistent && d2 > despawn * despawn && species::remove_when_far_away(self, d2) {
            return false;
        }
        let no_despawn = f64::from(category.no_despawn_distance());
        if !persistent && self.no_action_time > 600 && self.random.int(800) == 0 && d2 > no_despawn * no_despawn && species::remove_when_far_away(self, d2) {
            return false;
        } else if d2 < no_despawn * no_despawn {
            self.no_action_time = 0;
        }
        true
    }
    /// Snapshot for other mobs' searches.
    pub fn view(&self) -> EntityView {
        let (w, h) = self.dimensions();
        EntityView {
            r: EntityRef::Mob(self.id),
            species: Some(self.species()),
            category: self.kind.category,
            position: self.pos(),
            width: w,
            height: h,
            eye_height: self.eye_height(),
            alive: self.alive() && !self.dead,
            baby: self.is_baby(),
            age: self.age_ticks,
            in_love: self.in_love > 0,
            panicking: self.panicking,
            tame: self.data.tame,
            owned_by_player: self.data.tame,
            sitting: self.data.sitting,
            health: self.health,
            target: self.target,
            creative: false,
            in_water: self.in_water,
            on_ground: self.on_ground,
            variant: self.data.variant_index as u32,
            hostile: self.hostile(),
        }
    }
    /// Loot tables, experience and fixed drops for a finished death.
    fn death(&mut self) -> Death {
        let experience = self.experience_reward();
        let mut loot_tables = Vec::new();
        let mut extra_drops = Vec::new();
        if !self.is_baby() {
            loot_tables = species::loot_tables(self);
            extra_drops = species::extra_drops(self);
        }
        Death {
            id: self.id,
            kind: self.kind.id.clone(),
            position: self.pos(),
            killed_by_player: self.last_hurt_by_player_time > 0,
            loot_tables,
            experience,
            extra_drops,
        }
    }
}
/// `LivingEntity.tick` range checks keep yaw continuous with the previous tick.
fn wrap_rot_range(yaw: f32, _previous: f32) -> f32 {
    yaw
}

fn player_view_entity(p: &PlayerView) -> EntityView {
    EntityView {
        r: EntityRef::Player,
        species: None,
        category: Category::Misc,
        position: p.position,
        width: p.width,
        height: p.height,
        eye_height: p.eye_height,
        alive: p.alive,
        baby: false,
        age: 0,
        in_love: false,
        panicking: false,
        tame: false,
        owned_by_player: false,
        sitting: false,
        health: 20.0,
        target: None,
        creative: p.creative,
        in_water: false,
        on_ground: true,
        variant: 0,
        hostile: false,
    }
}

/// Tick every mob in vanilla order (entity list order), applying mob-on-mob
/// effects between mobs. Returns finished deaths; game-facing effects are
/// appended to `events`. Mobs in unloaded chunks are frozen; far, idle mobs
/// despawn per `Mob.checkDespawn`.
pub fn tick_all(mobs: &mut Vec<Mob>, w: &MobWorld, events: &mut Vec<MobEvent>) -> Vec<Death> {
    let mut entities: Vec<EntityView> = Vec::with_capacity(mobs.len() + 1);
    if let Some(p) = &w.player {
        entities.push(player_view_entity(p));
    }
    let offset = entities.len();
    entities.extend(mobs.iter().map(Mob::view));
    let mut keep = vec![true; mobs.len()];
    let mut deaths = Vec::new();
    let mut spawned: Vec<Mob> = Vec::new();
    let level = w.level();
    for i in 0..mobs.len() {
        if !w.world.loaded(mobs[i].position.x.floor() as i32, mobs[i].position.z.floor() as i32) {
            // Entities in non-ticking chunks: monsters despawn, others wait.
            keep[i] = !mobs[i].hostile() || mobs[i].persistent;
            continue;
        }
        let mut requests = Vec::new();
        {
            let mut cx = Cx { w, level, entities: &entities, events, requests: &mut requests, self_index: i };
            let mob = &mut mobs[i];
            let despawn_ok = mob.dead || mob.check_despawn(&cx);
            if !despawn_ok {
                keep[i] = false;
                mob.removed = true;
                continue;
            }
            keep[i] = mob.tick(&mut cx);
            if !keep[i] && mob.dead {
                deaths.push(mob.death());
            }
        }
        entities[offset + i] = mobs[i].view();
        for r in requests {
            match r {
                Request::Hurt { target: EntityRef::Mob(id), damage } => {
                    if let Some(j) = mobs.iter().position(|m| m.id == id) {
                        let at = mobs[j].pos();
                        if mobs[j].hurt_by(damage, w.game_time) {
                            let s = if mobs[j].dead { mobs[j].death_sound() } else { mobs[j].hurt_sound() };
                            let volume = species::sound_volume(&mobs[j]);
                            let pitch = mobs[j].voice_pitch();
                            events.push(MobEvent::Sound(s, at, volume, pitch));
                        }
                        entities[offset + j] = mobs[j].view();
                    }
                }
                Request::Hurt { target: EntityRef::Player, damage } => {
                    events.push(MobEvent::AttackPlayer(damage.amount, damage.from.unwrap_or(mobs[i].pos())));
                }
                Request::SetTarget { mob, target } => {
                    if let Some(j) = mobs.iter().position(|m| m.id == mob) {
                        mobs[j].target = target;
                        entities[offset + j] = mobs[j].view();
                    }
                }
                Request::Bred { partner } => {
                    if let Some(j) = mobs.iter().position(|m| m.id == partner) {
                        mobs[j].age_ticks = 6000;
                        mobs[j].in_love = 0;
                        entities[offset + j] = mobs[j].view();
                    }
                }
                Request::Spawn(m) => spawned.push(*m),
                Request::Knockback { target: EntityRef::Mob(id), strength, dx, dz } => {
                    if let Some(j) = mobs.iter().position(|m| m.id == id) {
                        mobs[j].knockback(strength, dx, dz);
                    }
                }
                Request::Knockback { .. } => {}
            }
        }
    }
    let mut index = 0;
    mobs.retain(|_| {
        index += 1;
        keep[index - 1]
    });
    mobs.extend(spawned);
    deaths
}

impl Mob {
    /// Frame snapshot for the renderer (`EntityRenderer.extractRenderState`).
    pub fn render_state(&self, alpha: f32) -> RenderState {
        let a = f64::from(alpha);
        let body = self.previous_body_yaw + wrap(self.body_yaw - self.previous_body_yaw) * alpha;
        let head = self.previous_head_yaw + wrap(self.head_yaw - self.previous_head_yaw) * alpha;
        let mut s = RenderState {
            kind: Arc::from(self.kind.id.as_str()),
            entity_id: self.id,
            position: [
                self.previous.x + (self.position.x - self.previous.x) * a,
                self.previous.y + (self.position.y - self.previous.y) * a,
                self.previous.z + (self.position.z - self.previous.z) * a,
            ],
            body_rot: body,
            head_yaw: wrap(head - body),
            head_pitch: self.previous_pitch + (self.pitch - self.previous_pitch) * alpha,
            age_in_ticks: self.age as f32 + alpha,
            walk_pos: self.limb_position - self.limb_speed * (1.0 - alpha) * if self.is_baby() { 3.0 } else { 1.0 },
            walk_speed: (self.previous_limb_speed + (self.limb_speed - self.previous_limb_speed) * alpha).min(1.0),
            scale: 1.0,
            baby: self.is_baby(),
            age_scale: if self.is_baby() { 0.5 } else { 1.0 },
            death_time: if self.death_time > 0 { self.death_time as f32 + alpha } else { 0.0 },
            hurt_overlay: self.hurt_time > 0 || self.death_time > 0,
            on_ground: self.on_ground,
            in_water: self.in_water,
            bb_width: self.width(),
            bb_height: self.height(),
            aggressive: self.aggressive,
            attack_time: {
                let mut d = self.attack_anim - self.previous_attack_anim;
                if d < 0.0 {
                    d += 1.0;
                }
                self.previous_attack_anim + d * alpha
            },
            main_hand: self.main_hand.clone(),
            in_love: self.in_love > 0,
            ..RenderState::default()
        };
        species::fill_render_state(self, alpha, &mut s);
        s
    }
}
/// Model geometry for a mob, posed for its animation state, as camera-relative
/// vertices textured from the shared atlas.
pub fn render(mob: &Mob, alpha: f32, camera: glam::DVec3, sprites: SpriteLookup, light: u32, out: &mut Vec<Vertex>) {
    render::render(&mob.render_state(alpha), &mob.kind.models, camera, sprites, light, out);
}

/// Fluid at a block (for callers outside the module).
pub fn fluid_at(w: &MobWorld, x: i32, y: i32, z: i32) -> Fluid {
    w.facts.get(w.world.block(x, y, z)).fluid
}
