//! `GoalSelector` and the vanilla goals (`net.minecraft.world.entity.ai.goal`).
use super::{
    Cx, Damage, DamageKind, EntityRef, EntityView, Mob, Rand, Request, Species,
    nav::{Navigation, find_surface_position},
    path::{Path, PathLevel, PathType, path_type_static},
    species,
};

pub mod flag {
    pub const MOVE: u8 = 1;
    pub const LOOK: u8 = 2;
    pub const JUMP: u8 = 4;
    pub const TARGET: u8 = 8;
}
const FLAG_BITS: [u8; 4] = [flag::MOVE, flag::LOOK, flag::JUMP, flag::TARGET];

/// `Goal`. Goals receive the owning mob and the tick context.
pub trait Goal: Send + Sync {
    fn name(&self) -> &'static str;
    fn flags(&self) -> u8 {
        0
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool;
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        self.can_use(mob, cx)
    }
    fn interruptable(&self) -> bool {
        true
    }
    fn start(&mut self, _mob: &mut Mob, _cx: &mut Cx) {}
    fn stop(&mut self, _mob: &mut Mob, _cx: &mut Cx) {}
    fn every_tick(&self) -> bool {
        false
    }
    fn tick(&mut self, _mob: &mut Mob, _cx: &mut Cx) {}
}
/// `reducedTickDelay`.
pub fn reduced(ticks: i32) -> i32 {
    (ticks + 1) / 2
}
fn adjusted(every_tick: bool, ticks: i32) -> i32 {
    if every_tick { ticks } else { reduced(ticks) }
}

