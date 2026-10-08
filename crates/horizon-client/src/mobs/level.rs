//! World binding for mob AI: per-state pathfinding facts precomputed from the
//! vanilla block classes/tags, data-driven variants, biome tags, and the
//! [`GameLevel`] view (`PathLevel`, light, sky, line of sight).
use super::{
    Dimension,
    path::{PathLevel, PathType},
};
use crate::{blocks::BlockFacts, collisions::SceneCollisions, world::World};
use horizon_core::{
    geometry::{Aabb, Axis, Vec3, VoxelShape, traverse_blocks},
    position::BlockPos,
};
use horizon_world::{block_collision::BlockCollisionRegistry, level::Behaviours, registry::BlockStateId};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::Arc,
};

/// Pathfinding-relevant facts of one block state.
#[derive(Clone, Copy, Debug)]
pub struct PathInfo {
    /// `WalkNodeEvaluator.getPathTypeFromState`.
    pub path_type: PathType,
    /// Collision shape `max(Y)`, NaN when the shape is empty.
    pub collision_top: f32,
    pub air: bool,
    pub pathfindable_land: bool,
    /// Legacy `isSolid`.
    pub solid: bool,
    pub solid_render: bool,
    pub door_or_fence: bool,
    pub cauldron: bool,
    /// Water or lava fluid state (`#entity_floatable`).
    pub floatable: bool,
    pub water: bool,
    pub lava: bool,
    /// Full-block collision shape.
    pub full_collision: bool,
    /// `isFaceSturdy(UP)`.
    pub sturdy_up: bool,
    pub emission: u8,
    /// `BlockBehaviour.Properties.isValidSpawn` override.
    pub spawn_rule: SpawnRule,
}

/// `isValidSpawn` predicates (`Blocks::never`, `always`, `ocelotOrParrot`...).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnRule {
    /// `isFaceSturdy(UP) && lightEmission < 14`.
    Default,
    Never,
    Always,
    OcelotOrParrot,
    PolarBear,
    FireImmune,
}
impl PathInfo {
    /// `BlockState.isValidSpawn(level, pos, type)`.
    pub fn is_valid_spawn(&self, species: super::Species, fire_immune: bool) -> bool {
        match self.spawn_rule {
            SpawnRule::Default => self.sturdy_up && self.emission < 14,
            SpawnRule::Never => false,
            SpawnRule::Always => true,
            SpawnRule::OcelotOrParrot => matches!(species.name(), "ocelot" | "parrot"),
            SpawnRule::PolarBear => species.name() == "polar_bear",
            SpawnRule::FireImmune => fire_immune,
        }
    }
}
/// A data-driven variant (`pig_variant`, `cow_variant`, `wolf_variant`...).
#[derive(Clone, Debug)]
pub struct Variant {
    pub id: String,
    /// Asset (texture) id without namespace, adult.
    pub asset: String,
    pub baby_asset: Option<String>,
    /// Wolf: wild/tame/angry assets (adult, baby).
    pub wolf_assets: Option<[(String, String); 3]>,
    pub model: Option<String>,
    /// (condition, priority): `None` = always.
    pub selectors: Vec<(Option<Condition>, i32)>,
}
#[derive(Clone, Debug)]
pub enum Condition {
    /// Biome in a set (tag already resolved).
    Biome(HashSet<String>),
    /// Structure check (not evaluated: treated as false).
    Structure,
    /// Moon brightness range.
    MoonBrightness(f64, f64),
}
/// Sound set of a sound variant (adult, baby).
#[derive(Clone, Debug, Default)]
pub struct SoundSet {
    pub ambient: String,
    pub hurt: String,
    pub death: String,
    pub step: String,
    pub eat: String,
}
#[derive(Clone, Debug, Default)]
pub struct SoundVariant {
    pub id: String,
    pub adult: SoundSet,
    pub baby: SoundSet,
}

