//! Vanilla ground pathfinding: `PathType`, `Node`, `BinaryHeap`, `PathFinder`
//! (A* with the 1.5 heuristic fudge) and `WalkNodeEvaluator` (node types,
//! penalties, jumping onto blocks, dropping down, doors and fences).
//!
//! The evaluator reads the world through [`PathLevel`]; the game binds it to
//! resident chunks (see [`super::level`]) and tests bind it to synthetic grids.
use horizon_core::geometry::{Aabb, Vec3};
use std::collections::HashMap;

/// `net.minecraft.world.level.pathfinder.PathType` with its default malus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum PathType {
    Blocked,
    Open,
    Walkable,
    WalkableDoor,
    Trapdoor,
    PowderSnow,
    OnTopOfPowderSnow,
    Fence,
    Lava,
    Water,
    WaterBorder,
    Rail,
    UnpassableRail,
    FireInNeighbor,
    Fire,
    DamagingInNeighbor,
    Damaging,
    DoorOpen,
    DoorWoodClosed,
    DoorIronClosed,
    Breach,
    Leaves,
    StickyHoney,
    Cocoa,
    DamageCautious,
    OnTopOfTrapdoor,
    BigMobsCloseToDanger,
}
pub const PATH_TYPES: usize = 27;
impl PathType {
    pub const ALL: [PathType; PATH_TYPES] = [
        PathType::Blocked,
        PathType::Open,
        PathType::Walkable,
        PathType::WalkableDoor,
        PathType::Trapdoor,
        PathType::PowderSnow,
        PathType::OnTopOfPowderSnow,
        PathType::Fence,
        PathType::Lava,
        PathType::Water,
        PathType::WaterBorder,
        PathType::Rail,
        PathType::UnpassableRail,
        PathType::FireInNeighbor,
        PathType::Fire,
        PathType::DamagingInNeighbor,
        PathType::Damaging,
        PathType::DoorOpen,
        PathType::DoorWoodClosed,
        PathType::DoorIronClosed,
        PathType::Breach,
        PathType::Leaves,
        PathType::StickyHoney,
        PathType::Cocoa,
        PathType::DamageCautious,
        PathType::OnTopOfTrapdoor,
        PathType::BigMobsCloseToDanger,
    ];
    pub fn default_malus(self) -> f32 {
        use PathType::*;
        match self {
            Blocked | PowderSnow | Fence | Lava | UnpassableRail | Damaging | DoorWoodClosed | DoorIronClosed | Leaves => -1.0,
            Water | WaterBorder | FireInNeighbor | DamagingInNeighbor | StickyHoney => 8.0,
            Fire => 16.0,
            Breach | BigMobsCloseToDanger => 4.0,
            _ => 0.0,
        }
    }
}
/// `Mob.pathfindingMalus` overrides on top of the type defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Malus(pub [f32; PATH_TYPES]);
impl Default for Malus {
    fn default() -> Self {
        Self(PathType::ALL.map(PathType::default_malus))
    }
}
impl Malus {
    pub fn get(&self, t: PathType) -> f32 {
        self.0[t as usize]
    }
    pub fn set(&mut self, t: PathType, v: f32) {
        self.0[t as usize] = v;
    }
}

/// World queries used by the walk evaluator and navigation.
pub trait PathLevel {
    /// `WalkNodeEvaluator.getPathTypeFromState` at a position.
    fn path_type_from_state(&self, x: i32, y: i32, z: i32) -> PathType;
    /// Collision shape top of the block at the position (`shape.max(Y)`),
    /// `None` when the collision shape is empty.
    fn collision_top(&self, x: i32, y: i32, z: i32) -> Option<f64>;
    /// `!level.noCollision(mob, aabb)` against block collision shapes.
    fn has_collision(&self, aabb: Aabb) -> bool;
    fn is_air(&self, x: i32, y: i32, z: i32) -> bool;
    /// `BlockState.isPathfindable(LAND)`.
    fn is_pathfindable_land(&self, x: i32, y: i32, z: i32) -> bool;
    /// Fluid state in `#entity_floatable` (water or lava).
    fn is_floatable_fluid(&self, x: i32, y: i32, z: i32) -> bool;
    fn is_water(&self, x: i32, y: i32, z: i32) -> bool;
    /// Legacy `BlockState.isSolid()`.
    fn is_solid(&self, x: i32, y: i32, z: i32) -> bool;
    /// `BlockState.isSolidRender()`.
    fn is_solid_render(&self, x: i32, y: i32, z: i32) -> bool;
    /// `#doors` or `#fences` (MoveControl does not jump onto them).
    fn is_door_or_fence(&self, x: i32, y: i32, z: i32) -> bool;
    fn is_cauldron(&self, x: i32, y: i32, z: i32) -> bool;
    /// `Level.canSeeSky`.
    fn can_see_sky(&self, x: i32, y: i32, z: i32) -> bool;
    fn min_y(&self) -> i32;
    fn max_y(&self) -> i32;
    fn loaded(&self, x: i32, z: i32) -> bool;
}