pub struct Wrapped {
    pub priority: i32,
    pub running: bool,
    pub goal: Box<dyn Goal>,
}
/// `GoalSelector` (insertion-ordered goals, flag locks, disabled flags).
#[derive(Default)]
pub struct GoalSelector {
    pub goals: Vec<Wrapped>,
    locked: [Option<usize>; 4],
    pub disabled: u8,
}
impl GoalSelector {
    pub fn add(&mut self, priority: i32, goal: impl Goal + 'static) {
        self.goals.push(Wrapped { priority, running: false, goal: Box::new(goal) });
    }
    pub fn absorb(&mut self, other: GoalSelector) {
        self.goals.extend(other.goals);
    }
    /// `removeAllGoals(predicate)` by goal name.
    pub fn remove_named(&mut self, name: &str, mob: &mut Mob, cx: &mut Cx) {
        for g in self.goals.iter_mut().filter(|g| g.goal.name() == name && g.running) {
            g.running = false;
            g.goal.stop(mob, cx);
        }
        let before: Vec<bool> = self.goals.iter().map(|g| g.goal.name() == name).collect();
        let mut i = 0;
        self.goals.retain(|_| {
            i += 1;
            !before[i - 1]
        });
        self.locked = [None; 4];
        for (i, g) in self.goals.iter().enumerate() {
            if g.running {
                for (b, bit) in FLAG_BITS.iter().enumerate() {
                    if g.goal.flags() & bit != 0 {
                        self.locked[b] = Some(i);
                    }
                }
            }
        }
    }
    pub fn is_running(&self, name: &str) -> bool {
        self.goals.iter().any(|g| g.running && g.goal.name() == name)
    }
    fn can_be_replaced(&self, index: usize) -> bool {
        let g = &self.goals[index];
        let flags = g.goal.flags();
        for (b, bit) in FLAG_BITS.iter().enumerate() {
            if flags & bit != 0
                && let Some(holder) = self.locked[b]
            {
                let h = &self.goals[holder];
                if !(h.goal.interruptable() && g.priority < h.priority) {
                    return false;
                }
            }
        }
        true
    }
    pub fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        for g in self.goals.iter_mut() {
            if g.running && (g.goal.flags() & self.disabled != 0 || !g.goal.can_continue(mob, cx)) {
                g.running = false;
                g.goal.stop(mob, cx);
            }
        }
        for slot in self.locked.iter_mut() {
            if slot.is_some_and(|i| !self.goals[i].running) {
                *slot = None;
            }
        }
        for i in 0..self.goals.len() {
            if self.goals[i].running || self.goals[i].goal.flags() & self.disabled != 0 || !self.can_be_replaced(i) {
                continue;
            }
            if !self.goals[i].goal.can_use(mob, cx) {
                continue;
            }
            let flags = self.goals[i].goal.flags();
            for (b, bit) in FLAG_BITS.iter().enumerate() {
                if flags & bit != 0 {
                    if let Some(h) = self.locked[b]
                        && self.goals[h].running
                    {
                        self.goals[h].running = false;
                        self.goals[h].goal.stop(mob, cx);
                    }
                    self.locked[b] = Some(i);
                }
            }
            if !self.goals[i].running {
                self.goals[i].running = true;
                self.goals[i].goal.start(mob, cx);
            }
        }
        self.tick_running(mob, cx, true);
    }
    pub fn tick_running(&mut self, mob: &mut Mob, cx: &mut Cx, all: bool) {
        for g in self.goals.iter_mut() {
            if g.running && (all || g.goal.every_tick()) {
                g.goal.tick(mob, cx);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Navigation helpers on the mob.
impl Mob {
    pub fn nav_move_to(&mut self, cx: &Cx, at: [f64; 3], speed: f64) -> bool {
        let m = self.nav_mob(cx.w.game_time);
        self.nav.move_to(&cx.level, &m, at, speed)
    }
    pub fn nav_move_to_entity(&mut self, cx: &Cx, at: [f64; 3], reach: i32, speed: f64) -> bool {
        let m = self.nav_mob(cx.w.game_time);
        self.nav.move_to_entity(&cx.level, &m, at, reach, speed)
    }
    pub fn nav_create_path(&mut self, cx: &Cx, at: [f64; 3], reach: i32) -> Option<Path> {
        let m = self.nav_mob(cx.w.game_time);
        self.nav.create_path_to(&cx.level, &m, at, reach)
    }
    pub fn nav_move_path(&mut self, cx: &Cx, path: Option<Path>, speed: f64) -> bool {
        let m = self.nav_mob(cx.w.game_time);
        self.nav.move_to_path(&cx.level, &m, path, speed)
    }
    /// `getPathfindingMalus(getPathTypeStatic(pos)) != 0`.
    pub fn has_malus(&self, cx: &Cx, p: [i32; 3]) -> bool {
        self.malus.get(path_type_static(&cx.level, p[0], p[1], p[2])) != 0.0
    }
    /// `Sensing.hasLineOfSight`.
    pub fn can_see(&self, cx: &Cx, target: &EntityView) -> bool {
        cx.level.line_of_sight(self.eye(), target.eye())
    }
    /// `isWithinMeleeAttackRange` with the default reach.
    pub fn within_melee_range(&self, target: &EntityView) -> bool {
        let reach = f64::from(2.04f32).sqrt() - f64::from(0.6f32);
        let b = self.bounds();
        let (lo, hi) = (b.min_position(), b.max_position());
        let t = target.bounds();
        let (tl, th) = (t.min_position(), t.max_position());
        lo.x - reach < th.x && hi.x + reach > tl.x && lo.y < th.y && hi.y > tl.y && lo.z - reach < th.z && hi.z + reach > tl.z
    }
    /// `Mob.doHurtTarget`: attack damage to the target with knockback.
    pub fn do_hurt_target(&mut self, cx: &mut Cx, target: EntityRef) -> bool {
        let damage = species::attack_damage(self);
        let from = self.pos();
        cx.requests.push(Request::Hurt { target, damage: Damage { amount: damage, kind: DamageKind::MobAttack, attacker: Some(EntityRef::Mob(self.id)), from: Some(from) } });
        let kb = self.attributes.attack_knockback;
        if kb > 0.0 {
            let yaw = self.yaw.to_radians();
            cx.requests.push(Request::Knockback { target, strength: kb * 0.5, dx: f64::from(yaw.sin()), dz: -f64::from(yaw.cos()) });
            self.velocity.x *= 0.6;
            self.velocity.z *= 0.6;
        }
        self.last_hurt_mob = Some(target);
        species::on_hurt_target(self, cx, target);
        true
    }
}

// ---------------------------------------------------------------------------
// Random positions (`DefaultRandomPos`, `LandRandomPos`, `RandomPos`).
fn random_direction(mob: &mut Mob, h: i32, v: i32) -> [i32; 3] {
    let x = mob.random.int(2 * h + 1) - h;
    let y = mob.random.int(2 * v + 1) - v;
    let z = mob.random.int(2 * h + 1) - h;
    [x, y, z]
}
#[allow(clippy::too_many_arguments)]
fn random_direction_within(mob: &mut Mob, min_h: f64, max_h: f64, v: i32, flying: i32, xd: f64, zd: f64, max_rad: f64) -> Option<[i32; 3]> {
    let center = zd.atan2(xd) - f64::from(std::f32::consts::FRAC_PI_2);
    let y_rad = center + f64::from(2.0 * mob.random.float() - 1.0) * max_rad;
    let dist = (min_h + mob.random.double().sqrt() * (max_h - min_h)) * f64::from(std::f32::consts::SQRT_2);
    let xt = -dist * y_rad.sin();
    let zt = dist * y_rad.cos();
    if xt.abs() > max_h || zt.abs() > max_h {
        return None;
    }
    let yt = mob.random.int(2 * v + 1) - v + flying;
    Some([xt.floor() as i32, yt, zt.floor() as i32])
}
fn restricted(mob: &Mob, h: f64) -> bool {
    mob.home.is_some_and(|(c, r)| {
        let d2 = (f64::from(c[0]) + 0.5 - mob.position.x).powi(2) + (f64::from(c[1]) + 0.5 - mob.position.y).powi(2) + (f64::from(c[2]) + 0.5 - mob.position.z).powi(2);
        let lim = f64::from(r) + h + 1.0;
        d2 < lim * lim
    })
}
fn within_home(mob: &Mob, p: [i32; 3]) -> bool {
    match mob.home {
        None => true,
        Some((c, r)) => {
            let d2 = f64::from((c[0] - p[0]).pow(2) + (c[1] - p[1]).pow(2) + (c[2] - p[2]).pow(2));
            d2 < f64::from(r * r)
        }
    }
}
fn pos_toward_direction(mob: &mut Mob, xz: f64, dir: [i32; 3]) -> [i32; 3] {
    let (mut xt, mut zt) = (f64::from(dir[0]), f64::from(dir[2]));
    if let Some((c, _)) = mob.home
        && xz > 1.0
    {
        if mob.position.x > f64::from(c[0]) {
            xt -= mob.random.double() * xz / 2.0;
        } else {
            xt += mob.random.double() * xz / 2.0;
        }
        if mob.position.z > f64::from(c[2]) {
            zt -= mob.random.double() * xz / 2.0;
        } else {
            zt += mob.random.double() * xz / 2.0;
        }
    }
    [(xt + mob.position.x).floor() as i32, (f64::from(dir[1]) + mob.position.y).floor() as i32, (zt + mob.position.z).floor() as i32]
}
fn outside_limits(cx: &Cx, p: [i32; 3]) -> bool {
    p[1] < cx.level.min_y() || p[1] > cx.level.max_y()
}
/// `RandomPos.generateRandomPos` with the mob's walk target weighting.
fn generate(mob: &mut Mob, cx: &Cx, mut supplier: impl FnMut(&mut Mob, &Cx) -> Option<[i32; 3]>) -> Option<[f64; 3]> {
    let mut best = f64::NEG_INFINITY;
    let mut best_pos = None;
    for _ in 0..10 {
        if let Some(p) = supplier(mob, cx) {
            let value = f64::from(mob.walk_target_value(cx, p));
            if value > best {
                best = value;
                best_pos = Some(p);
            }
        }
    }
    best_pos.map(|p| [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5])
}
fn default_candidate(mob: &mut Mob, cx: &Cx, h: i32, restrict: bool, dir: [i32; 3]) -> Option<[i32; 3]> {
    let p = pos_toward_direction(mob, f64::from(h), dir);
    (!outside_limits(cx, p) && !(restrict && !within_home(mob, p)) && Navigation::is_stable_destination(&cx.level, p) && !mob.has_malus(cx, p)).then_some(p)
}
/// `DefaultRandomPos.getPos`.
pub fn default_random_pos(mob: &mut Mob, cx: &Cx, h: i32, v: i32) -> Option<[f64; 3]> {
    let restrict = restricted(mob, f64::from(h));
    generate(mob, cx, |mob, cx| {
        let dir = random_direction(mob, h, v);
        default_candidate(mob, cx, h, restrict, dir)
    })
}
/// `DefaultRandomPos.getPosAway`.
pub fn default_random_pos_away(mob: &mut Mob, cx: &Cx, h: i32, v: i32, from: [f64; 3]) -> Option<[f64; 3]> {
    let away = [mob.position.x - from[0], mob.position.z - from[2]];
    let restrict = restricted(mob, f64::from(h));
    generate(mob, cx, |mob, cx| {
        let dir = random_direction_within(mob, 0.0, f64::from(h), v, 0, away[0], away[1], f64::from(std::f32::consts::FRAC_PI_2))?;
        default_candidate(mob, cx, h, restrict, dir)
    })
}
/// `DefaultRandomPos.getPosTowards`.
pub fn default_random_pos_towards(mob: &mut Mob, cx: &Cx, h: i32, v: i32, to: [f64; 3], max_rad: f64) -> Option<[f64; 3]> {
    let dir2 = [to[0] - mob.position.x, to[2] - mob.position.z];
    let restrict = restricted(mob, f64::from(h));
    generate(mob, cx, |mob, cx| {
        let dir = random_direction_within(mob, 0.0, f64::from(h), v, 0, dir2[0], dir2[1], max_rad)?;
        default_candidate(mob, cx, h, restrict, dir)
    })
}
fn move_up_out_of_solid(cx: &Cx, p: [i32; 3]) -> [i32; 3] {
    if !cx.level.is_solid(p[0], p[1], p[2]) {
        return p;
    }
    let mut y = p[1] + 1;
    while y <= cx.level.max_y() && cx.level.is_solid(p[0], y, p[2]) {
        y += 1;
    }
    [p[0], y, p[2]]
}
fn land_candidate(mob: &mut Mob, cx: &Cx, h: f64, restrict: bool, dir: [i32; 3]) -> Option<[i32; 3]> {
    let p = pos_toward_direction(mob, h, dir);
    if outside_limits(cx, p) || (restrict && !within_home(mob, p)) || !Navigation::is_stable_destination(&cx.level, p) {
        return None;
    }
    let p = move_up_out_of_solid(cx, p);
    (!cx.level.is_water(p[0], p[1], p[2]) && !mob.has_malus(cx, p)).then_some(p)
}
/// `LandRandomPos.getPos`.
pub fn land_random_pos(mob: &mut Mob, cx: &Cx, h: i32, v: i32) -> Option<[f64; 3]> {
    let restrict = restricted(mob, f64::from(h));
    generate(mob, cx, |mob, cx| {
        let dir = random_direction(mob, h, v);
        land_candidate(mob, cx, f64::from(h), restrict, dir)
    })
}
/// `LandRandomPos.getPosAway`.
pub fn land_random_pos_away(mob: &mut Mob, cx: &Cx, h: i32, v: i32, from: [f64; 3]) -> Option<[f64; 3]> {
    let mut away = [mob.position.x - from[0], mob.position.z - from[2]];
    if away[0] == 0.0 && away[1] == 0.0 && (mob.position.y - from[1]) == 0.0 {
        away = [mob.random.double() - 0.5, mob.random.double() - 0.5];
    }
    let restrict = restricted(mob, f64::from(h));
    generate(mob, cx, |mob, cx| {
        let dir = random_direction_within(mob, 0.0, f64::from(h), v, 0, away[0], away[1], f64::from(std::f32::consts::FRAC_PI_2))?;
        land_candidate(mob, cx, f64::from(h), restrict, dir)
    })
}
/// `LandRandomPos.getPosTowards`.
pub fn land_random_pos_towards(mob: &mut Mob, cx: &Cx, h: i32, v: i32, to: [f64; 3]) -> Option<[f64; 3]> {
    let dir2 = [to[0] - mob.position.x, to[2] - mob.position.z];
    let restrict = restricted(mob, f64::from(h));
    generate(mob, cx, |mob, cx| {
        let dir = random_direction_within(mob, 0.0, f64::from(h), v, 0, dir2[0], dir2[1], f64::from(std::f32::consts::FRAC_PI_2))?;
        land_candidate(mob, cx, f64::from(h), restrict, dir)
    })
}

// ---------------------------------------------------------------------------
// Targeting (`TargetingConditions`) and entity searches.
#[derive(Clone, Copy, Debug)]
pub struct Targeting {
    pub combat: bool,
    pub range: f64,
    pub check_los: bool,
    pub test_invisible: bool,
}
impl Targeting {
    pub fn combat() -> Self {
        Self { combat: true, range: -1.0, check_los: true, test_invisible: true }
    }
    pub fn non_combat() -> Self {
        Self { combat: false, range: -1.0, check_los: true, test_invisible: true }
    }
    pub fn range(mut self, r: f64) -> Self {
        self.range = r;
        self
    }
    pub fn ignore_los(mut self) -> Self {
        self.check_los = false;
        self
    }
    pub fn ignore_invisibility(mut self) -> Self {
        self.test_invisible = false;
        self
    }
    /// `TargetingConditions.test(level, targeter, target)`.
    pub fn test(&self, mob: &Mob, cx: &Cx, t: &EntityView) -> bool {
        if t.r == EntityRef::Mob(mob.id) || !t.alive {
            return false;
        }
        if self.combat && (t.creative || !species::can_attack(mob, t)) {
            return false;
        }
        if self.range > 0.0 {
            let visibility = if self.test_invisible { species::visibility_percent(mob, cx, t) } else { 1.0 };
            let d = (self.range * visibility).max(2.0);
            if t.dist2(mob.pos()) > d * d {
                return false;
            }
        }
        if self.check_los && !mob.can_see(cx, t) {
            return false;
        }
        true
    }
}
/// `getEntitiesOfClass(bb.inflate(h, v, h))` filtered by `pred`, nearest by
/// distance to `from` that passes `cond` (`Level.getNearestEntity`).
pub fn nearest<'c>(mob: &Mob, cx: &'c Cx, h: f64, v: f64, from: [f64; 3], cond: &Targeting, pred: &dyn Fn(&EntityView) -> bool) -> Option<&'c EntityView> {
    let b = mob.bounds();
    let (lo, hi) = (b.min_position(), b.max_position());
    let mut best: Option<&EntityView> = None;
    let mut best_d = -1.0;
    for e in cx.entities {
        if !pred(e) {
            continue;
        }
        let eb = e.bounds();
        let (el, eh) = (eb.min_position(), eb.max_position());
        if !(el.x < hi.x + h && eh.x > lo.x - h && el.y < hi.y + v && eh.y > lo.y - v && el.z < hi.z + h && eh.z > lo.z - h) {
            continue;
        }
        if !cond.test(mob, cx, e) {
            continue;
        }
        let d = e.dist2(from);
        if best_d == -1.0 || d < best_d {
            best_d = d;
            best = Some(e);
        }
    }
    best
}
/// `Level.getNearestPlayer(conditions, mob, x, y, z)`.
pub fn nearest_player<'c>(mob: &Mob, cx: &'c Cx, from: [f64; 3], cond: &Targeting, pred: &dyn Fn(&EntityView) -> bool) -> Option<&'c EntityView> {
    let p = cx.player()?;
    (pred(p) && cond.test(mob, cx, p)).then_some(p).filter(|_| {
        let _ = from;
        true
    })
}