pub struct MobRules {
    pub path: Vec<PathInfo>,
    pub names: Vec<Arc<str>>,
    pub block_tags: HashMap<String, HashSet<BlockStateId>>,
    pub item_tags: HashMap<String, HashSet<String>>,
    pub biome_tags: HashMap<String, HashSet<String>>,
    pub variants: HashMap<&'static str, Vec<Variant>>,
    pub sound_variants: HashMap<&'static str, Vec<SoundVariant>>,
    pub defaults: HashMap<String, BlockStateId>,
    /// `natural_mob_spawns` per biome and category plus spawn costs.
    pub spawn_costs: HashMap<String, HashMap<String, (f64, f64)>>,
    /// Dimension monster spawn light rules: (block light limit, light test max).
    pub monster_spawn_block_light_limit: [i32; 3],
    pub monster_spawn_light_max: [i32; 3],
}
impl Default for MobRules {
    fn default() -> Self {
        Self {
            path: Vec::new(),
            names: Vec::new(),
            block_tags: HashMap::new(),
            item_tags: HashMap::new(),
            biome_tags: HashMap::new(),
            variants: HashMap::new(),
            sound_variants: HashMap::new(),
            defaults: HashMap::new(),
            spawn_costs: HashMap::new(),
            monster_spawn_block_light_limit: [0, 15, 0],
            monster_spawn_light_max: [7, 7, 0],
        }
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
/// Resolve a tag directory (`data/minecraft/tags/<kind>`) into flat sets of ids.
fn load_tag_dir(dir: &Path) -> HashMap<String, HashSet<String>> {
    let mut raw: HashMap<String, Vec<String>> = HashMap::new();
    fn walk(base: &Path, dir: &Path, raw: &mut HashMap<String, Vec<String>>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, raw);
            } else if p.extension().is_some_and(|x| x == "json") {
                let Some(rel) = p.strip_prefix(base).ok().and_then(|r| r.with_extension("").to_str().map(|s| s.replace('\\', "/"))) else { continue };
                let Some(v) = read_json(&p) else { continue };
                let values = v["values"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().map(str::to_owned).or_else(|| x["id"].as_str().map(str::to_owned)))
                    .collect();
                raw.insert(format!("minecraft:{rel}"), values);
            }
        }
    }
    walk(dir, dir, &mut raw);
    fn resolve(name: &str, raw: &HashMap<String, Vec<String>>, out: &mut HashSet<String>, depth: u32) {
        if depth > 16 {
            return;
        }
        for v in raw.get(name).into_iter().flatten() {
            if let Some(t) = v.strip_prefix('#') {
                resolve(t, raw, out, depth + 1);
            } else {
                out.insert(v.clone());
            }
        }
    }
    raw.keys()
        .map(|k| {
            let mut s = HashSet::new();
            resolve(k, &raw, &mut s, 0);
            (k.clone(), s)
        })
        .collect()
}