/// The mob-dependent inputs of a path search.
#[derive(Clone, Debug)]
pub struct PathMob {
    pub position: [f64; 3],
    pub width: f32,
    pub height: f32,
    pub on_ground: bool,
    pub in_floatable_fluid: bool,
    pub max_up_step: f32,
    pub max_fall_distance: i32,
    pub malus: Malus,
    pub can_float: bool,
    pub can_open_doors: bool,
    pub can_pass_doors: bool,
    pub can_walk_over_fences: bool,
}
impl PathMob {
    pub fn block_position(&self) -> [i32; 3] {
        [self.position[0].floor() as i32, self.position[1].floor() as i32, self.position[2].floor() as i32]
    }
    pub fn bounds(&self) -> Aabb {
        let w = f64::from(self.width) / 2.0;
        Aabb::new(
            Vec3::new(self.position[0] - w, self.position[1], self.position[2] - w),
            Vec3::new(self.position[0] + w, self.position[1] + f64::from(self.height), self.position[2] + w),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub heap_idx: i32,
    pub g: f32,
    pub h: f32,
    pub f: f32,
    pub came_from: Option<usize>,
    pub closed: bool,
    pub walked_distance: f32,
    pub cost_malus: f32,
    pub kind: PathType,
}
impl Node {
    fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z, heap_idx: -1, g: 0.0, h: 0.0, f: 0.0, came_from: None, closed: false, walked_distance: 0.0, cost_malus: 0.0, kind: PathType::Blocked }
    }
    /// `Node.createHash` (vanilla's lossy key; colliding positions share a node).
    pub fn hash(x: i32, y: i32, z: i32) -> i32 {
        (y & 0xFF) | ((x & 32767) << 8) | ((z & 32767) << 24) | if x < 0 { i32::MIN } else { 0 } | if z < 0 { 32768 } else { 0 }
    }
    pub fn distance_to(&self, x: i32, y: i32, z: i32) -> f32 {
        let (dx, dy, dz) = ((x - self.x) as f32, (y - self.y) as f32, (z - self.z) as f32);
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
    pub fn distance_manhattan(&self, x: i32, y: i32, z: i32) -> f32 {
        ((x - self.x).abs() as f32) + ((y - self.y).abs() as f32) + ((z - self.z).abs() as f32)
    }
    pub fn pos(&self) -> [i32; 3] {
        [self.x, self.y, self.z]
    }
}

/// A computed path (`net.minecraft.world.level.pathfinder.Path`).
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub nodes: Vec<Node>,
    pub next: usize,
    pub target: [i32; 3],
    pub dist_to_target: f32,
    pub reached: bool,
}
impl Path {
    pub fn new(nodes: Vec<Node>, target: [i32; 3], reached: bool) -> Self {
        let dist_to_target = nodes.last().map_or(f32::MAX, |n| n.distance_manhattan(target[0], target[1], target[2]));
        Self { nodes, next: 0, target, dist_to_target, reached }
    }
    pub fn is_done(&self) -> bool {
        self.next >= self.nodes.len()
    }
    pub fn advance(&mut self) {
        self.next += 1;
    }
    pub fn end_node(&self) -> Option<&Node> {
        self.nodes.last()
    }
    pub fn next_node(&self) -> &Node {
        &self.nodes[self.next]
    }
    /// `getEntityPosAtNode`: node corner plus half the (block-rounded) width.
    pub fn entity_pos_at(&self, width: f32, index: usize) -> [f64; 3] {
        let n = &self.nodes[index];
        let half = f64::from((width + 1.0) as i32) * 0.5;
        [f64::from(n.x) + half, f64::from(n.y), f64::from(n.z) + half]
    }
    pub fn next_entity_pos(&self, width: f32) -> [f64; 3] {
        self.entity_pos_at(width, self.next)
    }
    /// `sameAs`: equal node coordinates (Node.equals compares positions).
    pub fn same_as(&self, other: Option<&Path>) -> bool {
        other.is_some_and(|o| o.nodes.len() == self.nodes.len() && o.nodes.iter().zip(&self.nodes).all(|(a, b)| a.pos() == b.pos()))
    }
    pub fn truncate(&mut self, index: usize) {
        self.nodes.truncate(index);
    }
}

