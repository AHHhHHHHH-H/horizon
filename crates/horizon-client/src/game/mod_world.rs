//! Immutable, bounded views of chunks already resident in this client.
use super::Game;
use horizon_client::{
    biome::Biomes,
    blocks::Fluid,
    chunk::{Chunk, HEIGHT, MIN_Y},
};
use horizon_mods::{BlockSample, ClientWorldView};
use horizon_world::registry::BlockRegistry;
use std::{collections::HashMap, sync::Arc};

pub(super) struct Tables {
    registry: &'static BlockRegistry,
    fluids: Box<[&'static str]>,
}
struct View {
    tables: Arc<Tables>,
    biomes: Arc<Biomes>,
    chunks: HashMap<(i32, i32), Arc<Chunk>>,
}
impl std::fmt::Debug for View {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientWorldView")
            .field("chunks", &self.chunks.len())
            .finish()
    }
}
impl ClientWorldView for View {
    fn sample(&self, x: i32, y: i32, z: i32) -> Option<BlockSample> {
        if !(MIN_Y..MIN_Y + HEIGHT).contains(&y) {
            return None;
        }
        let chunk = self.chunks.get(&(x >> 4, z >> 4))?;
        let id = chunk.block(x & 15, y, z & 15);
        let state = self.tables.registry.state(id)?;
        let properties = state
            .properties
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        let state = if properties.is_empty() {
            state.name.clone()
        } else {
            format!("{}[{properties}]", state.name)
        };
        let light = chunk.light(x & 15, y, z & 15);
        Some(BlockSample {
            state,
            biome: self.biomes.get(chunk.biome(x, y, z)).name.clone(),
            fluid: self
                .tables
                .fluids
                .get(id.0 as usize)
                .copied()
                .unwrap_or("minecraft:empty")
                .into(),
            block_light: light & 15,
            sky_light: light >> 4,
        })
    }
}
impl Game<'_> {
    pub(super) fn mod_world_snapshot(&mut self) -> Option<Arc<dyn ClientWorldView>> {
        if !self.mods.mods.wants_client_world() || self.title.is_some() {
            return None;
        }
        let p = self.player.as_ref()?.position;
        let (cx, cz) = ((p.x.floor() as i32) >> 4, (p.z.floor() as i32) >> 4);
        let registry = self.pack.assets.registry;
        if self
            .mods
            .world_tables
            .as_ref()
            .is_none_or(|t| !std::ptr::eq(t.registry, registry))
        {
            self.mods.world_tables = Some(Arc::new(Tables {
                registry,
                fluids: self
                    .res
                    .facts()
                    .states
                    .iter()
                    .map(|s| match s.fluid {
                        Fluid::None => "minecraft:empty",
                        Fluid::Water => "minecraft:water",
                        Fluid::Lava => "minecraft:lava",
                    })
                    .collect(),
            }));
        }
        let mut chunks = HashMap::new();
        for x in cx.saturating_sub(2)..=cx.saturating_add(2) {
            for z in cz.saturating_sub(2)..=cz.saturating_add(2) {
                if let Some(chunk) = self.world.chunks.get(&(x, z)) {
                    chunks.insert((x, z), chunk.clone());
                }
            }
        }
        Some(Arc::new(View {
            tables: self.mods.world_tables.as_ref()?.clone(),
            biomes: self.res.biomes().clone(),
            chunks,
        }))
    }
}

impl Game<'_> {
    pub(super) fn mod_nearby_entities(&self) -> Arc<[horizon_mods::NearbyEntity]> {
        use horizon_mods::NearbyEntity;
        if !self.mods.mods.wants_entities() || self.title.is_some() {
            return Arc::from([]);
        }
        let Some(player) = self.player.as_ref() else {
            return Arc::from([]);
        };
        let mut nearby = Vec::new();
        let mut add = |id,
                       kind: &str,
                       pos: horizon_core::geometry::Vec3,
                       velocity: horizon_core::geometry::Vec3,
                       width,
                       height,
                       on_ground,
                       burning| {
            let d2 = (pos.x - player.position.x).powi(2)
                + (pos.y - player.position.y).powi(2)
                + (pos.z - player.position.z).powi(2);
            if d2.is_finite() && d2 <= 32.0_f64.powi(2) {
                nearby.push((
                    d2,
                    NearbyEntity {
                        id,
                        kind: kind.into(),
                        x: pos.x,
                        y: pos.y,
                        z: pos.z,
                        velocity_x: velocity.x,
                        velocity_y: velocity.y,
                        velocity_z: velocity.z,
                        width,
                        height,
                        on_ground,
                        burning,
                    },
                ));
            }
        };
        for m in &self.mobs {
            if !m.alive() {
                continue;
            }
            let b = m.bounds();
            add(
                m.id,
                &m.kind.id,
                m.position,
                m.velocity,
                (b.max_position().x - b.min_position().x) as f32,
                (b.max_position().y - b.min_position().y) as f32,
                m.on_ground,
                m.fire_ticks > 0,
            );
        }
        for i in &self.dropped {
            add(
                i.id,
                "minecraft:item",
                i.position,
                i.velocity,
                0.25,
                0.25,
                i.on_ground,
                i.fire_ticks > 0,
            );
        }
        for o in &self.orbs {
            add(
                o.id,
                "minecraft:experience_orb",
                o.position,
                o.velocity,
                0.5,
                0.5,
                o.on_ground,
                false,
            );
        }
        for b in &self.boats {
            add(
                b.id,
                if b.chest {
                    "minecraft:chest_boat"
                } else {
                    "minecraft:boat"
                },
                b.position,
                b.velocity,
                1.375,
                0.5625,
                b.on_ground,
                false,
            );
        }
        for c in &self.carts {
            add(
                c.id,
                c.kind.id(),
                c.position,
                c.velocity,
                0.98,
                0.7,
                c.on_ground,
                false,
            );
        }
        nearby.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
        nearby.truncate(128);
        nearby.into_iter().map(|(_, e)| e).collect()
    }
}

