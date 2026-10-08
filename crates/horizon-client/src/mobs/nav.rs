//! `PathNavigation` / `GroundPathNavigation`: path requests, following,
//! corner cutting, stuck detection and timeouts.
use super::path::{Path, PathLevel, PathMob, PathType, find_path, floor_level};

/// Mob state the navigation reads each call.
#[derive(Clone, Debug)]
pub struct NavMob {
    pub path: PathMob,
    pub in_liquid: bool,
    /// `LivingEntity.getSpeed()` (set by the move control).
    pub speed: f32,
    /// FOLLOW_RANGE attribute value.
    pub follow_range: f32,
    pub game_time: i64,
}
impl NavMob {
    fn can_update_path(&self) -> bool {
        self.path.on_ground || self.in_liquid
    }
}

#[derive(Clone, Debug)]
pub struct Navigation {
    pub path: Option<Path>,
    pub speed_modifier: f64,
    tick: i32,
    last_stuck_check: i32,
    last_stuck_check_pos: [f64; 3],
    timeout_cached_node: [i32; 3],
    timeout_timer: i64,
    last_timeout_check: i64,
    timeout_limit: f64,
    max_distance_to_waypoint: f32,
    has_delayed_recomputation: bool,
    time_last_recompute: i64,
    target_pos: Option<[i32; 3]>,
    reach_range: i32,
    max_visited_nodes: i32,
    max_visited_multiplier: f32,
    pub is_stuck: bool,
    required_path_length: f32,
    pub avoid_sun: bool,
    pub can_path_below_surface: bool,
    pub can_float: bool,
    pub can_open_doors: bool,
    pub can_pass_doors: bool,
    pub can_walk_over_fences: bool,
}
impl Navigation {
    /// `new PathFinder(..., floor(FOLLOW_RANGE base * 16))`.
    pub fn new(follow_range_base: f64) -> Self {
        Self {
            path: None,
            speed_modifier: 0.0,
            tick: 0,
            last_stuck_check: 0,
            last_stuck_check_pos: [0.0; 3],
            timeout_cached_node: [0; 3],
            timeout_timer: 0,
            last_timeout_check: 0,
            timeout_limit: 0.0,
            max_distance_to_waypoint: 0.5,
            has_delayed_recomputation: false,
            time_last_recompute: 0,
            target_pos: None,
            reach_range: 0,
            max_visited_nodes: (follow_range_base * 16.0).floor() as i32,
            max_visited_multiplier: 1.0,
            is_stuck: false,
            required_path_length: 16.0,
            avoid_sun: false,
            can_path_below_surface: false,
            can_float: false,
            can_open_doors: false,
            can_pass_doors: true,
            can_walk_over_fences: false,
        }
    }
    pub fn set_required_path_length(&mut self, length: f32, follow_range: f32) {
        self.required_path_length = length;
        self.max_visited_nodes = (follow_range.max(length) * 16.0).floor() as i32;
    }
    fn max_path_length(&self, m: &NavMob) -> f32 {
        m.follow_range.max(self.required_path_length)
    }
    pub fn target_pos(&self) -> Option<[i32; 3]> {
        self.target_pos
    }
    pub fn is_done(&self) -> bool {
        self.path.as_ref().is_none_or(Path::is_done)
    }
    pub fn is_in_progress(&self) -> bool {
        !self.is_done()
    }
    pub fn stop(&mut self) {
        self.path = None;
    }
    fn with_flags(&self, m: &NavMob) -> PathMob {
        let mut p = m.path.clone();
        p.can_float = self.can_float;
        p.can_open_doors = self.can_open_doors;
        p.can_pass_doors = self.can_pass_doors;
        p.can_walk_over_fences = self.can_walk_over_fences;
        p
    }
    /// `GroundPathNavigation.createPath(BlockPos, reachRange)`.
    pub fn create_path<L: PathLevel>(&mut self, level: &L, m: &NavMob, pos: [i32; 3], reach_range: i32) -> Option<Path> {
        if !level.loaded(pos[0], pos[2]) {
            return None;
        }
        let pos = if self.can_path_below_surface { pos } else { find_surface_position(level, pos) };
        let max = self.max_path_length(m);
        self.create_path_set(level, m, &[pos], reach_range, max)
    }
    /// `createPath(BlockPos.containing(x, y, z), reach)`.
    pub fn create_path_to<L: PathLevel>(&mut self, level: &L, m: &NavMob, at: [f64; 3], reach_range: i32) -> Option<Path> {
        self.create_path(level, m, [at[0].floor() as i32, at[1].floor() as i32, at[2].floor() as i32], reach_range)
    }
    fn create_path_set<L: PathLevel>(&mut self, level: &L, m: &NavMob, targets: &[[i32; 3]], reach_range: i32, max_path_length: f32) -> Option<Path> {
        if targets.is_empty() || m.path.position[1] < f64::from(level.min_y()) || !m.can_update_path() {
            return None;
        }
        if let Some(p) = &self.path
            && !p.is_done()
            && self.target_pos.is_some_and(|t| targets.contains(&t))
        {
            return Some(p.clone());
        }
        let mob = self.with_flags(m);
        let visited = (self.max_visited_nodes as f32 * self.max_visited_multiplier) as i32;
        let path = find_path(level, &mob, targets, max_path_length, reach_range, visited);
        if let Some(p) = &path {
            self.target_pos = Some(p.target);
            self.reach_range = reach_range;
            self.reset_stuck_timeout();
        }
        path
    }
    /// `moveTo(Path, speed)`.
    pub fn move_to_path<L: PathLevel>(&mut self, level: &L, m: &NavMob, path: Option<Path>, speed: f64) -> bool {
        let Some(path) = path else {
            self.path = None;
            return false;
        };
        if !path.same_as(self.path.as_ref()) {
            self.path = Some(path);
        }
        if self.is_done() {
            return false;
        }
        self.trim_path(level, m);
        if self.path.as_ref().is_none_or(|p| p.nodes.is_empty()) {
            return false;
        }
        self.speed_modifier = speed;
        self.last_stuck_check = self.tick;
        self.last_stuck_check_pos = self.temp_mob_pos(level, m);
        true
    }
    /// `moveTo(x, y, z, speed)` (reach range 1).
    pub fn move_to<L: PathLevel>(&mut self, level: &L, m: &NavMob, at: [f64; 3], speed: f64) -> bool {
        let p = self.create_path_to(level, m, at, 1);
        self.move_to_path(level, m, p, speed)
    }
    /// `moveTo(Entity, reach, speed)` for a target standing at `at`.
    pub fn move_to_entity<L: PathLevel>(&mut self, level: &L, m: &NavMob, at: [f64; 3], reach: i32, speed: f64) -> bool {
        let p = self.create_path_to(level, m, at, reach);
        p.is_some() && self.move_to_path(level, m, p, speed)
    }
    fn recompute_path<L: PathLevel>(&mut self, level: &L, m: &NavMob) {
        if m.game_time - self.time_last_recompute <= 20 || !m.can_update_path() {
            self.has_delayed_recomputation = true;
        } else if let Some(t) = self.target_pos {
            self.path = None;
            self.path = self.create_path(level, m, t, self.reach_range);
            self.time_last_recompute = m.game_time;
            self.has_delayed_recomputation = false;
        }
    }
    /// `shouldRecomputePath` after a block change, then `recomputePath`.
    pub fn block_changed<L: PathLevel>(&mut self, level: &L, m: &NavMob, pos: [i32; 3]) {
        if self.has_delayed_recomputation {
            return;
        }
        let Some(p) = &self.path else { return };
        if p.is_done() || p.nodes.is_empty() {
            return;
        }
        let end = p.end_node().copied().unwrap_or_else(|| *p.next_node());
        let mid = [
            (f64::from(end.x) + m.path.position[0]) / 2.0,
            (f64::from(end.y) + m.path.position[1]) / 2.0,
            (f64::from(end.z) + m.path.position[2]) / 2.0,
        ];
        let d = (p.nodes.len() - p.next) as f64;
        let c = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
        let dist2 = (c[0] - mid[0]).powi(2) + (c[1] - mid[1]).powi(2) + (c[2] - mid[2]).powi(2);
        if dist2 < d * d {
            self.recompute_path(level, m);
        }
    }
    /// `PathNavigation.tick`: returns the move-control wanted position
    /// (`setWantedPosition(x, groundY, z, speedModifier)`).
    pub fn tick<L: PathLevel>(&mut self, level: &L, m: &NavMob) -> Option<([f64; 3], f64)> {
        self.tick += 1;
        if self.has_delayed_recomputation {
            self.recompute_path(level, m);
        }
        if self.is_done() {
            return None;
        }
        if m.can_update_path() {
            self.follow_the_path(level, m);
        } else if let Some(path) = self.path.as_mut()
            && !path.is_done()
        {
            let mob_pos = temp_mob_pos(level, m, self.can_float);
            let pos = path.next_entity_pos(m.path.width);
            if mob_pos[1] > pos[1] && !m.path.on_ground && mob_pos[0].floor() == pos[0].floor() && mob_pos[2].floor() == pos[2].floor() {
                path.advance();
            }
        }
        if self.is_done() {
            return None;
        }
        let path = self.path.as_ref()?;
        let target = path.next_entity_pos(m.path.width);
        let (bx, by, bz) = (target[0].floor() as i32, target[1].floor() as i32, target[2].floor() as i32);
        let ground_y = if level.is_air(bx, by - 1, bz) { target[1] } else { floor_level(level, bx, by, bz) };
        Some(([target[0], ground_y, target[2]], self.speed_modifier))
    }
    fn temp_mob_pos<L: PathLevel>(&self, level: &L, m: &NavMob) -> [f64; 3] {
        temp_mob_pos(level, m, self.can_float)
    }
    fn follow_the_path<L: PathLevel>(&mut self, level: &L, m: &NavMob) {
        let mob_pos = self.temp_mob_pos(level, m);
        let width = m.path.width;
        self.max_distance_to_waypoint = if width > 0.75 { width / 2.0 } else { 0.75 - width / 2.0 };
        let Some(path) = self.path.as_mut() else { return };
        let n = *path.next_node();
        let xd = (m.path.position[0] - (f64::from(n.x) + 0.5)).abs();
        let yd = (m.path.position[1] - f64::from(n.y)).abs();
        let zd = (m.path.position[2] - (f64::from(n.z) + 0.5)).abs();
        let close = xd < f64::from(self.max_distance_to_waypoint) && zd < f64::from(self.max_distance_to_waypoint) && yd < 1.0;
        if close || (can_cut_corner(n.kind) && should_target_next_node_in_direction(path, mob_pos)) {
            path.advance();
        }
        self.do_stuck_detection(m, mob_pos);
    }
    fn do_stuck_detection(&mut self, m: &NavMob, mob_pos: [f64; 3]) {
        if self.tick - self.last_stuck_check > 100 {
            let effective = if m.speed >= 1.0 { m.speed } else { m.speed * m.speed };
            let threshold = f64::from(effective * 100.0 * 0.25);
            let d2 = dist2(mob_pos, self.last_stuck_check_pos);
            if d2 < threshold * threshold {
                self.is_stuck = true;
                self.stop();
            } else {
                self.is_stuck = false;
            }
            self.last_stuck_check = self.tick;
            self.last_stuck_check_pos = mob_pos;
        }
        if let Some(path) = &self.path
            && !path.is_done()
        {
            let pos = path.next_node().pos();
            let time = m.game_time;
            if pos == self.timeout_cached_node {
                self.timeout_timer += time - self.last_timeout_check;
            } else {
                self.timeout_cached_node = pos;
                let c = [f64::from(pos[0]) + 0.5, f64::from(pos[1]), f64::from(pos[2]) + 0.5];
                let dist = dist2(mob_pos, c).sqrt();
                self.timeout_limit = if m.speed > 0.0 { dist / f64::from(m.speed) * 20.0 } else { 0.0 };
            }
            if self.timeout_limit > 0.0 && self.timeout_timer as f64 > self.timeout_limit * 3.0 {
                self.reset_stuck_timeout();
                self.stop();
            }
            self.last_timeout_check = time;
        }
    }
    fn reset_stuck_timeout(&mut self) {
        self.timeout_cached_node = [0; 3];
        self.timeout_timer = 0;
        self.timeout_limit = 0.0;
        self.is_stuck = false;
    }
    fn trim_path<L: PathLevel>(&mut self, level: &L, m: &NavMob) {
        let Some(path) = self.path.as_mut() else { return };
        for i in 0..path.nodes.len() {
            let node = path.nodes[i];
            if level.is_cauldron(node.x, node.y, node.z) {
                let next = path.nodes.get(i + 1).copied();
                path.nodes[i].y = node.y + 1;
                if let Some(next) = next
                    && node.y >= next.y
                {
                    let mut moved = node;
                    moved.x = next.x;
                    moved.y = node.y + 1;
                    moved.z = next.z;
                    path.nodes[i + 1] = moved;
                }
            }
        }
        if self.avoid_sun {
            let p = m.path.position;
            if level.can_see_sky(p[0].floor() as i32, (p[1] + 0.5).floor() as i32, p[2].floor() as i32) {
                return;
            }
            if let Some(i) = path.nodes.iter().position(|n| level.can_see_sky(n.x, n.y, n.z)) {
                path.truncate(i);
            }
        }
    }
    /// `isStableDestination`: solid-render block below.
    pub fn is_stable_destination<L: PathLevel>(level: &L, pos: [i32; 3]) -> bool {
        level.is_solid_render(pos[0], pos[1] - 1, pos[2])
    }
}
fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}
/// `GroundPathNavigation.getTempMobPos`.
fn temp_mob_pos<L: PathLevel>(level: &L, m: &NavMob, can_float: bool) -> [f64; 3] {
    let p = m.path.position;
    let y = if m.path.in_floatable_fluid && can_float {
        let (x, z) = (p[0].floor() as i32, p[2].floor() as i32);
        let start = p[1].floor() as i32;
        let mut surface = start;
        let mut steps = 0;
        let mut result = None;
        while level.is_floatable_fluid(x, surface, z) {
            surface += 1;
            steps += 1;
            if steps > 16 {
                result = Some(start);
                break;
            }
        }
        result.unwrap_or(surface)
    } else {
        (p[1] + 0.5).floor() as i32
    };
    [p[0], f64::from(y), p[2]]
}
fn can_cut_corner(t: PathType) -> bool {
    t != PathType::FireInNeighbor && t != PathType::DamagingInNeighbor && t != PathType::WalkableDoor
}
fn should_target_next_node_in_direction(path: &Path, mob: [f64; 3]) -> bool {
    if path.next + 1 >= path.nodes.len() {
        return false;
    }
    let n = path.next_node();
    let current = [f64::from(n.x) + 0.5, f64::from(n.y), f64::from(n.z) + 0.5];
    if dist2(mob, current) >= 4.0 {
        return false;
    }
    let nn = &path.nodes[path.next + 1];
    let next = [f64::from(nn.x) + 0.5, f64::from(nn.y), f64::from(nn.z) + 0.5];
    let to_current = [current[0] - mob[0], current[1] - mob[1], current[2] - mob[2]];
    let to_next = [next[0] - mob[0], next[1] - mob[1], next[2] - mob[2]];
    let len = |v: [f64; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    let (c2, n2) = (len(to_current), len(to_next));
    if !(n2 < c2) && !(c2 < 0.5) {
        return false;
    }
    let norm = |v: [f64; 3]| {
        let l = len(v).sqrt();
        if l < 1.0e-5 { [0.0; 3] } else { v.map(|x| x / l) }
    };
    let (a, b) = (norm(to_current), norm(to_next));
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] < 0.0
}
/// `GroundPathNavigation.findSurfacePosition`.
pub fn find_surface_position<L: PathLevel>(level: &L, pos: [i32; 3]) -> [i32; 3] {
    let [x, mut y, z] = pos;
    if level.is_air(x, y, z) {
        let mut cy = y - 1;
        while cy >= level.min_y() && level.is_air(x, cy, z) {
            cy -= 1;
        }
        if cy >= level.min_y() {
            return [x, cy + 1, z];
        }
        cy = y + 1;
        while cy <= level.max_y() && level.is_air(x, cy, z) {
            cy += 1;
        }
        y = cy;
    }
    if !level.is_solid(x, y, z) {
        return [x, y, z];
    }
    let mut cy = y + 1;
    while cy <= level.max_y() && level.is_solid(x, cy, z) {
        cy += 1;
    }
    [x, cy, z]
}