// ---------------------------------------------------------------------------
// Goals.

pub struct FloatGoal;
impl Goal for FloatGoal {
    fn name(&self) -> &'static str {
        "float"
    }
    fn flags(&self) -> u8 {
        flag::JUMP
    }
    fn can_use(&mut self, mob: &mut Mob, _cx: &mut Cx) -> bool {
        (mob.in_liquid() && mob.water_height.max(mob.lava_height) > mob.fluid_jump_threshold()) || mob.in_lava
    }
    fn every_tick(&self) -> bool {
        true
    }
    fn tick(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        if mob.random.float() < 0.8 {
            mob.jump_control = true;
        }
    }
}

pub struct PanicGoal {
    pub speed: f64,
    pos: [f64; 3],
}
impl PanicGoal {
    pub fn new(speed: f64) -> Self {
        Self { speed, pos: [0.0; 3] }
    }
    fn look_for_water(mob: &Mob, cx: &Cx, xz: i32) -> Option<[i32; 3]> {
        let [x, y, z] = mob.block_pos();
        if !cx.level.info_at(x, y, z).collision_top.is_nan() {
            return None;
        }
        // findBlocksInBoxByManhattanDistance(pos, xz, 1): ordered by Manhattan distance.
        let mut best: Option<([i32; 3], i32)> = None;
        for dy in -1..=1 {
            for dx in -xz..=xz {
                for dz in -xz..=xz {
                    let d = dx.abs() + dy.abs() + dz.abs();
                    if cx.level.is_water(x + dx, y + dy, z + dz) && best.is_none_or(|(_, bd)| d < bd) {
                        best = Some(([x + dx, y + dy, z + dz], d));
                    }
                }
            }
        }
        best.map(|b| b.0)
    }
}
impl Goal for PanicGoal {
    fn name(&self) -> &'static str {
        "panic"
    }
    fn flags(&self) -> u8 {
        flag::MOVE
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        let should = mob.last_damage.is_some_and(|(k, t)| cx.w.game_time - t <= 40 && species::panic_causes(mob, k));
        if !should {
            return false;
        }
        if mob.fire_ticks > 0
            && let Some(p) = Self::look_for_water(mob, cx, 5)
        {
            self.pos = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
            return true;
        }
        match default_random_pos(mob, cx, 5, 4) {
            Some(p) => {
                self.pos = p;
                true
            }
            None => false,
        }
    }
    fn can_continue(&mut self, mob: &mut Mob, _cx: &mut Cx) -> bool {
        !mob.nav.is_done()
    }
    fn start(&mut self, mob: &mut Mob, cx: &mut Cx) {
        mob.nav_move_to(cx, self.pos, self.speed);
        mob.panicking = true;
    }
    fn stop(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.panicking = false;
    }
}

pub struct RandomStrollGoal {
    pub speed: f64,
    pub interval: i32,
    pub check_no_action_time: bool,
    pub water_avoiding: Option<f32>,
    wanted: [f64; 3],
    pub force: bool,
}
impl RandomStrollGoal {
    pub fn new(speed: f64) -> Self {
        Self { speed, interval: 120, check_no_action_time: true, water_avoiding: None, wanted: [0.0; 3], force: false }
    }
    /// `WaterAvoidingRandomStrollGoal`.
    pub fn water_avoiding(speed: f64) -> Self {
        Self { water_avoiding: Some(0.001), ..Self::new(speed) }
    }
    pub fn with_interval(mut self, interval: i32) -> Self {
        self.interval = interval;
        self
    }
    fn position(&mut self, mob: &mut Mob, cx: &Cx) -> Option<[f64; 3]> {
        match self.water_avoiding {
            None => default_random_pos(mob, cx, 10, 7),
            Some(probability) => {
                if mob.in_water {
                    land_random_pos(mob, cx, 15, 7).or_else(|| default_random_pos(mob, cx, 10, 7))
                } else if mob.random.float() >= probability {
                    land_random_pos(mob, cx, 10, 7)
                } else {
                    default_random_pos(mob, cx, 10, 7)
                }
            }
        }
    }
}
impl Goal for RandomStrollGoal {
    fn name(&self) -> &'static str {
        "random_stroll"
    }
    fn flags(&self) -> u8 {
        flag::MOVE
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if !self.force {
            if self.check_no_action_time && mob.no_action_time >= 100 {
                return false;
            }
            if mob.random.int(reduced(self.interval)) != 0 {
                return false;
            }
        }
        let Some(p) = self.position(mob, cx) else { return false };
        self.wanted = p;
        self.force = false;
        true
    }
    fn can_continue(&mut self, mob: &mut Mob, _cx: &mut Cx) -> bool {
        !mob.nav.is_done()
    }
    fn start(&mut self, mob: &mut Mob, cx: &mut Cx) {
        mob.nav_move_to(cx, self.wanted, self.speed);
    }
    fn stop(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.nav.stop();
    }
}

/// `LookAtPlayerGoal` (players, or another entity class through `filter`).
pub struct LookAtGoal {
    pub distance: f32,
    pub probability: f32,
    pub only_horizontal: bool,
    pub players: bool,
    pub filter: fn(&EntityView) -> bool,
    look_at: Option<EntityRef>,
    look_time: i32,
}
impl LookAtGoal {
    pub fn player(distance: f32) -> Self {
        Self { distance, probability: 0.02, only_horizontal: false, players: true, filter: |_| true, look_at: None, look_time: 0 }
    }
    pub fn entity(distance: f32, filter: fn(&EntityView) -> bool) -> Self {
        Self { players: false, filter, ..Self::player(distance) }
    }
    pub fn probability(mut self, p: f32) -> Self {
        self.probability = p;
        self
    }
}
impl Goal for LookAtGoal {
    fn name(&self) -> &'static str {
        "look_at_player"
    }
    fn flags(&self) -> u8 {
        flag::LOOK
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if mob.random.float() >= self.probability {
            return false;
        }
        if mob.target.is_some() {
            self.look_at = mob.target;
        }
        let cond = Targeting::non_combat().range(f64::from(self.distance));
        let eye = mob.eye();
        let filter = self.filter;
        self.look_at = if self.players {
            nearest_player(mob, cx, eye, &cond, &|e| e.is_player()).map(|e| e.r)
        } else {
            let d = f64::from(self.distance);
            nearest(mob, cx, d, 3.0, eye, &cond, &|e| !e.is_player() && filter(e)).map(|e| e.r)
        };
        self.look_at.is_some()
    }
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        let Some(t) = self.look_at.and_then(|r| cx.view(r)) else { return false };
        if !t.alive {
            return false;
        }
        let d = f64::from(self.distance);
        if t.dist2(mob.pos()) > d * d {
            return false;
        }
        self.look_time > 0
    }
    fn start(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        let r = mob.random.int(40);
        self.look_time = adjusted(false, 40 + r);
    }
    fn stop(&mut self, _mob: &mut Mob, _cx: &mut Cx) {
        self.look_at = None;
    }
    fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        if let Some(t) = self.look_at.and_then(|r| cx.view(r))
            && t.alive
        {
            let y = if self.only_horizontal { mob.eye_y() } else { t.eye()[1] };
            let (ys, xs) = (mob.head_rot_speed() as f32, mob.max_head_x_rot() as f32);
            mob.look_control.set_look_at([t.position[0], y, t.position[2]], ys, xs);
            self.look_time -= 1;
        }
    }
}

