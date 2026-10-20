//! A companion's senses in the game (companion/perception.rs): what its
//! eye reaches each tick, the world memory it keeps and saves beside its
//! file (`companions/<uuid>.knowledge`), and the one place skills ask for
//! blocks and structures, so that the perception level decides what they
//! may know: Fair and Scout only what it saw, Sharp senses also through
//! walls within 16 blocks, All-knowing the loaded world. Scout and
//! All-knowing also know where structures are, through `/locate`'s code.
use super::{Game, companion::Body, companion_work::BlockSearch};
use horizon_client::{
    blocks::Fluid,
    companion::perception::{
        KnowledgeFile, Keep, Look, PerceptionLevel, SIGHT, Senses, SightWorld,
    },
    world::World,
};
use std::{path::PathBuf, sync::Arc};

/// Rays its eye casts a tick (a full turn of 4096 directions in ~3 s).
pub(super) const RAYS: u32 = 64;

/// Per block state: how sight passes it, whether it is remembered.
pub(super) struct SightTable {
    look: Vec<Look>,
    keep: Vec<Keep>,
    /// Each block name's first state, to read a saved memory back.
    first: std::collections::HashMap<String, u32>,
}

/// Blocks worth remembering for themselves.
fn notable(name: &str) -> bool {
    let n = name.trim_start_matches("minecraft:");
    n.ends_with("_ore")
        || n.ends_with("_log")
        || n.ends_with("_stem")
        || n.ends_with("_bed")
        || n.ends_with("chest")
        || n.ends_with("_nylium")
        || n.starts_with("infested_")
        || n.contains("stone_bricks")
        || n.contains("nether_brick")
        || matches!(
            n,
            "ancient_debris"
                | "barrel"
                | "crafting_table"
                | "furnace"
                | "blast_furnace"
                | "smoker"
                | "obsidian"
                | "crying_obsidian"
                | "nether_portal"
                | "end_portal_frame"
                | "end_portal"
                | "spawner"
                | "bookshelf"
                | "bell"
                | "hay_block"
                | "gravel"
                | "clay"
                | "sugar_cane"
                | "pumpkin"
                | "melon"
                | "magma_block"
                | "glowstone"
                | "soul_sand"
                | "wheat"
                | "carrots"
                | "potatoes"
                | "beetroots"
                | "sweet_berry_bush"
                | "gilded_blackstone"
                | "gold_block"
                | "iron_bars"
        )
}

struct GameSight<'a> {
    world: &'a World,
    table: &'a SightTable,
    min_y: i32,
    max_y: i32,
}

impl SightWorld for GameSight<'_> {
    fn state(&self, p: [i32; 3]) -> Option<u32> {
        if p[1] < self.min_y || p[1] >= self.max_y || !self.world.loaded(p[0], p[2]) {
            return None;
        }
        Some(self.world.block(p[0], p[1], p[2]).0)
    }
    fn look(&self, s: u32) -> Look {
        self.table.look.get(s as usize).copied().unwrap_or(Look::Opaque)
    }
    fn keep(&self, s: u32) -> Keep {
        self.table.keep.get(s as usize).copied().unwrap_or(Keep::No)
    }
}

/// Where a companion's knowledge is saved.
pub(super) fn knowledge_path(world: &std::path::Path, uuid: &str) -> PathBuf {
    horizon_client::companion::file::dir(world).join(format!("{uuid}.knowledge"))
}