#[derive(Clone, Copy)]
struct Actor {
    ground: bool,
    water: bool,
    lava: bool,
    velocity: [f64; 3],
}
#[derive(Default)]
pub(super) struct Observer {
    dimension: String,
    actors: HashMap<Option<u32>, Actor>,
    container: bool,
    last_interaction: Option<[i32; 3]>,
}
impl Game<'_> {
    pub(super) fn mod_observed_event(
        &mut self,
        kind: horizon_mods::WorldEventKind,
        entity: Option<u32>,
        at: [f64; 3],
        detail: String,
        speed: f32,
    ) {
        if !self.mods.mods.wants_world_events() || self.title.is_some() {
            return;
        }
        self.mods
            .mods
            .observed_world_event(horizon_mods::WorldEvent {
                kind,
                entity,
                x: at[0],
                y: at[1],
                z: at[2],
                detail,
                speed,
                tick: self.ticks,
            });
    }
    pub(super) fn mod_block_interaction(&mut self, pos: [i32; 3]) {
        if !self.mods.mods.wants_world_events() {
            return;
        }
        self.mods.world_observer.last_interaction = Some(pos);
        let state = self.world.block(pos[0], pos[1], pos[2]);
        let name = self.res.facts().get(state).name.to_string();
        self.mod_observed_event(
            horizon_mods::WorldEventKind::BlockInteracted,
            None,
            pos.map(|v| f64::from(v) + 0.5),
            name,
            0.0,
        );
    }
    pub(super) fn mod_block_change(&mut self, pos: [i32; 3]) {
        if !self.mods.mods.wants_world_events() {
            return;
        }
        let state = self.world.block(pos[0], pos[1], pos[2]);
        let name = self.res.facts().get(state).name.to_string();
        self.mod_observed_event(
            horizon_mods::WorldEventKind::BlockChanged,
            None,
            pos.map(|v| f64::from(v) + 0.5),
            name,
            0.0,
        );
    }
    pub(super) fn observe_mod_world_events(&mut self) {
        use horizon_mods::WorldEventKind as K;
        if !self.mods.mods.wants_world_events() || self.title.is_some() {
            self.mods.world_observer = Observer::default();
            return;
        }
        let Some(player) = self.player.as_ref() else {
            return;
        };
        let position = player.position;
        let mut actors = Vec::new();
        actors.push((
            None,
            position.components(),
            "minecraft:player".to_owned(),
            Actor {
                ground: player.ground.on_ground,
                water: player.fluids.was_touching_water,
                lava: player.is_in_lava(),
                velocity: player.velocity.components(),
            },
        ));
        for m in self
            .mobs
            .iter()
            .filter(|m| {
                (m.position.x - position.x).powi(2)
                    + (m.position.y - position.y).powi(2)
                    + (m.position.z - position.z).powi(2)
                    <= 32.0_f64.powi(2)
            })
            .take(128)
        {
            actors.push((
                Some(m.id),
                m.position.components(),
                m.kind.id.clone(),
                Actor {
                    ground: m.on_ground,
                    water: m.fluids.in_water,
                    lava: m.fluids.in_lava,
                    velocity: m.velocity.components(),
                },
            ));
        }
        let dimension = self.dim.id();
        let observer = &mut self.mods.world_observer;
        if observer.dimension != dimension {
            *observer = Observer {
                dimension: dimension.into(),
                ..Observer::default()
            };
        }
        let mut events = Vec::new();
        let mut now = HashMap::new();
        for (id, at, detail, actor) in actors {
            if let Some(old) = observer.actors.get(&id) {
                let speed = old.velocity.iter().map(|v| v * v).sum::<f64>().sqrt() as f32;
                for (changed, kind) in [
                    (!old.ground && actor.ground, K::Landed),
                    (!old.water && actor.water, K::EnteredWater),
                    (!old.lava && actor.lava, K::EnteredLava),
                ] {
                    if changed {
                        events.push((kind, id, at, detail.clone(), speed));
                    }
                }
            }
            now.insert(id, actor);
        }
        observer.actors = now;
        let container = self.screen == super::Screen::Inventory
            && (self.container.is_some() || self.station.is_some());
        if container && !observer.container {
            let block = self
                .container
                .map(|(x, y, z)| [x, y, z])
                .or(observer.last_interaction);
            let at = block.map_or(position.components(), |p| p.map(|v| f64::from(v) + 0.5));
            let detail = block
                .map(|p| {
                    self.res
                        .facts()
                        .get(self.world.block(p[0], p[1], p[2]))
                        .name
                        .to_string()
                })
                .unwrap_or_else(|| "container".into());
            events.push((K::ContainerOpened, None, at, detail, 0.0));
        }
        observer.container = container;
        for (kind, id, at, detail, speed) in events {
            self.mod_observed_event(kind, id, at, detail, speed);
        }
    }
}