pub struct RandomLookAroundGoal {
    rel: [f64; 2],
    look_time: i32,
}
impl RandomLookAroundGoal {
    pub fn new() -> Self {
        Self { rel: [0.0; 2], look_time: 0 }
    }
}
impl Default for RandomLookAroundGoal {
    fn default() -> Self {
        Self::new()
    }
}
impl Goal for RandomLookAroundGoal {
    fn name(&self) -> &'static str {
        "random_look_around"
    }
    fn flags(&self) -> u8 {
        flag::MOVE | flag::LOOK
    }
    fn can_use(&mut self, mob: &mut Mob, _cx: &mut Cx) -> bool {
        mob.random.float() < 0.02
    }
    fn can_continue(&mut self, _mob: &mut Mob, _cx: &mut Cx) -> bool {
        self.look_time >= 0
    }
    fn start(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        let rnd = std::f64::consts::TAU * mob.random.double();
        self.rel = [rnd.cos(), rnd.sin()];
        self.look_time = 20 + mob.random.int(20);
    }
    fn every_tick(&self) -> bool {
        true
    }
    fn tick(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        self.look_time -= 1;
        let at = [mob.position.x + self.rel[0], mob.eye_y(), mob.position.z + self.rel[1]];
        let (ys, xs) = (mob.head_rot_speed() as f32, mob.max_head_x_rot() as f32);
        mob.look_control.set_look_at(at, ys, xs);
    }
}

/// `TemptGoal` (items from an item tag, or explicit item ids).
pub struct TemptGoal {
    pub speed: f64,
    pub tag: &'static str,
    pub items: &'static [&'static str],
    pub can_scare: bool,
    pub stop_distance: f64,
    player: Option<EntityRef>,
    p: [f64; 3],
    p_rot: [f32; 2],
    calm_down: i32,
}
impl TemptGoal {
    pub fn new(speed: f64, tag: &'static str, can_scare: bool) -> Self {
        Self { speed, tag, items: &[], can_scare, stop_distance: 2.5, player: None, p: [0.0; 3], p_rot: [0.0; 2], calm_down: 0 }
    }
    pub fn items(speed: f64, items: &'static [&'static str], can_scare: bool) -> Self {
        Self { items, ..Self::new(speed, "", can_scare) }
    }
    fn tempting(&self, cx: &Cx, item: Option<&str>) -> bool {
        item.is_some_and(|i| (!self.tag.is_empty() && cx.w.rules.item_in_tag(self.tag, i)) || self.items.contains(&i))
    }
}
impl Goal for TemptGoal {
    fn name(&self) -> &'static str {
        "tempt"
    }
    fn flags(&self) -> u8 {
        flag::MOVE | flag::LOOK
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if self.calm_down > 0 {
            self.calm_down -= 1;
            return false;
        }
        let Some(pv) = cx.w.player.as_ref() else { return false };
        let ok = self.tempting(cx, pv.main_hand.as_deref()) || self.tempting(cx, pv.off_hand.as_deref());
        let cond = Targeting::non_combat().ignore_los().range(mob.attributes.tempt_range);
        self.player = if ok { nearest_player(mob, cx, mob.pos(), &cond, &|_| true).map(|e| e.r) } else { None };
        self.player.is_some()
    }
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if self.can_scare
            && let Some(pv) = cx.w.player.as_ref()
        {
            if mob.distance_to_sqr(pv.position) < 36.0 {
                let d2 = (pv.position[0] - self.p[0]).powi(2) + (pv.position[1] - self.p[1]).powi(2) + (pv.position[2] - self.p[2]).powi(2);
                if d2 > 0.010000000000000002 {
                    return false;
                }
                if (pv.pitch - self.p_rot[0]).abs() > 5.0 || (pv.yaw - self.p_rot[1]).abs() > 5.0 {
                    return false;
                }
            } else {
                self.p = pv.position;
            }
            self.p_rot = [pv.pitch, pv.yaw];
        }
        self.can_use(mob, cx)
    }
    fn start(&mut self, _mob: &mut Mob, cx: &mut Cx) {
        if let Some(pv) = cx.w.player.as_ref() {
            self.p = pv.position;
        }
    }
    fn stop(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        self.player = None;
        mob.nav.stop();
        self.calm_down = reduced(100);
    }
    fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        let Some(p) = self.player.and_then(|r| cx.view(r)).cloned() else { return };
        let (ys, xs) = ((mob.max_head_y_rot() + 20) as f32, mob.max_head_x_rot() as f32);
        mob.look_control.set_look_at(p.eye(), ys, xs);
        if mob.distance_to_sqr(p.position) < self.stop_distance * self.stop_distance {
            mob.nav.stop();
        } else {
            mob.nav_move_to_entity(cx, p.position, 1, self.speed);
        }
    }
}

pub struct BreedGoal {
    pub speed: f64,
    partner: Option<u32>,
    love_time: i32,
}
impl BreedGoal {
    pub fn new(speed: f64) -> Self {
        Self { speed, partner: None, love_time: 0 }
    }
}
impl Goal for BreedGoal {
    fn name(&self) -> &'static str {
        "breed"
    }
    fn flags(&self) -> u8 {
        flag::MOVE | flag::LOOK
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if mob.in_love <= 0 {
            return false;
        }
        let me = mob.species();
        let cond = Targeting::non_combat().range(8.0).ignore_los();
        let mut best = None;
        let mut dist = f64::MAX;
        let b = mob.bounds();
        let (lo, hi) = (b.min_position(), b.max_position());
        for e in cx.entities {
            if !e.is(me) || e.r == EntityRef::Mob(mob.id) || !e.in_love || e.panicking || !mob.in_love.gt(&0) {
                continue;
            }
            let eb = e.bounds();
            let (el, eh) = (eb.min_position(), eb.max_position());
            if !(el.x < hi.x + 8.0 && eh.x > lo.x - 8.0 && el.y < hi.y + 8.0 && eh.y > lo.y - 8.0 && el.z < hi.z + 8.0 && eh.z > lo.z - 8.0) {
                continue;
            }
            if !cond.test(mob, cx, e) || !species::can_mate(mob, e) {
                continue;
            }
            let d = e.dist2(mob.pos());
            if d < dist {
                dist = d;
                best = Some(e);
            }
        }
        self.partner = best.and_then(|e| match e.r {
            EntityRef::Mob(id) => Some(id),
            EntityRef::Player => None,
        });
        self.partner.is_some()
    }
    fn can_continue(&mut self, _mob: &mut Mob, cx: &mut Cx) -> bool {
        let Some(p) = self.partner.and_then(|id| cx.view(EntityRef::Mob(id))) else { return false };
        p.alive && p.in_love && self.love_time < 60 && !p.panicking
    }
    fn stop(&mut self, _mob: &mut Mob, _cx: &mut Cx) {
        self.partner = None;
        self.love_time = 0;
    }
    fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        let Some(p) = self.partner.and_then(|id| cx.view(EntityRef::Mob(id))).cloned() else { return };
        let xs = mob.max_head_x_rot() as f32;
        mob.look_control.set_look_at(p.eye(), 10.0, xs);
        mob.nav_move_to_entity(cx, p.position, 1, self.speed);
        self.love_time += 1;
        if self.love_time >= adjusted(false, 60) && mob.distance_to_sqr(p.position) < 9.0 {
            species::breed(mob, cx, &p);
        }
    }
}