/// `BinaryHeap` over arena indices, with vanilla's float comparisons.
#[derive(Default)]
struct Heap {
    heap: Vec<usize>,
}
impl Heap {
    fn insert(&mut self, nodes: &mut [Node], n: usize) {
        self.heap.push(n);
        nodes[n].heap_idx = (self.heap.len() - 1) as i32;
        self.up(nodes, self.heap.len() - 1);
    }
    fn pop(&mut self, nodes: &mut [Node]) -> usize {
        let popped = self.heap[0];
        let last = self.heap.pop().unwrap_or(popped);
        if !self.heap.is_empty() {
            self.heap[0] = last;
            self.down(nodes, 0);
        }
        nodes[popped].heap_idx = -1;
        popped
    }
    fn change_cost(&mut self, nodes: &mut [Node], n: usize, cost: f32) {
        let old = nodes[n].f;
        nodes[n].f = cost;
        let idx = nodes[n].heap_idx as usize;
        if cost < old {
            self.up(nodes, idx);
        } else {
            self.down(nodes, idx);
        }
    }
    fn up(&mut self, nodes: &mut [Node], mut idx: usize) {
        let node = self.heap[idx];
        let cost = nodes[node].f;
        while idx > 0 {
            let parent_idx = (idx - 1) >> 1;
            let parent = self.heap[parent_idx];
            if !(cost < nodes[parent].f) {
                break;
            }
            self.heap[idx] = parent;
            nodes[parent].heap_idx = idx as i32;
            idx = parent_idx;
        }
        self.heap[idx] = node;
        nodes[node].heap_idx = idx as i32;
    }
    fn down(&mut self, nodes: &mut [Node], mut idx: usize) {
        let node = self.heap[idx];
        let cost = nodes[node].f;
        let size = self.heap.len();
        loop {
            let left = 1 + (idx << 1);
            let right = left + 1;
            if left >= size {
                break;
            }
            let left_node = self.heap[left];
            let left_cost = nodes[left_node].f;
            let (right_node, right_cost) = if right >= size { (usize::MAX, f32::INFINITY) } else { (self.heap[right], nodes[self.heap[right]].f) };
            if left_cost < right_cost {
                if !(left_cost < cost) {
                    break;
                }
                self.heap[idx] = left_node;
                nodes[left_node].heap_idx = idx as i32;
                idx = left;
            } else {
                if !(right_cost < cost) {
                    break;
                }
                self.heap[idx] = right_node;
                nodes[right_node].heap_idx = idx as i32;
                idx = right;
            }
        }
        self.heap[idx] = node;
        nodes[node].heap_idx = idx as i32;
    }
}

const HORIZONTAL: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)]; // N, E, S, W
fn data2d(dir: usize) -> usize {
    // Direction.get2DDataValue: SOUTH 0, WEST 1, NORTH 2, EAST 3.
    [2, 3, 0, 1][dir]
}
fn clockwise(dir: usize) -> usize {
    (dir + 1) % 4
}

