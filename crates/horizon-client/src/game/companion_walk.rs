//! The walker for Movement 2.0 (companion/moves.rs): plans a path that may
//! dig, bridge, pillar, swim and staircase through the world as the
//! companion knows it, then carries it out step by step as a player would:
//! a dig START on what is in the way, a block placed against a face it
//! can click (under its feet while it jumps, beside its floor while it
//! sneaks at an edge), the movement keys toward the next block. A step that
//! fails (a block it can't break, a placement that didn't take, no progress)
//! marks that block and plans again. Long walks go by waypoints 40 blocks
//! apart.
use super::{
    Game,
    companion::Body,
    companion_body::Steer,
    companion_work::{Access, eye, feet},
    lan::LanHost,
};
use horizon_client::{
    blocks::Fluid,
    companion::{
        self as c,
        moves::{self, Cell, Goal, Move, Route, Rules, Terrain},
        perception::PerceptionLevel,
    },
    items::ItemStack,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// Ticks a step may take (digging and placing aside) before it plans again.
const STEP_PATIENCE: u64 = 100;
/// Plans in a row that got nowhere before the walk gives up.
const REPLANS: u32 = 8;
/// A long walk plans this far at a time.
const WAYPOINT: f64 = 40.0;
/// Nodes a plan opens at most.
const BUDGET: u32 = 5000;

/// Blocks it may place to bridge and pillar, cheapest first.
pub(super) const THROWAWAY: [&str; 16] = [
    "minecraft:cobblestone",
    "minecraft:cobbled_deepslate",
    "minecraft:netherrack",
    "minecraft:dirt",
    "minecraft:coarse_dirt",
    "minecraft:andesite",
    "minecraft:diorite",
    "minecraft:granite",
    "minecraft:tuff",
    "minecraft:stone",
    "minecraft:deepslate",
    "minecraft:blackstone",
    "minecraft:basalt",
    "minecraft:end_stone",
    "minecraft:sandstone",
    "minecraft:calcite",
];

/// Blocks never dug on the way.
fn keep_out(name: &str) -> bool {
    matches!(
        name,
        "minecraft:bedrock"
            | "minecraft:spawner"
            | "minecraft:end_portal_frame"
            | "minecraft:nether_portal"
            | "minecraft:end_portal"
            | "minecraft:obsidian"
            | "minecraft:crying_obsidian"
            | "minecraft:crafting_table"
            | "minecraft:furnace"
            | "minecraft:chest"
            | "minecraft:barrel"
    ) || name.ends_with("_bed")
        || name.starts_with("minecraft:infested_")
}

fn hazard(name: &str) -> bool {
    matches!(
        name,
        "minecraft:fire"
            | "minecraft:soul_fire"
            | "minecraft:magma_block"
            | "minecraft:campfire"
            | "minecraft:soul_campfire"
            | "minecraft:cactus"
            | "minecraft:sweet_berry_bush"
            | "minecraft:wither_rose"
            | "minecraft:powder_snow"
            | "minecraft:pointed_dripstone"
    )
}

fn falls(name: &str) -> bool {
    matches!(
        name,
        "minecraft:gravel" | "minecraft:sand" | "minecraft:red_sand" | "minecraft:suspicious_sand" | "minecraft:suspicious_gravel"
    ) || name.ends_with("_concrete_powder")
}

/// What a body's walker keeps between ticks (`Body::walk`).
#[derive(Default)]
pub(super) struct Walk {
    route: Option<Route>,
    goal: Option<Goal>,
    /// Where the current plan goes (the goal, or a waypoint toward it).
    leg: Option<Goal>,
    step: usize,
    since: u64,
    replans: u32,
    /// Blocks it failed to break or stand on: planned as walls.
    bad: Vec<[i32; 3]>,
    /// A pillar step's jump started.
    jumped: bool,
    /// Blocks placed and dug, plans made (the run's numbers).
    pub placed: u32,
    pub dug: u32,
    pub plans: u32,
    pub plan_time: std::time::Duration,
    pub plan_max: std::time::Duration,
}

impl Walk {
    pub fn reset(&mut self) {
        self.route = None;
        self.goal = None;
        self.leg = None;
        self.step = 0;
        self.replans = 0;
        self.jumped = false;
    }
    /// Where it is going and how (the companion list).
    pub fn status(&self) -> serde_json::Value {
        let step = self.route.as_ref().and_then(|r| r.steps.get(self.step));
        serde_json::json!({
            "goal": self.goal.map(|g| g.at),
            "leg": self.leg.map(|g| g.at),
            "steps": self.route.as_ref().map(|r| r.steps.len()),
            "step": self.step,
            "next": step.map(|s| format!("{:?} to {:?} dig {:?} place {:?}", s.how, s.to, s.dig, s.place)),
            "replans": self.replans,
            "bad": self.bad.len(),
            "since": self.since,
        })
    }
    /// The steps left of its plan (for the status line).
    pub fn left(&self) -> usize {
        self.route.as_ref().map_or(0, |r| r.steps.len().saturating_sub(self.step))
    }
}

/// The world as the path search reads it for one body.
struct GameTerrain<'a, 'g> {
    game: &'a Game<'g>,
    /// What it carries (for dig times).
    inventory: &'a [Option<ItemStack>],
    level: PerceptionLevel,
    memory: &'a horizon_client::companion::perception::Memory,
    /// Where its eye is (blocks this near are known at any level: it
    /// touches them).
    near: [i32; 3],
    bad: &'a [[i32; 3]],
    digs: RefCell<HashMap<u32, u16>>,
    min_y: i32,
}

impl GameTerrain<'_, '_> {
    fn dig_ticks(&self, state: u32) -> u16 {
        if let Some(t) = self.digs.borrow().get(&state) {
            return *t;
        }
        let facts = self.game.res.facts();
        let f = facts.get(horizon_world::registry::BlockStateId(state));
        let items = self.game.res.items();
        let ticks = if f.hardness < 0.0 || f.block_entity || keep_out(&f.name) {
            0
        } else if f.hardness == 0.0 {
            1
        } else {
            let mut speed = items.mining_speed(None, &f.name);
            let mut correct = !f.requires_tool;
            for s in self.inventory.iter().flatten() {
                speed = speed.max(items.mining_speed(Some(s), &f.name));
                correct |= items.correct_for_drops(Some(s), &f.name);
            }
            let t = f.hardness * if correct { 30.0 } else { 100.0 } / speed.max(0.1);
            if t > 400.0 { 0 } else { t.ceil().max(1.0) as u16 }
        };
        self.digs.borrow_mut().insert(state, ticks);
        ticks
    }
}

impl Terrain for GameTerrain<'_, '_> {
    fn cell(&self, p: [i32; 3]) -> Cell {
        let world = &self.game.world;
        if p[1] < self.min_y || !world.loaded(p[0], p[2]) || self.bad.contains(&p) {
            return Cell::Solid { dig: 0, falls: false };
        }
        let state = world.block(p[0], p[1], p[2]);
        let facts = self.game.res.facts();
        let f = facts.get(state);
        if f.fluid == Fluid::Lava {
            return Cell::Lava;
        }
        if hazard(&f.name) {
            return Cell::Hazard;
        }
        if f.fluid == Fluid::Water && !f.full_collision {
            return Cell::Water;
        }
        if facts.is_air(state) || !f.full_collision && !f.legacy_solid {
            return Cell::Open;
        }
        let solid = Cell::Solid {
            dig: self.dig_ticks(state.0),
            falls: falls(&f.name),
        };
        // Fair sight: a solid block it never saw and doesn't touch is a
        // block of unknown kind (planned as stone).
        if self.level.xray_radius(1).is_none()
            && (0..3).map(|k| (p[k] - self.near[k]).abs()).max().unwrap_or(0) > 3
            && self.memory.get(p).is_none()
        {
            return Cell::Unknown;
        }
        solid
    }
}

/// Which way a face looks, as `Direction.get3DDataValue`.
fn face_of(d: [i32; 3]) -> u8 {
    match d {
        [0, -1, 0] => 0,
        [0, 1, 0] => 1,
        [0, 0, -1] => 2,
        [0, 0, 1] => 3,
        [-1, 0, 0] => 4,
        _ => 5,
    }
}

impl Game<'_> {
    /// Throwaway blocks it carries (to bridge and pillar).
    pub(super) fn companion_blocks(inventory: &[Option<ItemStack>]) -> u32 {
        inventory
            .iter()
            .flatten()
            .filter(|s| THROWAWAY.contains(&&*s.item))
            .map(|s| s.count)
            .sum()
    }

    /// Ticks to dig stone with what it carries (an unknown block's cost).
    fn stone_ticks(&self, inventory: &[Option<ItemStack>]) -> u16 {
        let items = self.res.items();
        let mut speed = 1.0f32;
        let mut correct = false;
        for s in inventory.iter().flatten() {
            speed = speed.max(items.mining_speed(Some(s), "minecraft:stone"));
            correct |= items.correct_for_drops(Some(s), "minecraft:stone");
        }
        (1.5 * if correct { 30.0 } else { 100.0 } / speed).ceil() as u16
    }

    /// Plan a path for `b` from `from` to `goal` with Movement 2.0.
    pub(super) fn companion_plan_walk(&mut self, host: &LanHost, b: &mut Body, from: [i32; 3], goal: Goal) -> Route {
        let Some(g) = host.hub.guest(b.conn) else {
            return Route::default();
        };
        let inventory = g.inventory.clone();
        let rules = Rules {
            blocks: Self::companion_blocks(&inventory),
            stone: self.stone_ticks(&inventory),
            budget: BUDGET,
            min_y: self.dim.min_y() + 1,
            ..Default::default()
        };
        let started = std::time::Instant::now();
        let route = {
            let t = GameTerrain {
                game: self,
                inventory: &inventory,
                level: b.senses.level,
                memory: b.senses.memory(),
                near: from,
                bad: &b.walk.bad,
                digs: RefCell::new(HashMap::new()),
                min_y: self.dim.min_y(),
            };
            moves::find(&t, from, goal, &rules)
        };
        let took = started.elapsed();
        b.walk.plans += 1;
        b.walk.plan_time += took;
        b.walk.plan_max = b.walk.plan_max.max(took);
        route
    }

    /// Go to `goal` with Movement 2.0: Arrived, Moving, or Stuck once
    /// several plans in a row got nowhere.
    pub(super) fn companion_travel(&mut self, host: &mut LanHost, b: &mut Body, goal: Goal, pos: [f64; 3]) -> Steer {
        let f = feet(pos);
        if goal.reached(f) && self.companion_settled(host, b) {
            self.companion_stop_dig(host, b);
            b.walk.reset();
            return Steer::Arrived;
        }
        if b.walk.goal != Some(goal) {
            b.walk.reset();
            b.walk.goal = Some(goal);
        }
        let need_plan = b
            .walk
            .route
            .as_ref()
            .is_none_or(|r| b.walk.step >= r.steps.len());
        if need_plan {
            // The leg it planned is done: there, or as near as it got.
            if b.walk.route.is_some() {
                let leg_done = b.walk.leg.is_some_and(|l| l.reached(f));
                if leg_done {
                    b.walk.replans = 0;
                } else {
                    b.walk.replans += 1;
                }
            }
            if b.walk.replans > REPLANS {
                b.walk.reset();
                return Steer::Stuck;
            }
            if !self.companion_settled(host, b) {
                return Steer::Moving;
            }
            let leg = leg_toward(f, goal);
            let route = self.companion_plan_walk(host, b, f, leg);
            if route.steps.is_empty() {
                b.walk.replans += 1;
                b.walk.route = None;
                if !route.reached && b.walk.replans > 2 {
                    b.walk.reset();
                    return Steer::Stuck;
                }
                // Somewhere in the way it can't see past: try a nearer leg.
                if route.reached {
                    return Steer::Arrived;
                }
                return Steer::Moving;
            }
            b.walk.route = Some(route);
            b.walk.leg = Some(leg);
            b.walk.step = 0;
            b.walk.since = b.ticks;
            b.walk.jumped = false;
        }
        self.companion_walk_step(host, b, pos)
    }

    /// Whether it stands somewhere (on the ground, in water, or still).
    fn companion_settled(&self, host: &LanHost, b: &Body) -> bool {
        host.hub.guest(b.conn).is_some_and(|g| {
            g.on_ground || {
                let p = feet(g.position);
                self.res.facts().get(self.world.block(p[0], p[1], p[2])).fluid != Fluid::None
            }
        })
    }

    /// Carry out the current step of its plan.
    fn companion_walk_step(&mut self, host: &mut LanHost, b: &mut Body, pos: [f64; 3]) -> Steer {
        let Some(route) = b.walk.route.as_ref() else {
            return Steer::Moving;
        };
        let Some(step) = route.steps.get(b.walk.step).cloned() else {
            return Steer::Moving;
        };
        let next = route.steps.get(b.walk.step + 1).cloned();
        let f = feet(pos);
        let Some(g) = host.hub.guest(b.conn) else {
            return Steer::Stuck;
        };
        let on_ground = g.on_ground;
        let in_water = self.res.facts().get(self.world.block(f[0], f[1], f[2])).fluid == Fluid::Water;
        let food = g.food;
        // There: on to the next step.
        let to_c = [f64::from(step.to[0]) + 0.5, f64::from(step.to[1]), f64::from(step.to[2]) + 0.5];
        let h = c::horizontal(pos, to_c);
        let chain = step.how == Move::Walk
            && next
                .as_ref()
                .is_some_and(|n| n.how == Move::Walk && n.dig.is_empty() && n.place.is_none());
        let close = if chain { 0.6 } else { 0.35 };
        if f == step.to && h < close && (on_ground || in_water || chain) {
            self.companion_stop_dig(host, b);
            b.walk.step += 1;
            b.walk.since = b.ticks;
            b.walk.jumped = false;
            b.walk.replans = 0;
            return Steer::Moving;
        }
        // Too long on one step: that block is bad, plan again.
        let patience = STEP_PATIENCE + 400 * step.dig.len() as u64;
        if b.ticks - b.walk.since > patience {
            log::debug!("[companion] {} step {:?} took too long at {f:?}", b.file.name, step);
            self.companion_stop_dig(host, b);
            b.walk.bad.push(step.to);
            if b.walk.bad.len() > 64 {
                b.walk.bad.remove(0);
            }
            b.walk.route = None;
            return Steer::Moving;
        }
        // What is in the way goes first.
        if let Some(d) = step.dig.iter().copied().find(|d| !self.passable(*d)) {
            let face = match self.companion_access(eye(pos), d) {
                Access::Direct(face) => face,
                _ => face_of([
                    (f[0] - d[0]).signum(),
                    i32::from(d[1] < f[1]),
                    (f[2] - d[2]).signum(),
                ]),
            };
            // While it digs it doesn't wander off.
            let was = b.work.dig.map(|(p, _)| p);
            match self.companion_dig_block(host, b, d, face) {
                Some(true) => {
                    if was != Some(d) {
                        b.walk.dug += 1;
                        b.walk.since = b.ticks;
                    }
                }
                Some(false) => {}
                None => {
                    // Can't dig it (lava beside it, too hard): a wall now.
                    self.companion_stop_dig(host, b);
                    b.walk.bad.push(d);
                    b.walk.route = None;
                }
            }
            return Steer::Moving;
        }
        if b.work.dig.is_some() {
            self.companion_stop_dig(host, b);
        }
        // A block to place.
        if let Some(q) = step.place
            && !self.floor(q)
        {
            let Some(slot) = host.hub.guest(b.conn).and_then(|g| {
                g.inventory
                    .iter()
                    .position(|s| s.as_ref().is_some_and(|s| THROWAWAY.contains(&&*s.item)))
            }) else {
                // Out of blocks: plan again without bridging.
                b.walk.route = None;
                return Steer::Moving;
            };
            b.walk.since = b.ticks;
            if step.how == Move::Pillar {
                // Jump; at the top of the jump, a block under its feet.
                b.pitch = 90.0;
                if on_ground && !b.walk.jumped {
                    b.keys.jump = true;
                    b.walk.jumped = true;
                    return Steer::Moving;
                }
                if pos[1] >= f64::from(q[1]) + 1.0 {
                    self.companion_hold_slot(host, b, slot);
                    self.companion_use_block(host, b, [q[0], q[1] - 1, q[2]], 1, pos);
                    if self.floor(q) {
                        b.walk.placed += 1;
                    }
                } else if on_ground && b.walk.jumped {
                    // Landed without placing: jump again.
                    b.walk.jumped = false;
                }
                return Steer::Moving;
            }
            // A bridge (or a step's floor): against a solid neighbour of `q`,
            // the floor under it first.
            let floor = [f[0], f[1] - 1, f[2]];
            let neighbours = [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1], [0, -1, 0]];
            let against = std::iter::once(floor)
                .filter(|n| neighbours.iter().any(|d| [n[0] + d[0], n[1] + d[1], n[2] + d[2]] == q))
                .chain(neighbours.iter().map(|d| [q[0] + d[0], q[1] + d[1], q[2] + d[2]]))
                .find(|n| self.floor(*n) && super::companion_work::to_block(eye(pos), *n) < 4.4);
            let Some(n) = against else {
                b.walk.bad.push(q);
                b.walk.route = None;
                return Steer::Moving;
            };
            let face = face_of([q[0] - n[0], q[1] - n[1], q[2] - n[2]]);
            b.keys.shift = true;
            self.companion_hold_slot(host, b, slot);
            self.companion_use_block(host, b, n, face, pos);
            if self.floor(q) {
                b.walk.placed += 1;
            }
            return Steer::Moving;
        }
        // Move toward the step's block.
        let aim = if chain {
            // Straight walks: aim past this block at the next one.
            next.map_or(to_c, |n| [f64::from(n.to[0]) + 0.5, f64::from(n.to[1]), f64::from(n.to[2]) + 0.5])
        } else {
            to_c
        };
        b.yaw = c::yaw_toward(pos, aim) as f32;
        b.pitch = 0.0;
        b.keys.forward = h > 0.1;
        b.keys.sprint = chain && food > 6 && !in_water;
        match step.how {
            Move::Up => b.keys.jump = on_ground || in_water,
            Move::Swim | Move::Walk if in_water => {
                b.keys.jump = step.to[1] >= f[1];
                b.keys.shift = step.to[1] < f[1];
            }
            // Over the hole or the edge: in it, it drops.
            Move::Fall | Move::Down | Move::DigDown if h < 0.25 => b.keys.forward = false,
            _ => {}
        }
        // Edges it isn't meant to go off: sneaking keeps it on.
        if matches!(step.how, Move::Walk) && !in_water {
            let below = [step.to[0], step.to[1] - 1, step.to[2]];
            b.keys.shift |= !self.floor(below);
        }
        Steer::Moving
    }

    /// Dig block `p` on its way: Some(true) while digging (or it just
    /// broke), Some(false) while it waits, None when it can't.
    fn companion_dig_block(&mut self, host: &mut LanHost, b: &mut Body, p: [i32; 3], face: u8) -> Option<bool> {
        let facts = self.res.facts();
        let f = facts.get(self.world.block(p[0], p[1], p[2]));
        if keep_out(&f.name) || f.hardness < 0.0 {
            return None;
        }
        match self.companion_dig_step(host, b, p, face) {
            Ok(()) => Some(true),
            Err(()) => None,
        }
    }
}

/// The goal, or a waypoint 40 blocks toward it in its column.
fn leg_toward(f: [i32; 3], goal: Goal) -> Goal {
    let (dx, dz) = (f64::from(goal.at[0] - f[0]), f64::from(goal.at[2] - f[2]));
    let d = (dx * dx + dz * dz).sqrt();
    if d <= WAYPOINT + goal.near {
        return goal;
    }
    let k = WAYPOINT / d;
    Goal::column(
        [
            f[0] + (dx * k).round() as i32,
            goal.at[1],
            f[2] + (dz * k).round() as i32,
        ],
        3.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_walks_go_by_waypoints() {
        let g = Goal::exact([200, 70, 0]);
        let leg = leg_toward([0, 64, 0], g);
        assert_eq!(leg.at, [40, 70, 0]);
        assert!(leg.reached([40, 90, 1]));
        assert_eq!(leg_toward([180, 64, 0], g), g);
    }
}