pub struct FollowParentGoal {
    pub speed: f64,
    parent: Option<u32>,
    recalc: i32,
}
impl FollowParentGoal {
    pub fn new(speed: f64) -> Self {
        Self { speed, parent: None, recalc: 0 }
    }
}
impl Goal for FollowParentGoal {
    fn name(&self) -> &'static str {
        "follow_parent"
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if mob.age_ticks >= 0 {
            return false;
        }
        let me = mob.species();
        let b = mob.bounds();
        let (lo, hi) = (b.min_position(), b.max_position());
        let mut closest = None;
        let mut best = f64::MAX;
        for e in cx.entities {
            if !e.is(me) || e.age < 0 {
                continue;
            }
            let eb = e.bounds();
            let (el, eh) = (eb.min_position(), eb.max_position());
            if !(el.x < hi.x + 8.0 && eh.x > lo.x - 8.0 && el.y < hi.y + 4.0 && eh.y > lo.y - 4.0 && el.z < hi.z + 8.0 && eh.z > lo.z - 8.0) {
                continue;
            }
            let d = e.dist2(mob.pos());
            if d <= best {
                best = d;
                closest = Some(e.r);
            }
        }
        let Some(EntityRef::Mob(id)) = closest else { return false };
        if best < 9.0 {
            return false;
        }
        self.parent = Some(id);
        true
    }
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if mob.age_ticks >= 0 {
            return false;
        }
        let Some(p) = self.parent.and_then(|id| cx.view(EntityRef::Mob(id))) else { return false };
        if !p.alive {
            return false;
        }
        let d = p.dist2(mob.pos());
        !(d < 9.0) && !(d > 256.0)
    }
    fn start(&mut self, _mob: &mut Mob, _cx: &mut Cx) {
        self.recalc = 0;
    }
    fn stop(&mut self, _mob: &mut Mob, _cx: &mut Cx) {
        self.parent = None;
    }
    fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        self.recalc -= 1;
        if self.recalc <= 0 {
            self.recalc = adjusted(false, 10);
            if let Some(p) = self.parent.and_then(|id| cx.view(EntityRef::Mob(id))).map(|p| p.position) {
                mob.nav_move_to_entity(cx, p, 1, self.speed);
            }
        }
    }
}

/// `MeleeAttackGoal` (with the zombie arm-raise and spider daylight variants).
pub struct MeleeAttackGoal {
    pub speed: f64,
    pub follow_even_if_not_seen: bool,
    pub variant: MeleeVariant,
    path: Option<Path>,
    pathed: [f64; 3],
    recalc: i32,
    until_attack: i32,
    last_can_use: i64,
    raise_arm_ticks: i32,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MeleeVariant {
    Plain,
    Zombie,
    Spider,
    /// Skeleton melee fallback (`setAggressive` on start/stop).
    Aggressive,
}
impl MeleeAttackGoal {
    pub fn new(speed: f64, follow: bool) -> Self {
        Self { speed, follow_even_if_not_seen: follow, variant: MeleeVariant::Plain, path: None, pathed: [0.0; 3], recalc: 0, until_attack: 0, last_can_use: i64::MIN / 2, raise_arm_ticks: 0 }
    }
    pub fn variant(mut self, v: MeleeVariant) -> Self {
        self.variant = v;
        self
    }
}
impl Goal for MeleeAttackGoal {
    fn name(&self) -> &'static str {
        "melee_attack"
    }
    fn flags(&self) -> u8 {
        flag::MOVE | flag::LOOK
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        let time = cx.w.game_time;
        if time - self.last_can_use < 20 {
            return false;
        }
        self.last_can_use = time;
        let Some(t) = mob.target_view(cx).cloned() else { return false };
        if !t.alive {
            return false;
        }
        self.path = mob.nav_create_path(cx, t.position, 0);
        self.path.is_some() || mob.within_melee_range(&t)
    }
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if self.variant == MeleeVariant::Spider {
            let br = mob.light_dependent_magic_value(cx);
            if br >= 0.5 && mob.random.int(100) == 0 {
                mob.target = None;
                return false;
            }
        }
        let Some(t) = mob.target_view(cx).cloned() else { return false };
        if !t.alive {
            return false;
        }
        if !self.follow_even_if_not_seen {
            return !mob.nav.is_done();
        }
        let p = [t.position[0].floor() as i32, t.position[1].floor() as i32, t.position[2].floor() as i32];
        within_home(mob, p) && !t.creative
    }
    fn start(&mut self, mob: &mut Mob, cx: &mut Cx) {
        let path = self.path.take();
        mob.nav_move_path(cx, path, self.speed);
        mob.aggressive = true;
        self.recalc = 0;
        self.until_attack = 0;
        self.raise_arm_ticks = 0;
    }
    fn stop(&mut self, mob: &mut Mob, cx: &mut Cx) {
        if mob.target.and_then(|r| cx.view(r)).is_some_and(|t| t.creative) {
            mob.target = None;
        }
        mob.aggressive = false;
        mob.nav.stop();
    }
    fn every_tick(&self) -> bool {
        true
    }
    fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        if let Some(t) = mob.target.and_then(|r| cx.view(r)).cloned() {
            mob.look_control.set_look_at(t.eye(), 30.0, 30.0);
            self.recalc = (self.recalc - 1).max(0);
            let sees = self.follow_even_if_not_seen || mob.can_see(cx, &t);
            if sees
                && self.recalc <= 0
                && ((self.pathed == [0.0; 3]) || t.dist2(self.pathed) >= 1.0 || mob.random.float() < 0.05)
            {
                self.pathed = t.position;
                self.recalc = 4 + mob.random.int(7);
                let d2 = mob.distance_to_sqr(t.position);
                if d2 > 1024.0 {
                    self.recalc += 10;
                } else if d2 > 256.0 {
                    self.recalc += 5;
                }
                if !mob.nav_move_to_entity(cx, t.position, 0, self.speed) {
                    self.recalc += 15;
                }
                self.recalc = adjusted(true, self.recalc);
            }
            self.until_attack = (self.until_attack - 1).max(0);
            if self.until_attack <= 0 && mob.within_melee_range(&t) && mob.can_see(cx, &t) {
                self.until_attack = adjusted(true, 20);
                mob.swing();
                mob.do_hurt_target(cx, t.r);
            }
        }
        if self.variant == MeleeVariant::Zombie {
            self.raise_arm_ticks += 1;
            mob.aggressive = self.raise_arm_ticks >= 5 && self.until_attack < adjusted(true, 20) / 2;
        }
    }
}

/// `NearestAttackableTargetGoal` (+ `TargetGoal` continuation rules).
pub struct NearestTargetGoal {
    pub players: bool,
    pub filter: fn(&Mob, &EntityView) -> bool,
    pub interval: i32,
    pub must_see: bool,
    pub must_reach: bool,
    pub gate: fn(&Mob, &Cx) -> bool,
    target: Option<EntityRef>,
    base: TargetBase,
}
#[derive(Default)]
struct TargetBase {
    reach_cache: i32,
    reach_cache_time: i32,
    unseen_ticks: i32,
    unseen_memory: i32,
    target_mob: Option<EntityRef>,
}
impl TargetBase {
    fn new() -> Self {
        Self { unseen_memory: 60, ..Default::default() }
    }
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx, must_see: bool) -> bool {
        let t = mob.target.or(self.target_mob);
        let Some(t) = t.and_then(|r| cx.view(r)).cloned() else { return false };
        if !t.alive || t.creative || !species::can_attack(mob, &t) {
            return false;
        }
        let within = mob.attributes.follow_range;
        if t.dist2(mob.pos()) > within * within {
            return false;
        }
        if must_see {
            if mob.can_see(cx, &t) {
                self.unseen_ticks = 0;
            } else {
                self.unseen_ticks += 1;
                if self.unseen_ticks > reduced(self.unseen_memory) {
                    return false;
                }
            }
        }
        mob.target = Some(t.r);
        true
    }
    fn can_attack(&mut self, mob: &mut Mob, cx: &mut Cx, t: &EntityView, cond: &Targeting, must_reach: bool) -> bool {
        if !cond.test(mob, cx, t) {
            return false;
        }
        let p = [t.position[0].floor() as i32, t.position[1].floor() as i32, t.position[2].floor() as i32];
        if !within_home(mob, p) {
            return false;
        }
        if must_reach {
            self.reach_cache_time -= 1;
            if self.reach_cache_time <= 0 {
                self.reach_cache = 0;
            }
            if self.reach_cache == 0 {
                self.reach_cache = if self.can_reach(mob, cx, t) { 1 } else { 2 };
            }
            if self.reach_cache == 2 {
                return false;
            }
        }
        true
    }
    fn can_reach(&mut self, mob: &mut Mob, cx: &mut Cx, t: &EntityView) -> bool {
        self.reach_cache_time = reduced(10 + mob.random.int(5));
        let Some(path) = mob.nav_create_path(cx, t.position, 0) else { return false };
        let Some(last) = path.end_node() else { return false };
        let xx = last.x - t.position[0].floor() as i32;
        let zz = last.z - t.position[2].floor() as i32;
        f64::from(xx * xx + zz * zz) <= 2.25
    }
    fn start(&mut self) {
        self.reach_cache = 0;
        self.reach_cache_time = 0;
        self.unseen_ticks = 0;
    }
}
impl NearestTargetGoal {
    pub fn players(must_see: bool) -> Self {
        Self { players: true, filter: |_, _| true, interval: reduced(10), must_see, must_reach: false, gate: |_, _| true, target: None, base: TargetBase::new() }
    }
    pub fn mobs(must_see: bool, filter: fn(&Mob, &EntityView) -> bool) -> Self {
        Self { players: false, filter, ..Self::players(must_see) }
    }
    pub fn interval(mut self, ticks: i32) -> Self {
        self.interval = reduced(ticks);
        self
    }
    pub fn must_reach(mut self) -> Self {
        self.must_reach = true;
        self
    }
    pub fn gate(mut self, gate: fn(&Mob, &Cx) -> bool) -> Self {
        self.gate = gate;
        self
    }
}
impl Goal for NearestTargetGoal {
    fn name(&self) -> &'static str {
        "nearest_attackable_target"
    }
    fn flags(&self) -> u8 {
        flag::TARGET
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if !(self.gate)(mob, cx) {
            return false;
        }
        if self.interval > 0 && mob.random.int(self.interval) != 0 {
            return false;
        }
        let range = mob.attributes.follow_range;
        let mut cond = Targeting::combat().range(range);
        if !self.must_see {
            cond = cond.ignore_los();
        }
        // mustSee is enforced by TargetingConditions' line-of-sight check.
        let eye = mob.eye();
        let filter = self.filter;
        let found = if self.players {
            nearest_player(mob, cx, eye, &cond, &|e| e.is_player()).cloned()
        } else {
            nearest(mob, cx, range, range, eye, &cond, &|e| !e.is_player() && filter(mob, e)).cloned()
        };
        self.target = None;
        if let Some(t) = found
            && (!self.must_reach || self.base.can_attack(mob, cx, &t, &cond, true))
        {
            self.target = Some(t.r);
        }
        self.target.is_some()
    }
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        self.base.can_continue(mob, cx, self.must_see)
    }
    fn start(&mut self, mob: &mut Mob, cx: &mut Cx) {
        mob.set_target(self.target, cx);
        self.base.start();
    }
    fn stop(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.target = None;
        self.base.target_mob = None;
    }
}