/// `WalkNodeEvaluator` state for one search (`prepare` .. `done`).
pub struct WalkEvaluator<'a, L: PathLevel> {
    level: &'a L,
    mob: &'a PathMob,
    nodes: Vec<Node>,
    index: HashMap<i32, usize>,
    type_cache: HashMap<(i32, i32, i32), PathType>,
    collision_cache: HashMap<[u64; 6], bool>,
    entity_width: i32,
    entity_height: i32,
    entity_depth: i32,
    neighbors_scratch: [Option<usize>; 4],
}
impl<'a, L: PathLevel> WalkEvaluator<'a, L> {
    pub fn new(level: &'a L, mob: &'a PathMob) -> Self {
        Self {
            level,
            mob,
            nodes: Vec::new(),
            index: HashMap::new(),
            type_cache: HashMap::new(),
            collision_cache: HashMap::new(),
            entity_width: (mob.width + 1.0).floor() as i32,
            entity_height: (mob.height + 1.0).floor() as i32,
            entity_depth: (mob.width + 1.0).floor() as i32,
            neighbors_scratch: [None; 4],
        }
    }
    fn malus(&self, t: PathType) -> f32 {
        self.mob.malus.get(t)
    }
    pub fn node(&mut self, x: i32, y: i32, z: i32) -> usize {
        let key = Node::hash(x, y, z);
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        self.nodes.push(Node::new(x, y, z));
        let i = self.nodes.len() - 1;
        self.index.insert(key, i);
        i
    }
    /// `getStart`.
    pub fn start(&mut self) -> usize {
        let l = self.level;
        let m = self.mob;
        let [bx, _, bz] = m.block_position();
        let mut start_y = m.position[1].floor() as i32;
        let fx = m.position[0].floor() as i32;
        let fz = m.position[2].floor() as i32;
        if m.can_float && m.in_floatable_fluid {
            while l.is_floatable_fluid(fx, start_y, fz) {
                start_y += 1;
            }
            start_y -= 1;
        } else if m.on_ground {
            start_y = (m.position[1] + 0.5).floor() as i32;
        } else {
            let mut y = (m.position[1] + 1.0).floor() as i32;
            while y > l.min_y() {
                start_y = y;
                y -= 1;
                if !l.is_air(fx, y, fz) && !l.is_pathfindable_land(fx, y, fz) {
                    break;
                }
            }
        }
        if !self.can_start_at(bx, start_y, bz) {
            let b = m.bounds();
            let (lo, hi) = (b.min_position(), b.max_position());
            for (x, z) in [(lo.x, lo.z), (lo.x, hi.z), (hi.x, lo.z), (hi.x, hi.z)] {
                let (x, z) = (x.floor() as i32, z.floor() as i32);
                if self.can_start_at(x, start_y, z) {
                    return self.start_node(x, start_y, z);
                }
            }
        }
        self.start_node(bx, start_y, bz)
    }
    fn start_node(&mut self, x: i32, y: i32, z: i32) -> usize {
        let n = self.node(x, y, z);
        let (nx, ny, nz) = (self.nodes[n].x, self.nodes[n].y, self.nodes[n].z);
        let t = self.cached_type(nx, ny, nz);
        self.nodes[n].kind = t;
        self.nodes[n].cost_malus = self.malus(t);
        n
    }
    fn can_start_at(&mut self, x: i32, y: i32, z: i32) -> bool {
        let t = self.cached_type(x, y, z);
        t != PathType::Open && self.malus(t) >= 0.0
    }
    /// `getNeighbors`.
    fn neighbors(&mut self, out: &mut Vec<usize>, pos: usize) {
        out.clear();
        let (px, py, pz) = (self.nodes[pos].x, self.nodes[pos].y, self.nodes[pos].z);
        let mut jump = 0;
        let above = self.cached_type(px, py + 1, pz);
        let current = self.cached_type(px, py, pz);
        if self.malus(above) >= 0.0 && current != PathType::StickyHoney {
            jump = self.mob.max_up_step.max(1.0).floor() as i32;
        }
        let height = self.floor_level(px, py, pz);
        for (dir, (dx, dz)) in HORIZONTAL.iter().enumerate() {
            let node = self.find_accepted(px + dx, py, pz + dz, jump, height, dir, current);
            self.neighbors_scratch[data2d(dir)] = node;
            if let Some(n) = node
                && self.neighbor_valid(node, pos)
            {
                out.push(n);
            }
        }
        for (dir, (dx, dz)) in HORIZONTAL.iter().enumerate() {
            let second = clockwise(dir);
            let ew = self.neighbors_scratch[data2d(dir)];
            let ns = self.neighbors_scratch[data2d(second)];
            if self.diagonal_valid(pos, ew, ns) {
                let (sx, sz) = HORIZONTAL[second];
                let diagonal = self.find_accepted(px + dx + sx, py, pz + dz + sz, jump, height, dir, current);
                if let Some(d) = diagonal
                    && self.diagonal_node_valid(diagonal)
                {
                    out.push(d);
                }
            }
        }
    }
    fn neighbor_valid(&self, n: Option<usize>, current: usize) -> bool {
        n.is_some_and(|n| !self.nodes[n].closed && (self.nodes[n].cost_malus >= 0.0 || self.nodes[current].cost_malus < 0.0))
    }
    fn diagonal_valid(&self, pos: usize, ew: Option<usize>, ns: Option<usize>) -> bool {
        let (Some(ew), Some(ns)) = (ew, ns) else { return false };
        let (p, ew, ns) = (&self.nodes[pos], &self.nodes[ew], &self.nodes[ns]);
        if ns.y > p.y || ew.y > p.y {
            return false;
        }
        if ew.kind == PathType::WalkableDoor || ns.kind == PathType::WalkableDoor {
            return false;
        }
        if self.mob.width > 1.0 && (ew.cost_malus > 0.0 || ns.cost_malus > 0.0) {
            return false;
        }
        let between_posts = ns.kind == PathType::Fence && ew.kind == PathType::Fence && self.mob.width < 0.5;
        (ns.y < p.y || ns.cost_malus >= 0.0 || between_posts) && (ew.y < p.y || ew.cost_malus >= 0.0 || between_posts)
    }
    fn diagonal_node_valid(&self, n: Option<usize>) -> bool {
        n.is_some_and(|n| {
            let n = &self.nodes[n];
            !n.closed && n.kind != PathType::WalkableDoor && n.cost_malus >= 0.0
        })
    }
    fn partial_collision(t: PathType) -> bool {
        matches!(t, PathType::Fence | PathType::DoorWoodClosed | PathType::DoorIronClosed)
    }
    fn can_reach_without_collision(&mut self, to: usize) -> bool {
        let b = self.mob.bounds();
        let (lo, hi) = (b.min_position(), b.max_position());
        let size = ((hi.x - lo.x) + (hi.y - lo.y) + (hi.z - lo.z)) / 3.0;
        let n = &self.nodes[to];
        let mut delta = [
            f64::from(n.x) - self.mob.position[0] + (hi.x - lo.x) / 2.0,
            f64::from(n.y) - self.mob.position[1] + (hi.y - lo.y) / 2.0,
            f64::from(n.z) - self.mob.position[2] + (hi.z - lo.z) / 2.0,
        ];
        let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
        let steps = (length / size).ceil() as i32;
        let scale = f64::from(1.0f32 / steps as f32);
        delta = delta.map(|d| d * scale);
        let (mut lo, mut hi) = (lo, hi);
        for _ in 1..=steps {
            lo = Vec3::new(lo.x + delta[0], lo.y + delta[1], lo.z + delta[2]);
            hi = Vec3::new(hi.x + delta[0], hi.y + delta[1], hi.z + delta[2]);
            if self.has_collisions(Aabb::new(lo, hi)) {
                return false;
            }
        }
        true
    }
    /// `getFloorLevel` (floating mobs treat floatable fluid as half a block).
    fn floor_level(&self, x: i32, y: i32, z: i32) -> f64 {
        if self.mob.can_float && self.level.is_floatable_fluid(x, y, z) {
            return f64::from(y) + 0.5;
        }
        floor_level(self.level, x, y, z)
    }
    fn mob_jump_height(&self) -> f64 {
        f64::from(self.mob.max_up_step).max(1.125)
    }
    #[allow(clippy::too_many_arguments)]
    fn find_accepted(&mut self, x: i32, y: i32, z: i32, jump: i32, node_height: f64, dir: usize, current: PathType) -> Option<usize> {
        let mut best = None;
        let max_y_target = self.floor_level(x, y, z);
        if max_y_target - node_height > self.mob_jump_height() {
            return None;
        }
        let t = self.cached_type(x, y, z);
        let cost = self.malus(t);
        if cost >= 0.0 {
            best = Some(self.node_update_cost(x, y, z, t, cost));
        }
        if Self::partial_collision(current)
            && let Some(b) = best
            && self.nodes[b].cost_malus >= 0.0
            && !self.can_reach_without_collision(b)
        {
            best = None;
        }
        if t == PathType::Walkable {
            return best;
        }
        if (best.is_none() || best.is_some_and(|b| self.nodes[b].cost_malus < 0.0))
            && jump > 0
            && (t != PathType::Fence || self.mob.can_walk_over_fences)
            && t != PathType::UnpassableRail
            && t != PathType::Trapdoor
            && t != PathType::PowderSnow
        {
            best = self.try_jump_on(x, y, z, jump, node_height, dir, current);
        } else if t == PathType::Water && !self.mob.can_float {
            best = self.first_non_water_below(x, y, z, best);
        } else if t == PathType::Open {
            best = Some(self.first_ground_below(x, y, z));
        } else if Self::partial_collision(t) && best.is_none() {
            best = Some(self.closed_node(x, y, z, t));
        }
        best
    }
    fn node_update_cost(&mut self, x: i32, y: i32, z: i32, t: PathType, cost: f32) -> usize {
        let n = self.node(x, y, z);
        self.nodes[n].kind = t;
        self.nodes[n].cost_malus = self.nodes[n].cost_malus.max(cost);
        n
    }
    fn blocked_node(&mut self, x: i32, y: i32, z: i32) -> usize {
        let n = self.node(x, y, z);
        self.nodes[n].kind = PathType::Blocked;
        self.nodes[n].cost_malus = -1.0;
        n
    }
    fn closed_node(&mut self, x: i32, y: i32, z: i32, t: PathType) -> usize {
        let n = self.node(x, y, z);
        self.nodes[n].closed = true;
        self.nodes[n].kind = t;
        self.nodes[n].cost_malus = t.default_malus();
        n
    }
    #[allow(clippy::too_many_arguments)]
    fn try_jump_on(&mut self, x: i32, y: i32, z: i32, jump: i32, node_height: f64, dir: usize, current: PathType) -> Option<usize> {
        let above = self.find_accepted(x, y + 1, z, jump - 1, node_height, dir, current)?;
        if self.mob.width >= 1.0 {
            return Some(above);
        }
        let a = self.nodes[above];
        if a.kind != PathType::Open && a.kind != PathType::Walkable {
            return Some(above);
        }
        let (sx, sz) = HORIZONTAL[dir];
        let cx = f64::from(x - sx) + 0.5;
        let cz = f64::from(z - sz) + 0.5;
        let half = f64::from(self.mob.width) / 2.0;
        let floor_from = self.floor_level(cx.floor() as i32, y + 1, cz.floor() as i32);
        let floor_to = self.floor_level(a.x, a.y, a.z);
        let grow = Aabb::new(
            Vec3::new(cx - half, floor_from + 0.001, cz - half),
            Vec3::new(cx + half, f64::from(self.mob.height) + floor_to - 0.002, cz + half),
        );
        if self.has_collisions(grow) { None } else { Some(above) }
    }
    fn first_non_water_below(&mut self, x: i32, mut y: i32, z: i32, mut best: Option<usize>) -> Option<usize> {
        y -= 1;
        while y > self.level.min_y() {
            let t = self.cached_type(x, y, z);
            if t != PathType::Water {
                return best;
            }
            let cost = self.malus(t);
            best = Some(self.node_update_cost(x, y, z, t, cost));
            y -= 1;
        }
        best
    }
    fn first_ground_below(&mut self, x: i32, y: i32, z: i32) -> usize {
        let mut cy = y - 1;
        while cy >= self.level.min_y() {
            if y - cy > self.mob.max_fall_distance {
                return self.blocked_node(x, cy, z);
            }
            let t = self.cached_type(x, cy, z);
            let cost = self.malus(t);
            if t != PathType::Open {
                if cost >= 0.0 {
                    return self.node_update_cost(x, cy, z, t, cost);
                }
                return self.blocked_node(x, cy, z);
            }
            cy -= 1;
        }
        self.blocked_node(x, y, z)
    }
    fn has_collisions(&mut self, aabb: Aabb) -> bool {
        let (lo, hi) = (aabb.min_position(), aabb.max_position());
        let key = [lo.x.to_bits(), lo.y.to_bits(), lo.z.to_bits(), hi.x.to_bits(), hi.y.to_bits(), hi.z.to_bits()];
        if let Some(&v) = self.collision_cache.get(&key) {
            return v;
        }
        let v = self.level.has_collision(aabb);
        self.collision_cache.insert(key, v);
        v
    }
    pub fn cached_type(&mut self, x: i32, y: i32, z: i32) -> PathType {
        if let Some(&t) = self.type_cache.get(&(x, y, z)) {
            return t;
        }
        let t = self.path_type_of_mob(x, y, z);
        self.type_cache.insert((x, y, z), t);
        t
    }
    /// `getPathTypeOfMob`.
    fn path_type_of_mob(&self, x: i32, y: i32, z: i32) -> PathType {
        let types = self.types_within_bb(x, y, z);
        if types.len() == 1 {
            return types[0];
        }
        if types.contains(&PathType::Fence) {
            return PathType::Fence;
        }
        if types.contains(&PathType::UnpassableRail) {
            return PathType::UnpassableRail;
        }
        let mut highest_type = PathType::Blocked;
        let mut highest = self.malus(highest_type);
        for &t in &types {
            let m = self.malus(t);
            if m < 0.0 {
                return t;
            }
            if m >= highest {
                highest = m;
                highest_type = t;
            }
        }
        let current = path_type_static(self.level, x, y, z);
        if self.entity_width > 1 {
            let cheaper = self.malus(current) < highest;
            if cheaper && self.malus(PathType::BigMobsCloseToDanger) < highest {
                return PathType::BigMobsCloseToDanger;
            }
            highest_type
        } else if current == PathType::Open && highest_type != PathType::Open && highest == 0.0 {
            PathType::Open
        } else {
            highest_type
        }
    }
    /// `getPathTypeWithinMobBB`: the distinct types in an EnumSet (ordinal order).
    fn types_within_bb(&self, x: i32, y: i32, z: i32) -> Vec<PathType> {
        let mut set = [false; PATH_TYPES];
        let [mx, my, mz] = self.mob.block_position();
        for dx in 0..self.entity_width {
            for dy in 0..self.entity_height {
                for dz in 0..self.entity_depth {
                    let mut t = path_type_static(self.level, x + dx, y + dy, z + dz);
                    if t == PathType::DoorWoodClosed && self.mob.can_open_doors && self.mob.can_pass_doors {
                        t = PathType::WalkableDoor;
                    }
                    if t == PathType::DoorOpen && !self.mob.can_pass_doors {
                        t = PathType::Blocked;
                    }
                    if t == PathType::Rail && path_type_static(self.level, mx, my, mz) != PathType::Rail && path_type_static(self.level, mx, my - 1, mz) != PathType::Rail {
                        t = PathType::UnpassableRail;
                    }
                    set[t as usize] = true;
                }
            }
        }
        PathType::ALL.iter().copied().filter(|t| set[*t as usize]).collect()
    }
}