impl Game<'_> {
    /// The sight table, built once.
    pub(super) fn companion_sight_table(&mut self) -> Arc<SightTable> {
        if let Some(t) = &self.companions.sight {
            return t.clone();
        }
        let facts = self.res.facts();
        let n = facts.states.len();
        let mut look = Vec::with_capacity(n);
        let mut keep = Vec::with_capacity(n);
        let mut first = std::collections::HashMap::new();
        for (i, f) in facts.states.iter().enumerate() {
            first.entry(f.name.to_string()).or_insert(i as u32);
            let air = matches!(&*f.name, "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air");
            let (l, k) = if air {
                (Look::Clear, Keep::No)
            } else if f.fluid == Fluid::Water && !f.full_collision {
                let source = f.fluid_level == 8 && !f.fluid_falling;
                (Look::Water, if source { Keep::Notable } else { Keep::No })
            } else if f.fluid == Fluid::Lava {
                let source = f.fluid_level == 8 && !f.fluid_falling;
                (Look::Opaque, if source { Keep::Notable } else { Keep::Common })
            } else if !f.full_collision && !f.can_occlude || f.name.contains("glass") {
                (Look::Clear, if notable(&f.name) { Keep::Notable } else { Keep::No })
            } else {
                (Look::Opaque, if notable(&f.name) { Keep::Notable } else { Keep::Common })
            };
            look.push(l);
            keep.push(k);
        }
        let t = Arc::new(SightTable { look, keep, first });
        self.companions.sight = Some(t.clone());
        t
    }

    /// Its eye this tick: a few rays, what they reach remembered.
    pub(super) fn companion_see(&mut self, b: &mut Body, pos: [f64; 3]) {
        let started = std::time::Instant::now();
        let table = self.companion_sight_table();
        b.senses.enter(b.dim.id());
        let sight = GameSight {
            world: &self.world,
            table: &table,
            min_y: self.dim.min_y(),
            max_y: self.dim.min_y() + self.dim.logical_height(),
        };
        let eye = [pos[0], pos[1] + 1.62, pos[2]];
        b.senses.look(&sight, eye, RAYS, SIGHT);
        let took = started.elapsed();
        let cost = &mut self.companions.sight_cost;
        *cost = (cost.0 + 1, cost.1 + took, cost.2.max(took));
    }

    /// The blocks of `states` it knows of within `radius` of `centre`,
    /// nearest first; None while a search through walls (Sharp senses,
    /// All-knowing) is still under way in `search`, a few sections a tick.
    pub(super) fn companion_known(
        &self,
        b: &Body,
        states: &Arc<Vec<bool>>,
        centre: [i32; 3],
        radius: i32,
        search: &mut Option<BlockSearch>,
        budget: u32,
    ) -> Option<Vec<[i32; 3]>> {
        let wanted = |s: u32| states.get(s as usize).copied().unwrap_or(false);
        let mut scanned = Vec::new();
        if let Some(r) = b.senses.level.xray_radius(radius) {
            let s = search.get_or_insert_with(|| BlockSearch::new(states.clone(), centre, r));
            if !s.step(&self.world, budget) {
                return None;
            }
            scanned = s.found.clone();
        }
        let world = &self.world;
        // What it remembers is checked against what it sees there now only
        // where it can see (the walk refreshes the rest).
        let out = b.senses.known_blocks(wanted, centre, radius, |_| {
            scanned
                .into_iter()
                .filter(|p| states.get(world.block(p[0], p[1], p[2]).0 as usize).copied().unwrap_or(false))
                .collect()
        });
        Some(out)
    }

    /// The nearest structure of `kind` (`village`, `fortress`, `stronghold`,
    /// `ruined_portal`, `bastion_remnant`) it knows of: seen, or at Scout and
    /// All-knowing, located as `/locate` would (once per 256 blocks moved).
    pub(super) fn companion_structure(&mut self, b: &mut Body, kind: &str, pos: [f64; 3]) -> Option<[i32; 3]> {
        let here = pos.map(|v| v.floor() as i32);
        if b.senses.level.knows_structures() {
            let key = format!("located {kind}");
            let stale = b
                .senses
                .memory()
                .places
                .get(&key)
                .is_none_or(|p| (p[0] - here[0]).abs().max((p[2] - here[2]).abs()) > 256);
            if stale {
                b.senses.memory_mut().places.insert(key, here);
                let tag = match kind {
                    "village" => "#minecraft:village",
                    "ruined_portal" => "#minecraft:ruined_portal",
                    "fortress" => "minecraft:fortress",
                    "stronghold" => "minecraft:stronghold",
                    "bastion_remnant" => "minecraft:bastion_remnant",
                    other => other,
                };
                let names = match tag.strip_prefix('#') {
                    Some(t) => self.structure_tag(t),
                    None => vec![tag.to_owned()],
                };
                if let Some((_, at)) = self.end.locator().and_then(|t| t.locate(&names, here, 32)) {
                    b.senses.memory_mut().learn_structure(kind, at, true);
                }
            }
        }
        b.senses.memory().structure(kind, here).map(|s| s.pos)
    }

    /// Its senses from the file saved beside it (a fresh Fair memory when
    /// there is none).
    pub(super) fn companion_load_senses(&mut self, uuid: &str, dim: &str) -> Senses {
        let path = knowledge_path(&self.world_root, uuid);
        let Some(file) = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<KnowledgeFile>(&b).ok())
        else {
            return Senses::new(PerceptionLevel::default(), dim);
        };
        let table = self.companion_sight_table();
        Senses::from_file(&file, dim, |n| table.first.get(n).copied())
    }

    /// Save what it knows beside its file.
    pub(super) fn companion_save_senses(&self, b: &Body) {
        let facts = self.res.facts();
        let file = b.senses.to_file(|s| {
            facts
                .states
                .get(s as usize)
                .map_or_else(String::new, |f| f.name.to_string())
        });
        let path = knowledge_path(&self.world_root, &b.file.uuid);
        let saved = serde_json::to_vec(&file).map_err(std::io::Error::other).and_then(|text| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let partial = path.with_extension("knowledge.tmp");
            std::fs::write(&partial, text)?;
            std::fs::rename(partial, &path)
        });
        if let Err(e) = saved {
            log::warn!("[companion] could not save what {} knows: {e}", b.file.name);
        }
    }
}