/// `HurtByTargetGoal` (optionally alerting others of the same type).
pub struct HurtByTargetGoal {
    pub ignore: &'static [Species],
    pub alert_others: bool,
    pub alert_ignore: &'static [Species],
    timestamp: i32,
    base: TargetBase,
}
impl HurtByTargetGoal {
    pub fn new() -> Self {
        Self { ignore: &[], alert_others: false, alert_ignore: &[], timestamp: 0, base: TargetBase::new() }
    }
    pub fn ignoring(mut self, ignore: &'static [Species]) -> Self {
        self.ignore = ignore;
        self
    }
    pub fn alert(mut self, except: &'static [Species]) -> Self {
        self.alert_others = true;
        self.alert_ignore = except;
        self
    }
}
impl Default for HurtByTargetGoal {
    fn default() -> Self {
        Self::new()
    }
}
impl Goal for HurtByTargetGoal {
    fn name(&self) -> &'static str {
        "hurt_by_target"
    }
    fn flags(&self) -> u8 {
        flag::TARGET
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        let Some(by) = mob.last_hurt_by else { return false };
        if mob.last_hurt_by_timestamp == self.timestamp {
            return false;
        }
        let Some(v) = cx.view(by).cloned() else { return false };
        if v.species.is_some_and(|s| self.ignore.contains(&s)) {
            return false;
        }
        let cond = Targeting::combat().ignore_los().ignore_invisibility();
        self.base.can_attack(mob, cx, &v, &cond, false)
    }
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        self.base.can_continue(mob, cx, true)
    }
    fn start(&mut self, mob: &mut Mob, cx: &mut Cx) {
        let by = mob.last_hurt_by;
        mob.set_target(by, cx);
        self.base.target_mob = mob.target;
        self.timestamp = mob.last_hurt_by_timestamp;
        self.base.unseen_memory = 300;
        if self.alert_others
            && let Some(by) = by
        {
            let within = mob.attributes.follow_range;
            let me = mob.species();
            let p = mob.pos();
            for e in cx.entities {
                let EntityRef::Mob(id) = e.r else { continue };
                if id == mob.id || !e.is(me) || e.target.is_some() || !e.alive {
                    continue;
                }
                if (e.position[0] - p[0] - 0.5).abs() > within + 0.5 || (e.position[1] - p[1] - 0.5).abs() > 10.5 || (e.position[2] - p[2] - 0.5).abs() > within + 0.5 {
                    continue;
                }
                if e.species.is_some_and(|s| self.alert_ignore.contains(&s)) {
                    continue;
                }
                cx.requests.push(Request::SetTarget { mob: id, target: Some(by) });
            }
        }
        self.base.start();
    }
    fn stop(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.target = None;
        self.base.target_mob = None;
    }
}

/// `AvoidEntityGoal`.
pub struct AvoidEntityGoal {
    pub filter: fn(&Mob, &EntityView) -> bool,
    pub max_dist: f32,
    pub walk: f64,
    pub sprint: f64,
    to_avoid: Option<EntityRef>,
    path: Option<Path>,
}
impl AvoidEntityGoal {
    pub fn new(filter: fn(&Mob, &EntityView) -> bool, max_dist: f32, walk: f64, sprint: f64) -> Self {
        Self { filter, max_dist, walk, sprint, to_avoid: None, path: None }
    }
}
impl Goal for AvoidEntityGoal {
    fn name(&self) -> &'static str {
        "avoid_entity"
    }
    fn flags(&self) -> u8 {
        flag::MOVE
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        let d = f64::from(self.max_dist);
        let cond = Targeting::combat().range(d);
        let filter = self.filter;
        let pos = mob.pos();
        let Some(t) = nearest(mob, cx, d, 3.0, pos, &cond, &|e| !e.creative && filter(mob, e)).cloned() else { return false };
        self.to_avoid = Some(t.r);
        let Some(p) = default_random_pos_away(mob, cx, 16, 7, t.position) else { return false };
        if t.dist2(p) < t.dist2(mob.pos()) {
            return false;
        }
        self.path = mob.nav_create_path(cx, p, 0);
        self.path.is_some()
    }
    fn can_continue(&mut self, mob: &mut Mob, _cx: &mut Cx) -> bool {
        !mob.nav.is_done()
    }
    fn start(&mut self, mob: &mut Mob, cx: &mut Cx) {
        let path = self.path.take();
        mob.nav_move_path(cx, path, self.walk);
    }
    fn stop(&mut self, _mob: &mut Mob, _cx: &mut Cx) {
        self.to_avoid = None;
    }
    fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        if let Some(t) = self.to_avoid.and_then(|r| cx.view(r)) {
            mob.nav.speed_modifier = if t.dist2(mob.pos()) < 49.0 { self.sprint } else { self.walk };
        }
    }
}

/// `LeapAtTargetGoal`.
pub struct LeapAtTargetGoal {
    pub yd: f32,
    target: [f64; 3],
}
impl LeapAtTargetGoal {
    pub fn new(yd: f32) -> Self {
        Self { yd, target: [0.0; 3] }
    }
}
impl Goal for LeapAtTargetGoal {
    fn name(&self) -> &'static str {
        "leap_at_target"
    }
    fn flags(&self) -> u8 {
        flag::JUMP | flag::MOVE
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        let Some(t) = mob.target.and_then(|r| cx.view(r)).cloned() else { return false };
        let d = mob.distance_to_sqr(t.position);
        if !(4.0..=16.0).contains(&d) {
            return false;
        }
        if !mob.on_ground {
            return false;
        }
        self.target = t.position;
        mob.random.int(reduced(5)) == 0
    }
    fn can_continue(&mut self, mob: &mut Mob, _cx: &mut Cx) -> bool {
        !mob.on_ground
    }
    fn start(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        let v = mob.velocity;
        let (mut dx, mut dz) = (self.target[0] - mob.position.x, self.target[2] - mob.position.z);
        if dx * dx + dz * dz > 1.0e-7 {
            let l = (dx * dx + dz * dz).sqrt();
            dx = dx / l * 0.4 + v.x * 0.2;
            dz = dz / l * 0.4 + v.z * 0.2;
        }
        mob.velocity = horizon_core::geometry::Vec3::new(dx, f64::from(self.yd), dz);
    }
}