/// Static `WalkNodeEvaluator.getFloorLevel(level, pos)`.
pub fn floor_level<L: PathLevel + ?Sized>(level: &L, x: i32, y: i32, z: i32) -> f64 {
    f64::from(y - 1) + level.collision_top(x, y - 1, z).unwrap_or(0.0)
}

/// `getPathTypeStatic`: the block's own type, refined by the block below and
/// the neighbourhood when it is open.
pub fn path_type_static<L: PathLevel + ?Sized>(level: &L, x: i32, y: i32, z: i32) -> PathType {
    let t = level.path_type_from_state(x, y, z);
    if t == PathType::Open && y >= level.min_y() + 1 {
        return match level.path_type_from_state(x, y - 1, z) {
            PathType::Open | PathType::Water | PathType::Lava | PathType::Walkable => PathType::Open,
            PathType::Fire => PathType::Fire,
            PathType::Damaging => PathType::Damaging,
            PathType::StickyHoney => PathType::StickyHoney,
            PathType::PowderSnow => PathType::OnTopOfPowderSnow,
            PathType::DamageCautious => PathType::DamageCautious,
            PathType::Trapdoor => PathType::OnTopOfTrapdoor,
            _ => check_neighbour_blocks(level, x, y, z, PathType::Walkable),
        };
    }
    t
}
fn check_neighbour_blocks<L: PathLevel + ?Sized>(level: &L, x: i32, y: i32, z: i32, t: PathType) -> PathType {
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if dx != 0 || dz != 0 {
                    match level.path_type_from_state(x + dx, y + dy, z + dz) {
                        PathType::Damaging => return PathType::DamagingInNeighbor,
                        PathType::Fire | PathType::Lava => return PathType::FireInNeighbor,
                        PathType::Water => return PathType::WaterBorder,
                        PathType::DamageCautious => return PathType::DamageCautious,
                        _ => {}
                    }
                }
            }
        }
    }
    t
}