impl MobRules {
    /// Build from the prepared client resources and the loaded block behaviours.
    pub fn load(root: &Path, behaviours: &Behaviours, registry: &horizon_world::registry::BlockRegistry, facts: &BlockFacts, collision: &BlockCollisionRegistry) -> crate::Result<Self> {
        let behaviour_json = read_json(&root.join("block-behaviours.json")).ok_or("missing block-behaviours.json")?;
        let mut classes: HashMap<String, Vec<String>> = HashMap::new();
        for b in behaviour_json["blocks"].as_array().into_iter().flatten() {
            let name = b["name"].as_str().unwrap_or("").to_owned();
            let chain = b["class"].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::to_owned)).collect();
            classes.insert(name, chain);
        }
        let block_tags_raw = load_tag_dir(&root.join("data/minecraft/tags/block"));
        let item_tags = load_tag_dir(&root.join("data/minecraft/tags/item"));
        let biome_tags = load_tag_dir(&root.join("data/minecraft/tags/worldgen/biome"));
        let tag_has = |tag: &str, name: &str| block_tags_raw.get(tag).is_some_and(|s| s.contains(name));
        let mut path = Vec::with_capacity(facts.states.len());
        let mut names = Vec::with_capacity(facts.states.len());
        let mut block_tags: HashMap<String, HashSet<BlockStateId>> = HashMap::new();
        let wanted_tags = [
            "minecraft:edible_for_sheep",
            "minecraft:animals_spawnable_on",
            "minecraft:wolves_spawnable_on",
            "minecraft:rabbits_spawnable_on",
            "minecraft:foxes_spawnable_on",
            "minecraft:parrots_spawnable_on",
            "minecraft:goats_spawnable_on",
            "minecraft:mooshrooms_spawnable_on",
            "minecraft:polar_bears_spawnable_on_alternate",
            "minecraft:enderman_holdable",
            "minecraft:valid_spawn",
            "minecraft:infiniburn_overworld",
            "minecraft:strider_warm_blocks",
            "minecraft:dirt",
            "minecraft:sand",
            "minecraft:snow",
            "minecraft:ice",
            "minecraft:leaves",
            "minecraft:logs",
            "minecraft:fences",
            "minecraft:walls",
            "minecraft:fence_gates",
            "minecraft:doors",
            "minecraft:trapdoors",
            "minecraft:wool",
            "minecraft:beds",
            "minecraft:prevent_mob_spawning_inside",
            "minecraft:azalea_grows_on",
            "minecraft:axolotls_spawnable_on",
            "minecraft:frogs_spawnable_on",
            "minecraft:armadillo_spawnable_on",
            "minecraft:camels_spawnable_on",
            "minecraft:bee_attractive",
            "minecraft:flowers",
            "minecraft:wither_immune",
            "minecraft:dragon_immune",
        ];
        for (i, f) in facts.states.iter().enumerate() {
            let id = BlockStateId(i as u32);
            let name: &str = &f.name;
            names.push(f.name.clone());
            for t in wanted_tags {
                if tag_has(t, name) {
                    block_tags.entry(t.to_owned()).or_default().insert(id);
                }
            }
            let chain = classes.get(name).cloned().unwrap_or_default();
            let is = |c: &str| chain.iter().any(|x| x == c);
            let state = registry.state(id);
            let prop = |k: &str| state.and_then(|s| s.properties.get(k)).map(String::as_str);
            let open = prop("open") == Some("true");
            let air = facts.is_air(id);
            let fluid = behaviours.fluid_kind(id);
            let (water, lava) = match f.fluid {
                crate::blocks::Fluid::Water => (true, false),
                crate::blocks::Fluid::Lava => (false, true),
                crate::blocks::Fluid::None => (false, false),
            };
            let _ = fluid;
            let shape: Option<VoxelShape> = behaviours.collision_shape(id).cloned().or_else(|| {
                // Uncached (dynamic) shapes: fall back to the static collision table.
                collision.cached_shapes().shape(id).ok().flatten().cloned()
            });
            let collision_top = match &shape {
                Some(s) if !s.is_empty() => s.max(Axis::Y) as f32,
                Some(_) => f32::NAN,
                None => {
                    if f.full_collision {
                        1.0
                    } else {
                        f32::NAN
                    }
                }
            };
            let full_collision = f.full_collision;
            // BlockBehaviour.isPathfindable(LAND) and overrides.
            let never = [
                "AbstractBedBlock",
                "AbstractCauldronBlock",
                "AbstractSkullBlock",
                "AnvilBlock",
                "AzaleaBlock",
                "BambooStalkBlock",
                "BellBlock",
                "BrewingStandBlock",
                "CactusBlock",
                "CakeBlock",
                "CampfireBlock",
                "CandleCakeBlock",
                "ChainBlock",
                "ChestBlock",
                "ChorusPlantBlock",
                "CocoaBlock",
                "ComposterBlock",
                "ConduitBlock",
                "CopperGolemStatueBlock",
                "CrossCollisionBlock",
                "DecoratedPotBlock",
                "DragonEggBlock",
                "DriedGhastBlock",
                "EnchantingTableBlock",
                "EnderChestBlock",
                "EndPortalFrameBlock",
                "FarmlandBlock",
                "FenceBlock",
                "FlowerPotBlock",
                "GrindstoneBlock",
                "HeavyCoreBlock",
                "HopperBlock",
                "LanternBlock",
                "LecternBlock",
                "MudBlock",
                "PathBlock",
                "MovingPistonBlock",
                "PistonBaseBlock",
                "PistonHeadBlock",
                "RespawnAnchorBlock",
                "RodBlock",
                "SculkSensorBlock",
                "SeaPickleBlock",
                "ShelfBlock",
                "ShelfMushroomBlock",
                "SlabBlock",
                "SnifferEggBlock",
                "SoulSandBlock",
                "SpeleothemBlock",
                "StairBlock",
                "StonecutterBlock",
                "WallBlock",
                "WallHangingSignBlock",
            ];
            let pathfindable_land = if is("DoorBlock") || is("FenceGateBlock") || is("TrapDoorBlock") {
                open
            } else if is("LiquidBlock") {
                !lava
            } else if is("PowderSnowBlock") {
                true
            } else if is("SnowLayerBlock") {
                prop("layers").and_then(|l| l.parse::<i32>().ok()).unwrap_or(1) < 5
            } else if never.iter().any(|c| is(c)) {
                false
            } else {
                !full_collision
            };
            let lit = prop("lit") == Some("true");
            let path_type = if air {
                PathType::Open
            } else if tag_has("minecraft:trapdoors", name) || name == "minecraft:lily_pad" || name == "minecraft:big_dripleaf" {
                PathType::Trapdoor
            } else if name == "minecraft:powder_snow" {
                PathType::PowderSnow
            } else if name == "minecraft:cactus" || name == "minecraft:sweet_berry_bush" {
                PathType::Damaging
            } else if name == "minecraft:honey_block" {
                PathType::StickyHoney
            } else if name == "minecraft:cocoa" {
                PathType::Cocoa
            } else if name == "minecraft:wither_rose" || tag_has("minecraft:speleothems", name) || name == "minecraft:pointed_dripstone" {
                PathType::DamageCautious
            } else if lava {
                PathType::Lava
            } else if tag_has("minecraft:fire", name) || name == "minecraft:magma_block" || (tag_has("minecraft:campfires", name) && lit) || name == "minecraft:lava_cauldron" {
                PathType::Fire
            } else if is("DoorBlock") {
                if open {
                    PathType::DoorOpen
                } else if behaviours.data(id).set_type.as_ref().is_some_and(|t| !t.can_open_by_hand) {
                    PathType::DoorIronClosed
                } else {
                    PathType::DoorWoodClosed
                }
            } else if is("BaseRailBlock") {
                PathType::Rail
            } else if is("LeavesBlock") {
                PathType::Leaves
            } else if tag_has("minecraft:fences", name) || tag_has("minecraft:walls", name) || (is("FenceGateBlock") && !open) {
                PathType::Fence
            } else if !pathfindable_land {
                PathType::Blocked
            } else if water {
                PathType::Water
            } else {
                PathType::Open
            };
            let spawn_rule = if name.ends_with("_leaves") || name == "minecraft:firefly_bush" {
                SpawnRule::OcelotOrParrot
            } else if matches!(name, "minecraft:ice" | "minecraft:frosted_ice") {
                SpawnRule::PolarBear
            } else if name == "minecraft:magma_block" {
                SpawnRule::FireImmune
            } else if matches!(name, "minecraft:soul_sand" | "minecraft:carved_pumpkin" | "minecraft:jack_o_lantern" | "minecraft:redstone_lamp" | "minecraft:mud") {
                SpawnRule::Always
            } else if matches!(
                name,
                "minecraft:bedrock" | "minecraft:glass" | "minecraft:tinted_glass" | "minecraft:moving_piston" | "minecraft:repeater" | "minecraft:barrier" | "minecraft:chorus_flower" | "minecraft:scaffolding"
            ) || name.ends_with("_stained_glass")
                || name.ends_with("trapdoor")
                || name.ends_with("copper_grate")
            {
                SpawnRule::Never
            } else {
                SpawnRule::Default
            };
            path.push(PathInfo {
                path_type,
                collision_top,
                air,
                pathfindable_land,
                solid: behaviours.is_solid(id),
                solid_render: f.solid_render,
                door_or_fence: tag_has("minecraft:doors", name) || tag_has("minecraft:fences", name),
                cauldron: tag_has("minecraft:cauldrons", name),
                floatable: water || lava,
                water,
                lava,
                full_collision,
                sturdy_up: behaviours.is_face_sturdy(id, horizon_core::geometry::Direction::Up),
                emission: f.emission,
                spawn_rule,
            });
        }
        let mut defaults = HashMap::new();
        for name in ["minecraft:grass_block", "minecraft:dirt", "minecraft:air", "minecraft:short_grass", "minecraft:mycelium", "minecraft:water"] {
            if let Some(id) = registry.default_state(name) {
                defaults.insert(name.to_owned(), id);
            }
        }
        let mut rules = Self {
            path,
            names,
            block_tags,
            item_tags,
            biome_tags,
            defaults,
            ..Self::default()
        };
        for kind in ["pig", "cow", "chicken", "wolf", "cat", "frog"] {
            rules.variants.insert(kind, rules.load_variants(root, kind));
        }
        for kind in ["pig", "cow", "chicken", "wolf", "cat"] {
            rules.sound_variants.insert(kind, load_sound_variants(root, kind));
        }
        for (i, dim) in ["overworld", "the_nether", "the_end"].iter().enumerate() {
            if let Some(v) = read_json(&root.join(format!("data/minecraft/dimension_type/{dim}.json"))) {
                rules.monster_spawn_block_light_limit[i] = v["monster_spawn_block_light_limit"].as_i64().unwrap_or(0) as i32;
                let light = &v["monster_spawn_light_level"];
                rules.monster_spawn_light_max[i] = match light {
                    Value::Number(n) => n.as_i64().unwrap_or(0) as i32,
                    Value::Object(o) => o.get("max_inclusive").and_then(Value::as_i64).unwrap_or(7) as i32,
                    _ => 7,
                };
            }
        }
        Ok(rules)
    }
    fn load_variants(&self, root: &Path, kind: &str) -> Vec<Variant> {
        let dir = root.join(format!("data/minecraft/{kind}_variant"));
        let mut files: Vec<_> = fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        files.sort();
        let strip = |s: &str| s.trim_start_matches("minecraft:").to_owned();
        files
            .iter()
            .filter_map(|p| {
                let v = read_json(p)?;
                let id = format!("minecraft:{}", p.file_stem()?.to_str()?);
                let wolf_assets = v.get("assets").map(|a| {
                    let b = &v["baby_assets"];
                    ["wild", "tame", "angry"].map(|k| (strip(a[k].as_str().unwrap_or("")), strip(b[k].as_str().unwrap_or(""))))
                });
                let selectors = v["spawn_conditions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|s| {
                        let priority = s["priority"].as_i64().unwrap_or(0) as i32;
                        let c = &s["condition"];
                        let cond = match c["type"].as_str() {
                            Some("minecraft:biome") => {
                                let mut set = HashSet::new();
                                match &c["biomes"] {
                                    Value::String(t) if t.starts_with('#') => {
                                        set.extend(self.biome_tags.get(&t[1..]).into_iter().flatten().cloned());
                                    }
                                    Value::String(b) => {
                                        set.insert(b.clone());
                                    }
                                    Value::Array(list) => set.extend(list.iter().filter_map(|x| x.as_str().map(str::to_owned))),
                                    _ => {}
                                }
                                Some(Condition::Biome(set))
                            }
                            Some("minecraft:structure") => Some(Condition::Structure),
                            Some("minecraft:moon_brightness") => {
                                let r = &c["range"];
                                let (min, max) = match r {
                                    Value::Number(n) => (n.as_f64().unwrap_or(0.0), n.as_f64().unwrap_or(1.0)),
                                    _ => (r["min"].as_f64().unwrap_or(f64::MIN), r["max"].as_f64().unwrap_or(f64::MAX)),
                                };
                                Some(Condition::MoonBrightness(min, max))
                            }
                            _ => None,
                        };
                        (cond, priority)
                    })
                    .collect();
                Some(Variant {
                    id,
                    asset: v["asset_id"].as_str().map(strip).unwrap_or_default(),
                    baby_asset: v["baby_asset_id"].as_str().map(strip),
                    wolf_assets,
                    model: v["model"].as_str().map(str::to_owned),
                    selectors,
                })
            })
            .collect()
    }
    /// `VariantUtils.selectVariantToSpawn` / `PriorityProvider.pick`.
    pub fn select_variant(&self, kind: &str, biome: &str, moon_brightness: f64, random: &mut impl super::Rand) -> Option<usize> {
        let list = self.variants.get(kind)?;
        let mut entries: Vec<(usize, i32, &Option<Condition>)> = Vec::new();
        for (i, v) in list.iter().enumerate() {
            for (c, p) in &v.selectors {
                entries.push((i, *p, c));
            }
        }
        // Stable sort, highest priority first.
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        let mut highest = i32::MIN;
        let mut selected = Vec::new();
        for (i, p, c) in entries {
            if p < highest {
                continue;
            }
            let ok = match c {
                None => true,
                Some(Condition::Biome(set)) => set.contains(biome),
                Some(Condition::Structure) => false,
                Some(Condition::MoonBrightness(min, max)) => moon_brightness >= *min && moon_brightness <= *max,
            };
            if ok {
                highest = p;
                selected.push(i);
            }
        }
        if selected.is_empty() {
            return None;
        }
        Some(selected[random.int(selected.len() as i32) as usize])
    }
    pub fn variant(&self, kind: &str, index: usize) -> Option<&Variant> {
        self.variants.get(kind)?.get(index)
    }
    pub fn sound_variant(&self, kind: &str, index: usize) -> Option<&SoundVariant> {
        self.sound_variants.get(kind)?.get(index)
    }
    /// `pickRandomSoundVariant`: uniform over the registry.
    pub fn random_sound_variant(&self, kind: &str, random: &mut impl super::Rand) -> usize {
        let n = self.sound_variants.get(kind).map_or(0, Vec::len);
        if n == 0 { 0 } else { random.int(n as i32) as usize }
    }
    pub fn item_in_tag(&self, tag: &str, item: &str) -> bool {
        self.item_tags.get(tag).is_some_and(|s| s.contains(item))
    }
    pub fn block_in_tag(&self, tag: &str, state: BlockStateId) -> bool {
        self.block_tags.get(tag).is_some_and(|s| s.contains(&state))
    }
    pub fn info(&self, state: BlockStateId) -> &PathInfo {
        &self.path[state.0 as usize]
    }
    pub fn name(&self, state: BlockStateId) -> &str {
        &self.names[state.0 as usize]
    }
    pub fn default_state(&self, name: &str) -> Option<BlockStateId> {
        self.defaults.get(name).copied()
    }
}
fn load_sound_variants(root: &Path, kind: &str) -> Vec<SoundVariant> {
    let dir = root.join(format!("data/minecraft/{kind}_sound_variant"));
    let mut files: Vec<_> = fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    let strip = |v: &Value| v.as_str().unwrap_or("").trim_start_matches("minecraft:").to_owned();
    let set = |v: &Value| SoundSet {
        ambient: strip(&v["ambient_sound"]),
        hurt: strip(&v["hurt_sound"]),
        death: strip(&v["death_sound"]),
        step: strip(&v["step_sound"]),
        eat: strip(&v["eat_sound"]),
    };
    files
        .iter()
        .filter_map(|p| {
            let v = read_json(p)?;
            let id = format!("minecraft:{}", p.file_stem()?.to_str()?);
            let (adult, baby) = if v.get("adult_sounds").is_some() { (set(&v["adult_sounds"]), set(&v["baby_sounds"])) } else { (set(&v), set(&v)) };
            Some(SoundVariant { id, adult, baby })
        })
        .collect()
}