/// `EatBlockGoal` (sheep grazing).
pub struct EatBlockGoal {
    tick: i32,
}
impl EatBlockGoal {
    pub fn new() -> Self {
        Self { tick: 0 }
    }
}
impl Default for EatBlockGoal {
    fn default() -> Self {
        Self::new()
    }
}
impl Goal for EatBlockGoal {
    fn name(&self) -> &'static str {
        "eat_block"
    }
    fn flags(&self) -> u8 {
        flag::MOVE | flag::LOOK | flag::JUMP
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        let chance = adjusted(false, if mob.is_baby() { 50 } else { 1000 });
        if mob.random.int(chance) != 0 {
            return false;
        }
        let [x, y, z] = mob.block_pos();
        let rules = cx.w.rules;
        rules.block_in_tag("minecraft:edible_for_sheep", cx.level.state(x, y, z)) || cx.level.name_at(x, y - 1, z) == "minecraft:grass_block"
    }
    fn start(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        self.tick = adjusted(false, 40);
        mob.data.eat_tick = 40;
        mob.nav.stop();
    }
    fn stop(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        self.tick = 0;
        mob.data.eat_goal_tick = 0;
    }
    fn can_continue(&mut self, _mob: &mut Mob, _cx: &mut Cx) -> bool {
        self.tick > 0
    }
    fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        self.tick = (self.tick - 1).max(0);
        mob.data.eat_goal_tick = self.tick;
        if self.tick == adjusted(false, 4) {
            let [x, y, z] = mob.block_pos();
            let rules = cx.w.rules;
            let state = cx.level.state(x, y, z);
            if rules.block_in_tag("minecraft:edible_for_sheep", state) {
                if cx.w.mob_griefing {
                    cx.events.push(super::MobEvent::DestroyBlock([x, y, z]));
                }
                species::ate(mob, cx);
            } else if cx.level.name_at(x, y - 1, z) == "minecraft:grass_block" {
                if cx.w.mob_griefing
                    && let Some(dirt) = rules.default_state("minecraft:dirt")
                {
                    cx.events.push(super::MobEvent::BreakEffect([x, y - 1, z], cx.level.state(x, y - 1, z)));
                    cx.events.push(super::MobEvent::SetBlock([x, y - 1, z], dirt));
                }
                species::ate(mob, cx);
            }
        }
    }
}

/// `RestrictSunGoal`.
pub struct RestrictSunGoal;
impl Goal for RestrictSunGoal {
    fn name(&self) -> &'static str {
        "restrict_sun"
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        cx.w.daylight && mob.head_item.is_none()
    }
    fn start(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.nav.avoid_sun = true;
    }
    fn stop(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.nav.avoid_sun = false;
    }
}

/// `FleeSunGoal`.
pub struct FleeSunGoal {
    pub speed: f64,
    wanted: [f64; 3],
}
impl FleeSunGoal {
    pub fn new(speed: f64) -> Self {
        Self { speed, wanted: [0.0; 3] }
    }
}
impl Goal for FleeSunGoal {
    fn name(&self) -> &'static str {
        "flee_sun"
    }
    fn flags(&self) -> u8 {
        flag::MOVE
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if mob.target.is_some() || !cx.w.daylight || mob.fire_ticks <= 0 {
            return false;
        }
        let [x, y, z] = mob.block_pos();
        if !cx.level.can_see_sky(x, y, z) || mob.head_item.is_some() {
            return false;
        }
        for _ in 0..10 {
            let p = [x + mob.random.int(20) - 10, y + mob.random.int(6) - 3, z + mob.random.int(20) - 10];
            if !cx.level.can_see_sky(p[0], p[1], p[2]) && mob.walk_target_value(cx, p) < 0.0 {
                self.wanted = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
                return true;
            }
        }
        false
    }
    fn can_continue(&mut self, mob: &mut Mob, _cx: &mut Cx) -> bool {
        !mob.nav.is_done()
    }
    fn start(&mut self, mob: &mut Mob, cx: &mut Cx) {
        mob.nav_move_to(cx, self.wanted, self.speed);
    }
}

/// `SwellGoal` (creeper fuse while the target is within 3 blocks).
pub struct SwellGoal {
    target: Option<EntityRef>,
}
impl SwellGoal {
    pub fn new() -> Self {
        Self { target: None }
    }
}
impl Default for SwellGoal {
    fn default() -> Self {
        Self::new()
    }
}
impl Goal for SwellGoal {
    fn name(&self) -> &'static str {
        "swell"
    }
    fn flags(&self) -> u8 {
        flag::MOVE
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        let t = mob.target_view(cx);
        mob.data.swell_dir > 0 || t.is_some_and(|t| t.alive && t.dist2(mob.pos()) < 9.0)
    }
    fn start(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.nav.stop();
        self.target = mob.target;
    }
    fn stop(&mut self, _mob: &mut Mob, _cx: &mut Cx) {
        self.target = None;
    }
    fn every_tick(&self) -> bool {
        true
    }
    fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        let t = self.target.and_then(|r| cx.view(r)).cloned();
        mob.data.swell_dir = match t {
            Some(t) if t.alive => {
                if t.dist2(mob.pos()) > 49.0 || !mob.can_see(cx, &t) {
                    -1
                } else {
                    1
                }
            }
            _ => -1,
        };
    }
}

/// `RangedBowAttackGoal`.
pub struct RangedBowAttackGoal {
    pub speed: f64,
    pub interval_min: i32,
    pub radius_sqr: f32,
    attack_time: i32,
    see_time: i32,
    clockwise: bool,
    backwards: bool,
    strafing_time: i32,
}
impl RangedBowAttackGoal {
    pub fn new(speed: f64, interval_min: i32, radius: f32) -> Self {
        Self { speed, interval_min, radius_sqr: radius * radius, attack_time: -1, see_time: 0, clockwise: false, backwards: false, strafing_time: -1 }
    }
}
fn holding_bow(mob: &Mob) -> bool {
    mob.main_hand.as_deref() == Some("minecraft:bow")
}
impl Goal for RangedBowAttackGoal {
    fn name(&self) -> &'static str {
        "ranged_bow_attack"
    }
    fn flags(&self) -> u8 {
        flag::MOVE | flag::LOOK
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        mob.target_view(cx).is_some() && holding_bow(mob)
    }
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        (self.can_use(mob, cx) || !mob.nav.is_done()) && holding_bow(mob)
    }
    fn start(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.aggressive = true;
    }
    fn stop(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.aggressive = false;
        self.see_time = 0;
        self.attack_time = -1;
        mob.using_item = false;
        mob.use_item_ticks = 0;
    }
    fn every_tick(&self) -> bool {
        true
    }
    fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        let Some(t) = mob.target.and_then(|r| cx.view(r)).cloned() else { return };
        let d2 = mob.distance_to_sqr(t.position);
        let sees = mob.can_see(cx, &t);
        let saw = self.see_time > 0;
        if sees != saw {
            self.see_time = 0;
        }
        if sees {
            self.see_time += 1;
        } else {
            self.see_time -= 1;
        }
        let r = f64::from(self.radius_sqr);
        if !(d2 > r) && self.see_time >= 20 {
            mob.nav.stop();
            self.strafing_time += 1;
        } else {
            mob.nav_move_to_entity(cx, t.position, 1, self.speed);
            self.strafing_time = -1;
        }
        if self.strafing_time >= 20 {
            if f64::from(mob.random.float()) < 0.3 {
                self.clockwise = !self.clockwise;
            }
            if f64::from(mob.random.float()) < 0.3 {
                self.backwards = !self.backwards;
            }
            self.strafing_time = 0;
        }
        if self.strafing_time > -1 {
            if d2 > r * 0.75 {
                self.backwards = false;
            } else if d2 < r * 0.25 {
                self.backwards = true;
            }
            mob.move_control.strafe(if self.backwards { -0.5 } else { 0.5 }, if self.clockwise { 0.5 } else { -0.5 });
            let eye = t.eye();
            mob.look_at_direct(t.position, eye[1], 30.0, 30.0);
        } else {
            mob.look_control.set_look_at(t.eye(), 30.0, 30.0);
        }
        if mob.using_item {
            if !sees && self.see_time < -60 {
                mob.using_item = false;
                mob.use_item_ticks = 0;
            } else if sees {
                let pull = mob.use_item_ticks;
                if pull >= 20 {
                    mob.using_item = false;
                    mob.use_item_ticks = 0;
                    species::perform_ranged_attack(mob, cx, &t, bow_power(pull));
                    self.attack_time = self.interval_min;
                }
            }
        } else {
            self.attack_time -= 1;
            if self.attack_time <= 0 && self.see_time >= -60 {
                mob.using_item = true;
                mob.use_item_ticks = 0;
            }
        }
    }
}
/// `BowItem.getPowerForTime`.
pub fn bow_power(ticks: i32) -> f32 {
    let p = ticks as f32 / 20.0;
    ((p * p + p * 2.0) / 3.0).min(1.0)
}