/// `PathFinder.findPath` from the mob towards one or more target positions.
/// Returns `None` when no start node exists or no target produced a path.
pub fn find_path<L: PathLevel>(level: &L, mob: &PathMob, targets: &[[i32; 3]], max_path_length: f32, reach_range: i32, max_visited_nodes: i32) -> Option<Path> {
    let mut ev = WalkEvaluator::new(level, mob);
    let from = ev.start();
    // Targets: getTargetNodeAt creates (and shares) graph nodes.
    struct Target {
        pos: [i32; 3],
        block: [i32; 3],
        best_h: f32,
        best: Option<usize>,
        reached: bool,
    }
    let mut tos: Vec<Target> = Vec::new();
    for t in targets {
        let n = ev.node(t[0], t[1], t[2]);
        let pos = ev.nodes[n].pos();
        if tos.iter().any(|x| x.pos == pos) {
            continue;
        }
        tos.push(Target { pos, block: *t, best_h: f32::MAX, best: None, reached: false });
    }
    let best_h = |ev: &WalkEvaluator<L>, tos: &mut [Target], n: usize| {
        let mut best = f32::MAX;
        for t in tos.iter_mut() {
            let h = ev.nodes[n].distance_to(t.pos[0], t.pos[1], t.pos[2]);
            if h < t.best_h {
                t.best_h = h;
                t.best = Some(n);
            }
            best = best.min(h);
        }
        best
    };
    ev.nodes[from].g = 0.0;
    let h = best_h(&ev, &mut tos, from);
    ev.nodes[from].h = h;
    ev.nodes[from].f = h;
    let mut open = Heap::default();
    open.insert(&mut ev.nodes, from);
    let mut count = 0;
    let mut neighbors = Vec::with_capacity(32);
    let (fx, fy, fz) = (ev.nodes[from].x, ev.nodes[from].y, ev.nodes[from].z);
    let mut any_reached = false;
    while !open.heap.is_empty() {
        count += 1;
        if count >= max_visited_nodes {
            break;
        }
        let current = open.pop(&mut ev.nodes);
        ev.nodes[current].closed = true;
        for t in tos.iter_mut() {
            if ev.nodes[current].distance_manhattan(t.pos[0], t.pos[1], t.pos[2]) <= reach_range as f32 {
                t.reached = true;
                any_reached = true;
            }
        }
        if any_reached {
            break;
        }
        if ev.nodes[current].distance_to(fx, fy, fz) >= max_path_length {
            continue;
        }
        ev.neighbors(&mut neighbors, current);
        for &nb in &neighbors {
            let distance = {
                let c = &ev.nodes[current];
                let n = &ev.nodes[nb];
                c.distance_to(n.x, n.y, n.z)
            };
            let walked = ev.nodes[current].walked_distance + distance;
            ev.nodes[nb].walked_distance = walked;
            let tentative = ev.nodes[current].g + distance + ev.nodes[nb].cost_malus;
            let in_open = ev.nodes[nb].heap_idx >= 0;
            if walked < max_path_length && (!in_open || tentative < ev.nodes[nb].g) {
                ev.nodes[nb].came_from = Some(current);
                ev.nodes[nb].g = tentative;
                let h = best_h(&ev, &mut tos, nb) * 1.5;
                ev.nodes[nb].h = h;
                if in_open {
                    let f = ev.nodes[nb].g + h;
                    open.change_cost(&mut ev.nodes, nb, f);
                } else {
                    ev.nodes[nb].f = ev.nodes[nb].g + h;
                    open.insert(&mut ev.nodes, nb);
                }
            }
        }
    }
    let reconstruct = |closest: usize, target: [i32; 3], reached: bool| {
        let mut list = vec![ev.nodes[closest]];
        let mut n = closest;
        while let Some(prev) = ev.nodes[n].came_from {
            n = prev;
            list.push(ev.nodes[n]);
            if list.len() > 100_000 {
                break;
            }
        }
        list.reverse();
        Path::new(list, target, reached)
    };
    let paths: Vec<Path> = if any_reached {
        tos.iter().filter(|t| t.reached).filter_map(|t| t.best.map(|b| reconstruct(b, t.block, true))).collect()
    } else {
        tos.iter().filter_map(|t| t.best.map(|b| reconstruct(b, t.block, false))).collect()
    };
    if any_reached {
        paths.into_iter().min_by_key(|p| p.nodes.len())
    } else {
        paths.into_iter().min_by(|a, b| a.dist_to_target.total_cmp(&b.dist_to_target).then(a.nodes.len().cmp(&b.nodes.len())))
    }
}