/// `DimensionType.getBrightness(ambientLight, level)`.
pub fn brightness(dimension: Dimension, raw: i32) -> f32 {
    let ambient = match dimension {
        Dimension::Nether => 0.1,
        _ => 0.0,
    };
    let v = raw as f32 / 15.0;
    let curved = v / (4.0 - 3.0 * v);
    curved + (1.0 - curved) * ambient
}

/// Read-only world view for mob logic.
#[derive(Clone, Copy)]
pub struct GameLevel<'a> {
    pub world: &'a World,
    pub facts: &'a BlockFacts,
    pub collision: &'a BlockCollisionRegistry<'a>,
    pub rules: &'a MobRules,
}
impl GameLevel<'_> {
    pub fn state(&self, x: i32, y: i32, z: i32) -> BlockStateId {
        self.world.block(x, y, z)
    }
    pub fn info_at(&self, x: i32, y: i32, z: i32) -> &PathInfo {
        self.rules.info(self.state(x, y, z))
    }
    pub fn name_at(&self, x: i32, y: i32, z: i32) -> &str {
        self.rules.name(self.state(x, y, z))
    }
    pub fn sky_light(&self, x: i32, y: i32, z: i32) -> i32 {
        i32::from(self.world.light(x, y, z) >> 4)
    }
    pub fn block_light(&self, x: i32, y: i32, z: i32) -> i32 {
        i32::from(self.world.light(x, y, z) & 15)
    }
    /// `getRawBrightness(pos, skyDarken)`.
    pub fn raw_brightness(&self, x: i32, y: i32, z: i32, sky_darken: i32) -> i32 {
        (self.sky_light(x, y, z) - sky_darken).max(self.block_light(x, y, z))
    }
    /// `canSeeSky` (full sky light) for the block itself.
    pub fn can_see_sky_top(&self, x: i32, y: i32, z: i32) -> bool {
        self.sky_light(x, y, z) >= 15
    }
    /// `Level.isRainingAt`-style check helper.
    pub fn min_y_value(&self) -> i32 {
        crate::chunk::MIN_Y
    }
    /// `LivingEntity.hasLineOfSight`: collider clip from eye to eye.
    pub fn line_of_sight(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        if (to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2) + (to[2] - from[2]).powi(2) > 128.0 * 128.0 {
            return false;
        }
        let (a, b) = (Vec3::new(from[0], from[1], from[2]), Vec3::new(to[0], to[1], to[2]));
        let hit = traverse_blocks(a, b, 4096, |p: BlockPos| {
            let state = self.state(p.x, p.y, p.z);
            let info = self.rules.info(state);
            if info.collision_top.is_nan() {
                return None;
            }
            let shape = self.collision.cached_shapes().shape(state).ok().flatten().cloned().unwrap_or_else(VoxelShape::block);
            shape.clip(a, b, p).map(|_| ())
        });
        !matches!(hit, Ok(Some(())))
    }
    /// Biome name at a block position.
    pub fn biome_at<'b>(&self, biomes: &'b crate::biome::Biomes, x: i32, y: i32, z: i32) -> &'b str {
        match self.world.chunks.get(&(x >> 4, z >> 4)) {
            Some(c) => &biomes.get(c.biome(x & 15, y, z & 15)).name,
            None => "minecraft:plains",
        }
    }
}
impl PathLevel for GameLevel<'_> {
    fn path_type_from_state(&self, x: i32, y: i32, z: i32) -> PathType {
        self.info_at(x, y, z).path_type
    }
    fn collision_top(&self, x: i32, y: i32, z: i32) -> Option<f64> {
        let t = self.info_at(x, y, z).collision_top;
        (!t.is_nan()).then_some(f64::from(t))
    }
    fn has_collision(&self, aabb: Aabb) -> bool {
        let mut scene = SceneCollisions { world: self.world, binding: self.collision };
        !scene.unobstructed(aabb).unwrap_or(false)
    }
    fn is_air(&self, x: i32, y: i32, z: i32) -> bool {
        self.info_at(x, y, z).air
    }
    fn is_pathfindable_land(&self, x: i32, y: i32, z: i32) -> bool {
        self.info_at(x, y, z).pathfindable_land
    }
    fn is_floatable_fluid(&self, x: i32, y: i32, z: i32) -> bool {
        self.info_at(x, y, z).floatable
    }
    fn is_water(&self, x: i32, y: i32, z: i32) -> bool {
        self.info_at(x, y, z).water
    }
    fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
        self.info_at(x, y, z).solid
    }
    fn is_solid_render(&self, x: i32, y: i32, z: i32) -> bool {
        self.info_at(x, y, z).solid_render
    }
    fn is_door_or_fence(&self, x: i32, y: i32, z: i32) -> bool {
        self.info_at(x, y, z).door_or_fence
    }
    fn is_cauldron(&self, x: i32, y: i32, z: i32) -> bool {
        self.info_at(x, y, z).cauldron
    }
    fn can_see_sky(&self, x: i32, y: i32, z: i32) -> bool {
        self.can_see_sky_top(x, y, z)
    }
    fn min_y(&self) -> i32 {
        crate::chunk::MIN_Y
    }
    fn max_y(&self) -> i32 {
        crate::chunk::MIN_Y + crate::chunk::SECTIONS as i32 * 16 - 1
    }
    fn loaded(&self, x: i32, z: i32) -> bool {
        self.world.loaded(x, z)
    }
}