/// `RangedAttackGoal` (witch potions, snow golems).
pub struct RangedAttackGoal {
    pub speed: f64,
    pub interval_min: i32,
    pub interval_max: i32,
    pub radius: f32,
    attack_time: i32,
    see_time: i32,
    target: Option<EntityRef>,
}
impl RangedAttackGoal {
    pub fn new(speed: f64, min: i32, max: i32, radius: f32) -> Self {
        Self { speed, interval_min: min, interval_max: max, radius, attack_time: -1, see_time: 0, target: None }
    }
}
impl Goal for RangedAttackGoal {
    fn name(&self) -> &'static str {
        "ranged_attack"
    }
    fn flags(&self) -> u8 {
        flag::MOVE | flag::LOOK
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        match mob.target_view(cx) {
            Some(t) if t.alive => {
                self.target = Some(t.r);
                true
            }
            _ => false,
        }
    }
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        self.can_use(mob, cx) || self.target.and_then(|r| cx.view(r)).is_some_and(|t| t.alive) && !mob.nav.is_done()
    }
    fn stop(&mut self, _mob: &mut Mob, _cx: &mut Cx) {
        self.target = None;
        self.see_time = 0;
        self.attack_time = -1;
    }
    fn every_tick(&self) -> bool {
        true
    }
    fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        let Some(t) = self.target.and_then(|r| cx.view(r)).cloned() else { return };
        let d2 = mob.distance_to_sqr([t.position[0], t.position[1], t.position[2]]);
        let sees = mob.can_see(cx, &t);
        if sees {
            self.see_time += 1;
        } else {
            self.see_time = 0;
        }
        let r2 = f64::from(self.radius * self.radius);
        if !(d2 > r2) && self.see_time >= 5 {
            mob.nav.stop();
        } else {
            mob.nav_move_to_entity(cx, t.position, 1, self.speed);
        }
        mob.look_control.set_look_at(t.eye(), 30.0, 30.0);
        self.attack_time -= 1;
        if self.attack_time == 0 {
            if !sees {
                return;
            }
            let dist = (d2.sqrt() as f32) / self.radius;
            let power = dist.clamp(0.1, 1.0);
            species::perform_ranged_attack(mob, cx, &t, power);
            self.attack_time = ((dist * (self.interval_max - self.interval_min) as f32) + self.interval_min as f32).floor() as i32;
        } else if self.attack_time < 0 {
            self.attack_time = ((d2.sqrt() as f32 / self.radius) * (self.interval_max - self.interval_min) as f32 + self.interval_min as f32).floor() as i32;
        }
    }
}

/// `MoveTowardsRestrictionGoal`.
pub struct MoveTowardsRestrictionGoal {
    pub speed: f64,
    wanted: [f64; 3],
}
impl MoveTowardsRestrictionGoal {
    pub fn new(speed: f64) -> Self {
        Self { speed, wanted: [0.0; 3] }
    }
}
impl Goal for MoveTowardsRestrictionGoal {
    fn name(&self) -> &'static str {
        "move_towards_restriction"
    }
    fn flags(&self) -> u8 {
        flag::MOVE
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        let Some((c, _)) = mob.home else { return false };
        if within_home(mob, mob.block_pos()) {
            return false;
        }
        let to = [f64::from(c[0]) + 0.5, f64::from(c[1]), f64::from(c[2]) + 0.5];
        match default_random_pos_towards(mob, cx, 16, 7, to, f64::from(std::f32::consts::FRAC_PI_2)) {
            Some(p) => {
                self.wanted = p;
                true
            }
            None => false,
        }
    }
    fn can_continue(&mut self, mob: &mut Mob, _cx: &mut Cx) -> bool {
        !mob.nav.is_done()
    }
    fn start(&mut self, mob: &mut Mob, cx: &mut Cx) {
        mob.nav_move_to(cx, self.wanted, self.speed);
    }
}

/// `SitWhenOrderedToGoal`.
pub struct SitWhenOrderedToGoal;
impl Goal for SitWhenOrderedToGoal {
    fn name(&self) -> &'static str {
        "sit_when_ordered"
    }
    fn flags(&self) -> u8 {
        flag::JUMP | flag::MOVE
    }
    fn can_continue(&mut self, mob: &mut Mob, _cx: &mut Cx) -> bool {
        mob.data.order_to_sit
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if !mob.data.tame || mob.in_water || !mob.on_ground {
            return false;
        }
        // The owner (player) being attacked nearby makes the pet stand up.
        if cx.player().is_some_and(|p| p.dist2(mob.pos()) < 144.0) && cx.w.owner_hurt_by.is_some() {
            return false;
        }
        mob.data.order_to_sit
    }
    fn start(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.nav.stop();
        mob.data.sitting = true;
    }
    fn stop(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.data.sitting = false;
    }
}

/// `FollowOwnerGoal` (teleports when far from the owner).
pub struct FollowOwnerGoal {
    pub speed: f64,
    pub start_distance: f32,
    pub stop_distance: f32,
    recalc: i32,
    old_water_cost: f32,
}
impl FollowOwnerGoal {
    pub fn new(speed: f64, start: f32, stop: f32) -> Self {
        Self { speed, start_distance: start, stop_distance: stop, recalc: 0, old_water_cost: 0.0 }
    }
}
impl Goal for FollowOwnerGoal {
    fn name(&self) -> &'static str {
        "follow_owner"
    }
    fn flags(&self) -> u8 {
        flag::MOVE | flag::LOOK
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if !mob.data.tame || mob.data.order_to_sit || mob.data.sitting {
            return false;
        }
        let Some(p) = cx.player() else { return false };
        if !p.alive {
            return false;
        }
        let d = f64::from(self.start_distance);
        p.dist2(mob.pos()) >= d * d
    }
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if mob.nav.is_done() || mob.data.order_to_sit {
            return false;
        }
        let Some(p) = cx.player() else { return false };
        let d = f64::from(self.stop_distance);
        p.dist2(mob.pos()) > d * d
    }
    fn start(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        self.recalc = 0;
        self.old_water_cost = mob.malus.get(PathType::Water);
        mob.malus.set(PathType::Water, 0.0);
    }
    fn stop(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.nav.stop();
        mob.malus.set(PathType::Water, self.old_water_cost);
    }
    fn tick(&mut self, mob: &mut Mob, cx: &mut Cx) {
        let Some(p) = cx.player().cloned() else { return };
        let teleport = p.dist2(mob.pos()) >= 144.0;
        if !teleport {
            let (ys, xs) = (10.0, mob.max_head_x_rot() as f32);
            mob.look_control.set_look_at(p.eye(), ys, xs);
        }
        self.recalc -= 1;
        if self.recalc <= 0 {
            self.recalc = adjusted(false, 10);
            if teleport {
                species::try_teleport_to_owner(mob, cx, &p);
            } else {
                mob.nav_move_to_entity(cx, p.position, 1, self.speed);
            }
        }
    }
}

/// `OwnerHurtByTargetGoal` / `OwnerHurtTargetGoal` (owner = the player).
pub struct OwnerDefenseGoal {
    /// true: target whoever hurt the owner; false: whoever the owner hurt.
    pub defend: bool,
    timestamp: i32,
    target: Option<EntityRef>,
    base: TargetBase,
}
impl OwnerDefenseGoal {
    pub fn new(defend: bool) -> Self {
        Self { defend, timestamp: 0, target: None, base: TargetBase::new() }
    }
}
impl Goal for OwnerDefenseGoal {
    fn name(&self) -> &'static str {
        if self.defend { "owner_hurt_by_target" } else { "owner_hurt_target" }
    }
    fn flags(&self) -> u8 {
        flag::TARGET
    }
    fn can_use(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        if !mob.data.tame || mob.data.order_to_sit {
            return false;
        }
        let (r, stamp) = if self.defend { (cx.w.owner_hurt_by, cx.w.owner_hurt_by_time) } else { (cx.w.owner_last_hurt, cx.w.owner_last_hurt_time) };
        let Some(r) = r else { return false };
        if stamp == self.timestamp || r == EntityRef::Mob(mob.id) {
            return false;
        }
        let Some(t) = cx.view(r).cloned() else { return false };
        if !species::wants_to_attack(mob, &t) {
            return false;
        }
        self.target = Some(r);
        let cond = Targeting::combat();
        self.base.can_attack(mob, cx, &t, &cond, false)
    }
    fn can_continue(&mut self, mob: &mut Mob, cx: &mut Cx) -> bool {
        self.base.can_continue(mob, cx, false)
    }
    fn start(&mut self, mob: &mut Mob, cx: &mut Cx) {
        mob.set_target(self.target, cx);
        self.timestamp = if self.defend { cx.w.owner_hurt_by_time } else { cx.w.owner_last_hurt_time };
        self.base.start();
    }
    fn stop(&mut self, mob: &mut Mob, _cx: &mut Cx) {
        mob.target = None;
    }
}

/// Look at the surface target: helper for `find_surface_position` users.
pub fn surface(cx: &Cx, p: [i32; 3]) -> [i32; 3] {
    find_surface_position(&cx.level, p)
}
